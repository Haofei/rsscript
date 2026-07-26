use super::super::*;
use crate::reg_vm::runtime_values::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_json_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::JsonParseOk
            | RegIntrinsic::JsonFieldOk
            | RegIntrinsic::JsonFieldIntOk => Err(EvalError::Runtime(format!(
                "internal native JSON intrinsic {intrinsic:?} reached the interpreter"
            ))),
            RegIntrinsic::JsonArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("[{}]", items.join(","))))
            }
            RegIntrinsic::JsonArrayBools => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_bools_value(value)))
            }
            RegIntrinsic::JsonArrayContainsPrefix => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    prefix,
                    JsonArrayStringMatch::Prefix,
                )))
            }
            RegIntrinsic::JsonArrayContainsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let item = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    item,
                    JsonArrayStringMatch::Exact,
                )))
            }
            RegIntrinsic::JsonArrayContainsSubstring => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    text,
                    JsonArrayStringMatch::Substring,
                )))
            }
            RegIntrinsic::JsonArrayCountWhere => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                let mut count = 0_i64;
                for item in items {
                    let result = self.call_closure_one(
                        unit,
                        &predicate,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => {
                            if expect_bool_ref(&value)? {
                                count += 1;
                            }
                        }
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(VmValue::Int(count)))
            }
            RegIntrinsic::JsonArrayFold => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                for item in items {
                    let result = self.call_closure_two(
                        unit,
                        &folder,
                        state,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::JsonArrayGet => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_get_value(value, index)))
            }
            RegIntrinsic::JsonArrayInts => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_ints_value(value)))
            }
            RegIntrinsic::JsonArrayLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_array()
                    .map(|items| VmValue::Int(items.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an array"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonArrayStrings => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_strings_value(value)))
            }
            RegIntrinsic::JsonAt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::Json(Rc::new(value))),
                ))
            }
            RegIntrinsic::JsonAtBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonAtBoolOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonAtInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonAtIntOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonAtOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_path_value(value, path)))
            }
            RegIntrinsic::JsonAtOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_bool_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_int_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_string_value,
                )))
            }
            RegIntrinsic::JsonAtOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_json_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Json(Rc::new(
                    json_value_at(value, path).unwrap_or_else(|_| fallback.clone()),
                )))
            }
            RegIntrinsic::JsonAtString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonAtStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAtToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonAtToStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAsBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_bool().map(VmValue::Bool).ok_or_else(
                    || json_error_value("JSON value is not a boolean"),
                )))
            }
            RegIntrinsic::JsonAsInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_i64().map(VmValue::Int).ok_or_else(
                    || json_error_value("JSON value is not an integer"),
                )))
            }
            RegIntrinsic::JsonAsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_str().map(VmValue::string).ok_or_else(
                    || json_error_value("JSON value is not a string"),
                )))
            }
            RegIntrinsic::JsonBoolAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonBoolAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonBoolField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonClone => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value.clone())))
            }
            RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => Err(EvalError::Runtime(
                "reg VM Json.decode requires typed intrinsic metadata.".to_string(),
            )),
            RegIntrinsic::JsonEncode => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::JsonField => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonFieldString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonIntAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonIntAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonIsArray => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_array()))
            }
            RegIntrinsic::JsonIsNull => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_null()))
            }
            RegIntrinsic::JsonIsObject => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_object()))
            }
            RegIntrinsic::JsonIntField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonKind => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_kind(value)))
            }
            RegIntrinsic::JsonObject => {
                let fields = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("{{{}}}", fields.join(","))))
            }
            RegIntrinsic::JsonObjectKeys => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| {
                        let mut keys = fields.keys().map(VmValue::string).collect::<Vec<_>>();
                        keys.sort_by_key(VmValue::display);
                        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(keys))))
                    })
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonObjectLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| VmValue::Int(fields.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    serde_json::from_str::<serde_json::Value>(text)
                        .map(|value| VmValue::Json(Rc::new(value)))
                        .map_err(|error| json_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::JsonParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_parse_file_value(path)))
            }
            RegIntrinsic::JsonQuoteString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_quote_string(value)?))
            }
            RegIntrinsic::JsonRawField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonStringArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let quoted = items
                    .iter()
                    .map(|item| json_quote_string(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::string(format!("[{}]", quoted.join(","))))
            }
            RegIntrinsic::JsonStringField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    json_quote_string(value)?
                )))
            }
            RegIntrinsic::JsonStrings => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(
                    items.into_iter().map(serde_json::Value::String).collect(),
                ))))
            }
            RegIntrinsic::JsonToStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonToStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonValue => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value)))
            }
            RegIntrinsic::JsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(&value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            other => unreachable!("exec_json_intrinsics called with non-json intrinsic: {other:?}"),
        }
    }
}

fn json_parse_file_value(path: &str) -> Result<VmValue, VmValue> {
    rsscript_runtime::file_read_string(path)
        .map_err(|error| json_error_value(error.to_string()))
        .and_then(|text| {
            serde_json::from_str::<serde_json::Value>(&text)
                .map(|value| VmValue::Json(Rc::new(value)))
                .map_err(|error| json_error_value(error.to_string()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_rejects_oversized_input_before_json_parsing() {
        let path = std::env::temp_dir().join(format!(
            "rsscript-vm-json-parse-limit-{}",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).expect("test file should be created");
        file.set_len(rsscript_runtime::RUNTIME_READ_CEILING_BYTES as u64 + 1)
            .expect("sparse test file should be sized");

        assert!(json_parse_file_value(&path.to_string_lossy()).is_err());

        let _ = std::fs::remove_file(path);
    }
}
