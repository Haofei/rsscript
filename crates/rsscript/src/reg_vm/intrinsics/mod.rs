use super::*;
use crate::reg_vm::resources::*;
use crate::reg_vm::runtime_values::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;
use crate::reg_vm::value_ops::*;

mod bytes;
mod char;
mod date;
mod deque;
mod hex;
mod json;
mod list;
mod map;
mod math;
mod option;
mod path;
mod regex;
mod result;
mod scalar;
mod set;
mod string;
mod url;

impl RegVm {
    // See `try_exec_pure`: interior-mutable `VmMapKey` is safe because
    // `retains(key)` forbids mutating a key while it is in a map.
    #[allow(clippy::mutable_key_type)]
    pub(super) fn call_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        self.charge_intrinsic_call()?;
        match intrinsic {
            RegIntrinsic::ArgsAll => Ok(VmValue::List(Rc::new(RefCell::new(
                self.args.iter().cloned().map(VmValue::string).collect(),
            )))),
            RegIntrinsic::ArgsCount => Ok(VmValue::Int(self.args.len() as i64)),
            RegIntrinsic::ArgsGet => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| self.args.get(index).cloned())
                    .map(|value| VmValue::some(VmValue::string(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ArgsGetOrDefault => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| self.args.get(index).cloned())
                        .unwrap_or(default),
                ))
            }
            RegIntrinsic::AssertEqual => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{}` does not equal right `{}`.",
                        left.display(),
                        right.display()
                    )))
                }
            }
            RegIntrinsic::AssertEqualBool => {
                let left = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::AssertEqualInt => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::Base64Decode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::Base64DecodeString => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|error| decode_error_value(error.to_string()))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map(VmValue::string)
                            .map_err(|error| decode_error_value(error.to_string()))
                    });
                Ok(json_result(result))
            }
            RegIntrinsic::Base64Encode => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
                ))
            }
            RegIntrinsic::Base64EncodeBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value),
                ))
            }
            RegIntrinsic::BytesConcat
            | RegIntrinsic::BytesConsume
            | RegIntrinsic::BytesFromString
            | RegIntrinsic::BytesFromUints
            | RegIntrinsic::BytesIsEmpty
            | RegIntrinsic::BytesLen
            | RegIntrinsic::BytesSlice
            | RegIntrinsic::BytesToString
            | RegIntrinsic::BytesToUints
            | RegIntrinsic::BytesViewStartsWith
            | RegIntrinsic::BytesViewToBytes => {
                self.exec_bytes_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::BufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(Vec::with_capacity(
                    size.max(0) as usize
                ))))
            }
            RegIntrinsic::DynFrom => Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone()),
            RegIntrinsic::CancellationSourceCancel => {
                // RSS-level *cooperative* cancellation: flips the program-visible
                // flag a task must poll (e.g. `token.is_cancelled()`); it preempts
                // only at await/poll points, not inside a tight compute loop. The
                // host-level *preemptive* hook for a runaway loop is the ambient
                // `limits.cancel` atomic polled by `tick()` — see `RegVm::tick`.
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                self.cancellation_flags.insert(id, true);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::CancellationSourceNew => {
                let id = self.next_cancellation_id;
                self.next_cancellation_id = self.next_cancellation_id.saturating_add(1);
                self.cancellation_flags.insert(id, false);
                Ok(cancellation_source_value(id))
            }
            RegIntrinsic::CancellationSourceToken => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                Ok(cancellation_token_value(id))
            }
            RegIntrinsic::CancellationTokenIsCancelled => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationToken",
                )?;
                Ok(VmValue::Bool(
                    self.cancellation_flags.get(&id).copied().unwrap_or(false),
                ))
            }
            RegIntrinsic::ChannelBounded => {
                let capacity = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(if capacity <= 0 {
                    Err(channel_error_value("channel capacity must be positive"))
                } else {
                    let id = self.next_channel_id;
                    self.next_channel_id = self.next_channel_id.saturating_add(1);
                    self.channels.insert(id, VmChannel::new(capacity as usize));
                    Ok(channel_value(id, capacity, false))
                }))
            }
            RegIntrinsic::ChannelSender => {
                let channel = expect_channel_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let state = self.channel_state_mut(channel.id)?;
                state.senders = state.senders.saturating_add(1);
                Ok(sender_value(channel.id, false))
            }
            RegIntrinsic::ChannelReceiver => {
                let channel_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Channel.receiver missing channel.".to_string())
                })?;
                let mut channel = expect_channel_ref(self.reg(base + channel_reg))?;
                let already_taken = self
                    .channels
                    .get(&channel.id)
                    .map(|state| state.receiver_taken)
                    .unwrap_or(channel.receiver_taken);
                Ok(json_result(if already_taken {
                    Err(channel_error_value("channel receiver already taken"))
                } else {
                    channel.receiver_taken = true;
                    self.channel_state_mut(channel.id)?.receiver_taken = true;
                    self.set_reg(base + channel_reg, channel.to_value());
                    Ok(receiver_value(channel.id, false))
                }))
            }
            RegIntrinsic::ChannelErrorMessage | RegIntrinsic::DecodeErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::CharCompare
            | RegIntrinsic::CharFromCode
            | RegIntrinsic::CharIsAlphanumeric
            | RegIntrinsic::CharIsAlpha
            | RegIntrinsic::CharIsDigit
            | RegIntrinsic::CharIsLower
            | RegIntrinsic::CharIsUpper
            | RegIntrinsic::CharIsWhitespace
            | RegIntrinsic::CharToCode
            | RegIntrinsic::CharToLower
            | RegIntrinsic::CharToString
            | RegIntrinsic::CharToUpper => {
                self.exec_char_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::DateAddDays
            | RegIntrinsic::DateAddMs
            | RegIntrinsic::DateDay
            | RegIntrinsic::DateDaysBetween
            | RegIntrinsic::DateDaysInMonth
            | RegIntrinsic::DateFormatIso
            | RegIntrinsic::DateFormatYmd
            | RegIntrinsic::DateHour
            | RegIntrinsic::DateIsLeapYear
            | RegIntrinsic::DateMinute
            | RegIntrinsic::DateMonth
            | RegIntrinsic::DateParseIso
            | RegIntrinsic::DateParseYmd
            | RegIntrinsic::DateSecond
            | RegIntrinsic::DateStartOfDay
            | RegIntrinsic::DateWeekday
            | RegIntrinsic::DateYear => {
                self.exec_date_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::CsvParseRow => {
                let buffer = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(json_result(csv_parse_row_value(
                    &expect_row_buffer_bytes_ref(buffer)?,
                )))
            }
            RegIntrinsic::DeadlineAfter | RegIntrinsic::DeadlineAfterMs => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(deadline_value(deadline_after_ms(ms)))
            }
            RegIntrinsic::DeadlineIsExpired => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(clock_system_unix_ms() >= deadline))
            }
            RegIntrinsic::DeadlineRemainingMs => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    deadline.saturating_sub(clock_system_unix_ms()).max(0),
                ))
            }
            RegIntrinsic::DequeIsEmpty
            | RegIntrinsic::DequeLen
            | RegIntrinsic::DequeNew
            | RegIntrinsic::DequeToList => {
                self.exec_deque_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::DiffUnified => {
                let old = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let new = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(diff_unified_string(old, new)))
            }
            RegIntrinsic::DurationAdd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left + right))
            }
            RegIntrinsic::DurationAsMs | RegIntrinsic::DurationMs => Ok(VmValue::Int(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?,
            )),
            RegIntrinsic::DurationAsSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value / 1000))
            }
            RegIntrinsic::DurationSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value * 1000))
            }
            RegIntrinsic::FalliblePipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::FalliblePipelineMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow().get(index).expect("index in bounds");
                            mapped.push(self.call_closure_one(unit, &mapper, value, next_base)?);
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                            mapped,
                        )))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineFilter => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut filtered = Vec::new();
                        for index in 0..len {
                            let value = items.borrow().get(index).expect("index in bounds");
                            let keep =
                                self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                            if expect_bool_ref(&keep)? {
                                filtered.push(value);
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                            filtered,
                        )))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineEach => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let values = items.borrow().clone();
                        for value in values.iter() {
                            let _ = self.call_closure_one(unit, &action, value, next_base)?;
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(values))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineTryMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow().get(index).expect("index in bounds");
                            match result_variant_payload(
                                &self.call_closure_one(unit, &mapper, value, next_base)?,
                            )? {
                                Ok(value) => mapped.push(value),
                                Err(error) => return Ok(value_err(error)),
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                            mapped,
                        )))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::HashSha256Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value)))
            }
            RegIntrinsic::HashSha256String => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value.as_bytes())))
            }
            RegIntrinsic::HashSha3_224Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(sha3_224_digest(value))))
            }
            RegIntrinsic::HashSha3_256Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(sha3_256_digest(value))))
            }
            RegIntrinsic::HashShake128Bytes => {
                let value = match intrinsic_arg(&self.stack, base, args, 0)? {
                    VmValue::Bytes(value) => Rc::clone(value),
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM expected Bytes, got `{}`.",
                            other.display()
                        )));
                    }
                };
                let out_len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let out_len = usize::try_from(out_len).map_err(|_| {
                    EvalError::Runtime(format!(
                        "Hash.shake128_bytes output length must be non-negative, got {out_len}"
                    ))
                })?;
                if out_len > MAX_INTRINSIC_OUTPUT_BYTES {
                    return Err(EvalError::Runtime(format!(
                        "Hash.shake128_bytes output exceeds the {} byte limit",
                        MAX_INTRINSIC_OUTPUT_BYTES
                    )));
                }
                self.account_bytes(out_len)?;
                Ok(VmValue::Bytes(Rc::new(shake128_digest(&value, out_len))))
            }
            RegIntrinsic::HmacSha256Bytes => {
                let key = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(key, value)))
            }
            RegIntrinsic::HmacSha256String => {
                let key = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(
                    key.as_bytes(),
                    value.as_bytes(),
                )))
            }
            RegIntrinsic::GzipDecompressBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut decoder = GzDecoder::new(value);
                let mut out = Vec::new();
                Ok(json_result(
                    decoder
                        .read_to_end(&mut out)
                        .map(|_| VmValue::Bytes(Rc::new(out)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HexDecode | RegIntrinsic::HexEncode | RegIntrinsic::HexEncodeString => {
                self.exec_hex_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::InstantElapsed => {
                let start = expect_instant_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    clock_system_unix_ms().saturating_sub(start).max(0),
                ))
            }
            RegIntrinsic::IntBitAnd
            | RegIntrinsic::IntBitNot
            | RegIntrinsic::IntBitOr
            | RegIntrinsic::IntBitXor
            | RegIntrinsic::IntShiftLeft
            | RegIntrinsic::IntShiftRight
            | RegIntrinsic::IntToString
            | RegIntrinsic::IntToFloat
            | RegIntrinsic::FloatToString
            | RegIntrinsic::FloatIsFinite
            | RegIntrinsic::FloatIsInfinite
            | RegIntrinsic::FloatIsNan => {
                self.exec_scalar_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::MathAbs
            | RegIntrinsic::MathAbsFloat
            | RegIntrinsic::MathCeil
            | RegIntrinsic::MathClamp
            | RegIntrinsic::MathClampFloat
            | RegIntrinsic::MathCos
            | RegIntrinsic::MathExp
            | RegIntrinsic::MathExp2
            | RegIntrinsic::MathFloor
            | RegIntrinsic::MathLog
            | RegIntrinsic::MathLog2
            | RegIntrinsic::MathMax
            | RegIntrinsic::MathMaxFloat
            | RegIntrinsic::MathMin
            | RegIntrinsic::MathMinFloat
            | RegIntrinsic::MathPow
            | RegIntrinsic::MathPowFloat
            | RegIntrinsic::MathRound
            | RegIntrinsic::MathSaturatingAdd
            | RegIntrinsic::MathSaturatingMul
            | RegIntrinsic::MathSaturatingSub
            | RegIntrinsic::MathSin
            | RegIntrinsic::MathSqrt
            | RegIntrinsic::MathTanh
            | RegIntrinsic::MathTruncFloat
            | RegIntrinsic::MathWrappingAdd
            | RegIntrinsic::MathWrappingMul
            | RegIntrinsic::MathWrappingSub => {
                self.exec_math_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::JsonArray
            | RegIntrinsic::JsonArrayBools
            | RegIntrinsic::JsonArrayContainsPrefix
            | RegIntrinsic::JsonArrayContainsString
            | RegIntrinsic::JsonArrayContainsSubstring
            | RegIntrinsic::JsonArrayCountWhere
            | RegIntrinsic::JsonArrayFold
            | RegIntrinsic::JsonArrayGet
            | RegIntrinsic::JsonArrayInts
            | RegIntrinsic::JsonArrayLen
            | RegIntrinsic::JsonArrayStrings
            | RegIntrinsic::JsonAt
            | RegIntrinsic::JsonAtBool
            | RegIntrinsic::JsonAtBoolOr
            | RegIntrinsic::JsonAtInt
            | RegIntrinsic::JsonAtIntOr
            | RegIntrinsic::JsonAtOptional
            | RegIntrinsic::JsonAtOptionalBool
            | RegIntrinsic::JsonAtOptionalInt
            | RegIntrinsic::JsonAtOptionalString
            | RegIntrinsic::JsonAtOr
            | RegIntrinsic::JsonAtString
            | RegIntrinsic::JsonAtStringOr
            | RegIntrinsic::JsonAtToString
            | RegIntrinsic::JsonAtToStringOr
            | RegIntrinsic::JsonAsBool
            | RegIntrinsic::JsonAsInt
            | RegIntrinsic::JsonAsString
            | RegIntrinsic::JsonBoolAt
            | RegIntrinsic::JsonBoolAtOr
            | RegIntrinsic::JsonBoolField
            | RegIntrinsic::JsonClone
            | RegIntrinsic::JsonDecode
            | RegIntrinsic::JsonDecodeText
            | RegIntrinsic::JsonEncode
            | RegIntrinsic::JsonErrorMessage
            | RegIntrinsic::JsonField
            | RegIntrinsic::JsonFieldBool
            | RegIntrinsic::JsonFieldInt
            | RegIntrinsic::JsonParseOk
            | RegIntrinsic::JsonFieldOk
            | RegIntrinsic::JsonFieldIntOk
            | RegIntrinsic::JsonFieldOptional
            | RegIntrinsic::JsonFieldOptionalBool
            | RegIntrinsic::JsonFieldOptionalInt
            | RegIntrinsic::JsonFieldOptionalString
            | RegIntrinsic::JsonFieldString
            | RegIntrinsic::JsonIntAt
            | RegIntrinsic::JsonIntAtOr
            | RegIntrinsic::JsonIsArray
            | RegIntrinsic::JsonIsNull
            | RegIntrinsic::JsonIsObject
            | RegIntrinsic::JsonIntField
            | RegIntrinsic::JsonKind
            | RegIntrinsic::JsonObject
            | RegIntrinsic::JsonObjectKeys
            | RegIntrinsic::JsonObjectLen
            | RegIntrinsic::JsonParse
            | RegIntrinsic::JsonQuoteString
            | RegIntrinsic::JsonRawField
            | RegIntrinsic::JsonStringAt
            | RegIntrinsic::JsonStringAtOr
            | RegIntrinsic::JsonStringArray
            | RegIntrinsic::JsonStringField
            | RegIntrinsic::JsonStrings
            | RegIntrinsic::JsonToStringAt
            | RegIntrinsic::JsonToStringAtOr
            | RegIntrinsic::JsonToString
            | RegIntrinsic::JsonValue
            | RegIntrinsic::JsonValues => {
                self.exec_json_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::ListAll
            | RegIntrinsic::ListAny
            | RegIntrinsic::ListContains
            | RegIntrinsic::ListContainsValue
            | RegIntrinsic::ListCountWhere
            | RegIntrinsic::ListConsume
            | RegIntrinsic::ListFind
            | RegIntrinsic::ListFirst
            | RegIntrinsic::ListFlatMap
            | RegIntrinsic::ListFlatten
            | RegIntrinsic::ListGroupBy
            | RegIntrinsic::ListIsEmpty
            | RegIntrinsic::ListJoin
            | RegIntrinsic::ListLast
            | RegIntrinsic::ListDedup
            | RegIntrinsic::ListEnumerate
            | RegIntrinsic::ListMax
            | RegIntrinsic::ListMin
            | RegIntrinsic::ListNew
            | RegIntrinsic::ListPartition
            | RegIntrinsic::ListReverse
            | RegIntrinsic::ListSkip
            | RegIntrinsic::ListSlice
            | RegIntrinsic::ListSum
            | RegIntrinsic::ListZip
            | RegIntrinsic::ListTryFold
            | RegIntrinsic::ListTake
            | RegIntrinsic::ListToJsonStrings
            | RegIntrinsic::ListToJsonValues => {
                self.exec_list_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::ListPipeline | RegIntrinsic::PipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::PipelineEach => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values.iter() {
                    let _ = self.call_closure_one(unit, &action, value, next_base)?;
                }
                self.fresh_list(values)
            }
            RegIntrinsic::PipelineTryMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut mapped = Vec::with_capacity(len);
                for index in 0..len {
                    let value = list.borrow().get(index).expect("index in bounds");
                    match result_variant_payload(
                        &self.call_closure_one(unit, &mapper, value, next_base)?,
                    )? {
                        Ok(value) => mapped.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                let mapped = self.fresh_list(TypedVec::from_values(mapped))?;
                Ok(value_ok(mapped))
            }
            RegIntrinsic::LogError => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.stderr.push_str(&line);
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogErrorJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.stderr.push_str(&value.to_string());
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogTrace => {
                let event =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                let message =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                self.push_stdout(&format!("trace {event}: {message}\n"))?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWrite => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.push_stdout(&line)?;
                self.push_stdout("\n")?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWriteJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.push_stdout(&value.to_string())?;
                self.push_stdout("\n")?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapContainsKey
            | RegIntrinsic::MapFilter
            | RegIntrinsic::MapFold
            | RegIntrinsic::MapForEach
            | RegIntrinsic::MapGetOrDefault
            | RegIntrinsic::MapIsEmpty
            | RegIntrinsic::MapKeys
            | RegIntrinsic::MapLen
            | RegIntrinsic::MapMapValues
            | RegIntrinsic::MapMerge
            | RegIntrinsic::MapNew
            | RegIntrinsic::MapTryFold
            | RegIntrinsic::MapValues => {
                self.exec_map_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::OptionAndThen
            | RegIntrinsic::OptionFilter
            | RegIntrinsic::OptionIsNone
            | RegIntrinsic::OptionIsSome
            | RegIntrinsic::OptionMap
            | RegIntrinsic::OptionOkOr
            | RegIntrinsic::OptionOr
            | RegIntrinsic::OptionUnwrapOr
            | RegIntrinsic::OptionUnwrapOrElse => {
                self.exec_option_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::CloneClone => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(deep_copy_value(value))
            }
            RegIntrinsic::OrdCompare => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                let value = match vm_value_cmp(left, right)? {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::OsClose => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::PatchApplyText => {
                let original = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let patch = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    patch_apply_text_string(original, patch)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::SetContains
            | RegIntrinsic::SetDifference
            | RegIntrinsic::SetIntersection
            | RegIntrinsic::SetIsEmpty
            | RegIntrinsic::SetIsSubset
            | RegIntrinsic::SetLen
            | RegIntrinsic::SetNew
            | RegIntrinsic::SetToList
            | RegIntrinsic::SetUnion
            | RegIntrinsic::SortedSetContains
            | RegIntrinsic::SortedSetIsEmpty
            | RegIntrinsic::SortedSetLen
            | RegIntrinsic::SortedSetNew
            | RegIntrinsic::SortedSetToList
            | RegIntrinsic::SortedMapContainsKey
            | RegIntrinsic::SortedMapGet
            | RegIntrinsic::SortedMapIsEmpty
            | RegIntrinsic::SortedMapKeys
            | RegIntrinsic::SortedMapLen
            | RegIntrinsic::SortedMapNew
            | RegIntrinsic::SortedMapValues => {
                self.exec_set_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::PathExtension
            | RegIntrinsic::PathFileName
            | RegIntrinsic::PathFromString
            | RegIntrinsic::PathToString
            | RegIntrinsic::PathIsAbsolute
            | RegIntrinsic::PathJoin
            | RegIntrinsic::PathNormalize
            | RegIntrinsic::PathParent
            | RegIntrinsic::PathResolveRelative
            | RegIntrinsic::PathSafeRelative
            | RegIntrinsic::PathStartsWith
            | RegIntrinsic::PathWithExtension => {
                self.exec_path_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::PersistentMapClear => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapContainsKey => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_map_get(&entries, key).is_some()))
            }
            RegIntrinsic::PersistentMapGet => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get(&entries, key)
                    .map(|value| VmValue::some(value))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PersistentMapInsert => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let value = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                sorted_map_insert(&mut entries, key, value);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::PersistentMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::PersistentMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::PersistentMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapRemove => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                sorted_map_remove(&mut entries, key);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::RegexCaptures
            | RegIntrinsic::RegexCompile
            | RegIntrinsic::RegexErrorMessage
            | RegIntrinsic::RegexFind
            | RegIntrinsic::RegexIsMatch
            | RegIntrinsic::RegexReplaceAll
            | RegIntrinsic::RegexSplit => {
                self.exec_regex_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::ReceiverClose => {
                let receiver_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Receiver.close missing receiver.".to_string())
                })?;
                let receiver = expect_receiver_ref(self.reg(base + receiver_reg))?;
                self.channel_state_mut(receiver.channel_id)?.receiver_closed = true;
                self.set_reg(
                    base + receiver_reg,
                    receiver_value(receiver.channel_id, true),
                );
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ReceiverIntoStream => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(stream_collect_error_value("channel receiver closed"));
                }
                Ok(stream_channel_value(receiver.channel_id))
            }
            RegIntrinsic::ReceiverRecv => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                if !self.channel_ready(receiver.channel_id) {
                    // Empty open channel: park until a sender enqueues or closes.
                    self.suspension = Some(Suspension {
                        wait: Wait::Recv {
                            channel: receiver.channel_id,
                        },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::ReceiverRecvCancellable => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel recv cancelled")));
                }
                if !self.channel_ready(receiver.channel_id) {
                    self.suspension = Some(Suspension {
                        wait: Wait::Recv {
                            channel: receiver.channel_id,
                        },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::RowBufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(row_buffer_value(Vec::with_capacity(size.max(0) as usize)))
            }
            RegIntrinsic::RowFieldString => {
                let row = intrinsic_arg(&self.stack, base, args, 0)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(row_field_string_value(
                    expect_row_fields_ref(row)?,
                    index,
                )))
            }
            RegIntrinsic::ResultErr
            | RegIntrinsic::ResultErrMessage
            | RegIntrinsic::ResultIsErr
            | RegIntrinsic::ResultIsOk
            | RegIntrinsic::ResultOk
            | RegIntrinsic::ResultAndThen
            | RegIntrinsic::ResultMap
            | RegIntrinsic::ResultMapError
            | RegIntrinsic::ResultUnwrapOr
            | RegIntrinsic::ResultUnwrapOrElse => {
                self.exec_result_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::StringAfter
            | RegIntrinsic::StringBefore
            | RegIntrinsic::StringBuilderNew
            | RegIntrinsic::StringCharAt
            | RegIntrinsic::StringChars
            | RegIntrinsic::StringContains
            | RegIntrinsic::StringCount
            | RegIntrinsic::StringCopy
            | RegIntrinsic::StringEndsWith
            | RegIntrinsic::StringFormat
            | RegIntrinsic::StringFromBool
            | RegIntrinsic::StringFromFloat
            | RegIntrinsic::StringFromInt
            | RegIntrinsic::StringIndexOf
            | RegIntrinsic::StringIsEmpty
            | RegIntrinsic::StringJoin
            | RegIntrinsic::StringLines
            | RegIntrinsic::StringLen
            | RegIntrinsic::StringPadLeft
            | RegIntrinsic::StringPadRight
            | RegIntrinsic::StringParseFloat
            | RegIntrinsic::StringParseInt
            | RegIntrinsic::StringRepeat
            | RegIntrinsic::StringReplace
            | RegIntrinsic::StringReplaceFirst
            | RegIntrinsic::StringReverse
            | RegIntrinsic::StringSlice
            | RegIntrinsic::StringSplit
            | RegIntrinsic::StringStartsWith
            | RegIntrinsic::StringStripPrefix
            | RegIntrinsic::StringToLowercase
            | RegIntrinsic::StringToUppercase
            | RegIntrinsic::StringTrim
            | RegIntrinsic::StringTrimEnd
            | RegIntrinsic::StringTrimStart => {
                self.exec_string_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::StreamCollectList => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    let state = self.channel_state_mut(channel_id)?;
                    let values = state.queue.drain(..).collect::<Vec<_>>();
                    if state.senders == 0 {
                        return Ok(value_ok(VmValue::List(Rc::new(RefCell::new(
                            TypedVec::from_values(values),
                        )))));
                    }
                    return Ok(value_err(channel_error_value(
                        "stream collect_list would block on an open channel stream",
                    )));
                }
                let values = stream.items.borrow().to_vec();
                stream.items.borrow_mut().clear();
                Ok(value_ok(VmValue::List(Rc::new(RefCell::new(
                    TypedVec::from_values(values),
                )))))
            }
            RegIntrinsic::StreamFromList => {
                let items = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?
                    .borrow()
                    .to_vec();
                Ok(stream_value(items))
            }
            RegIntrinsic::StreamNext => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    return Ok(json_result(self.channel_recv(channel_id)));
                }
                let value = if stream.items.borrow().is_empty() {
                    VmValue::OptionNone
                } else {
                    VmValue::some(stream.items.borrow_mut().remove(0))
                };
                Ok(value_ok(value))
            }
            RegIntrinsic::SenderClose => {
                let sender_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Sender.close missing sender.".to_string())
                })?;
                let sender = expect_sender_ref(self.reg(base + sender_reg))?;
                if !sender.closed {
                    let state = self.channel_state_mut(sender.channel_id)?;
                    state.senders = state.senders.saturating_sub(1);
                }
                self.set_reg(base + sender_reg, sender_value(sender.channel_id, true));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::SenderSend => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                if self.channel_send_would_block(&sender) {
                    // Full bounded channel: park until a receiver frees space.
                    self.suspension = Some(Suspension {
                        wait: Wait::Send { sender, value },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::SenderSendCancellable => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 2)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel send cancelled")));
                }
                if self.channel_send_would_block(&sender) {
                    self.suspension = Some(Suspension {
                        wait: Wait::Send { sender, value },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::TimerSleep => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.park_sleep_ms(ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::TimerSleepCancellable => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                self.park_sleep_ms(ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::TimerSleepUntil => {
                let target_unix_ms =
                    expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                self.park_sleep_ms(target_unix_ms - now_unix_ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::UrlDecodeComponent
            | RegIntrinsic::UrlEncodeComponent
            | RegIntrinsic::UrlFromString
            | RegIntrinsic::UrlToString => {
                self.exec_url_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::YamlParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(yaml_parse_json_value(text)))
            }
            RegIntrinsic::WeakDowngrade | RegIntrinsic::WeakFrom => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::WeakUpgrade => Ok(VmValue::some(
                intrinsic_arg(&self.stack, base, args, 0)?.clone(),
            )),
        }
    }
}
