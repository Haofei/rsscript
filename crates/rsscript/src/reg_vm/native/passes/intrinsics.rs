/// Static type of a register in the native-JIT subset: every register is an
/// unboxed `i64` holding either an `Int` or a `Bool` (`0`/`1`).
#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::reg_vm) enum NativeTy {
    Int,
    Bool,
    Float,
    /// An opaque handle to a heap value (struct/list) passed as a parameter, used
    /// only as the base of a heap-read instruction. Stored as `i64` (a table index).
    Handle,
    /// TV2: a flat `List<Int>` param read directly in-register (no per-element host
    /// call). Marshalled as a raw `*const i64` + element count; only assigned when
    /// every use of the param is a list read with a consistent `Int` element kind
    /// (`flatten_list_params`). Marshalling falls back if the runtime list isn't the
    /// flat `Ints` kind (TV1 is non-canonical).
    FlatInt,
    /// TV2: a mutable flat `List<Int>` param passed as raw `*mut i64` + element
    /// count. Native direct writes are protected by the VM-side heap transaction
    /// snapshot/rollback path.
    FlatIntMut,
    /// TV2: a flat `List<Float>` param read directly in-register. Marshalled as a raw
    /// `*const f64` + element count; falls back if the runtime list isn't `Floats`.
    FlatFloat,
    /// TV2: a mutable flat `List<Float>` param passed as raw `*mut f64` + element
    /// count (write-side counterpart of [`FlatFloat`]). Native direct writes are
    /// protected by the VM-side heap transaction snapshot/rollback path.
    FlatFloatMut,
}

#[cfg(feature = "native-jit")]
impl NativeTy {
    pub(in crate::reg_vm) fn jit_value_type(self) -> vm_jit::JitValueType {
        match self {
            NativeTy::Int => vm_jit::JitValueType::Int,
            NativeTy::Bool => vm_jit::JitValueType::Bool,
            NativeTy::Float => vm_jit::JitValueType::Float,
            NativeTy::Handle => vm_jit::JitValueType::Handle,
            NativeTy::FlatInt => vm_jit::JitValueType::FlatInt,
            NativeTy::FlatIntMut => vm_jit::JitValueType::FlatIntMut,
            NativeTy::FlatFloat => vm_jit::JitValueType::FlatFloat,
            NativeTy::FlatFloatMut => vm_jit::JitValueType::FlatFloatMut,
        }
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
pub(in crate::reg_vm) struct NativeHostIntrinsic {
    pub(in crate::reg_vm) helper: vm_jit::HostHelper,
    pub(in crate::reg_vm) result_ty: NativeTy,
}

#[cfg(feature = "native-jit")]
impl NativeHostIntrinsic {
    pub(in crate::reg_vm) fn arg_tys(self) -> Vec<NativeTy> {
        self.helper
            .arg_types()
            .iter()
            .map(|ty| match ty {
                vm_jit::JitValueType::Int => NativeTy::Int,
                vm_jit::JitValueType::Bool => NativeTy::Bool,
                vm_jit::JitValueType::Float => NativeTy::Float,
                vm_jit::JitValueType::Handle => NativeTy::Handle,
                vm_jit::JitValueType::FlatInt => NativeTy::FlatInt,
                vm_jit::JitValueType::FlatIntMut => NativeTy::FlatIntMut,
                vm_jit::JitValueType::FlatFloat => NativeTy::FlatFloat,
                vm_jit::JitValueType::FlatFloatMut => NativeTy::FlatFloatMut,
            })
            .collect()
    }

    pub(in crate::reg_vm) fn produces_output_handle(self) -> bool {
        self.helper.heap_effect().produces_heap_result()
    }

    pub(in crate::reg_vm) fn consumes_output_handles(self) -> bool {
        self.helper
            .arg_types()
            .iter()
            .any(|ty| *ty == vm_jit::JitValueType::Handle)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_host_intrinsic(
    intrinsic: RegIntrinsic,
) -> Option<NativeHostIntrinsic> {
    native_host_typed_intrinsic(intrinsic, None)
}

/// Intrinsics lowered to an *inline* Cranelift numeric conversion rather than a
/// host-helper call: `Int.to_float` (signed-int→f64) and the Float→Int rounding
/// casts `Math.floor`/`Math.ceil` (round, then saturating f64→i64). Returns the
/// expected argument count. `Math.round` is intentionally absent — its
/// round-half-away-from-zero semantics have no exact single Cranelift form, so it
/// stays interpreter-only. Centralising the list here keeps the native subset gate,
/// type inference, and lowering in agreement from one source of truth.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_inline_convert_intrinsic(intrinsic: RegIntrinsic) -> Option<usize> {
    match intrinsic {
        RegIntrinsic::IntToFloat | RegIntrinsic::MathFloor | RegIntrinsic::MathCeil => Some(1),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_host_typed_intrinsic(
    intrinsic: RegIntrinsic,
    type_arg: Option<&str>,
) -> Option<NativeHostIntrinsic> {
    match intrinsic {
        RegIntrinsic::ListNew if type_arg == Some("Int") => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::ListNewInt,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::StringFromInt => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringFromInt,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::StringLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::StringSlice => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringSlice,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::StringPadLeft => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringPadLeft,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::StringSplit => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringSplit,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::StringStartsWith => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::StringStartsWith,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::ListIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::ListIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::JsonParseOk => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::JsonParse,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::JsonFieldOk => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::JsonField,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::JsonFieldIntOk => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::JsonFieldInt,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::BytesLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::BytesLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::BytesSlice => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::BytesSlice,
            result_ty: NativeTy::Handle,
        }),
        RegIntrinsic::SetContains => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::MapContainsInt,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::MapIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::MapIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::MapLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::MapLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::SetIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SetIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::SetLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SetLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::SortedSetContains => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SortedSetContainsInt,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::SortedSetIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SortedSetIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::SortedSetLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::ListLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::SortedMapContainsKey => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SortedMapContainsKeyInt,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::SortedMapIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SortedMapIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        RegIntrinsic::SortedMapLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::SortedMapLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::DequeLen => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::DequeLen,
            result_ty: NativeTy::Int,
        }),
        RegIntrinsic::DequeIsEmpty => Some(NativeHostIntrinsic {
            helper: vm_jit::HostHelper::DequeIsEmpty,
            result_ty: NativeTy::Bool,
        }),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_string_concat_host() -> NativeHostIntrinsic {
    NativeHostIntrinsic {
        helper: vm_jit::HostHelper::StringConcat,
        result_ty: NativeTy::Handle,
    }
}

#[cfg(feature = "native-jit")]
fn native_checked_payload_intrinsic(intrinsic: RegIntrinsic) -> Option<RegIntrinsic> {
    match intrinsic {
        RegIntrinsic::JsonParse => Some(RegIntrinsic::JsonParseOk),
        RegIntrinsic::JsonField => Some(RegIntrinsic::JsonFieldOk),
        RegIntrinsic::JsonFieldInt => Some(RegIntrinsic::JsonFieldIntOk),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
struct NativeCheckedPayloadPair {
    call_ip: usize,
    payload_reg: usize,
    payload_intrinsic: RegIntrinsic,
}

#[cfg(feature = "native-jit")]
fn native_checked_payload_pairs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<Vec<NativeCheckedPayloadPair>> {
    if header >= exit || exit > code.len() {
        return None;
    }

    if !(header..exit).any(|i| {
        matches!(
            &code[i],
            RegInstr::CallIntrinsic { intrinsic, .. }
                if native_checked_payload_intrinsic(*intrinsic).is_some()
        )
    }) {
        return Some(Vec::new());
    }

    let mut read_counts = vec![0usize; n_regs];
    for instr in code {
        match instr_read_regs(instr) {
            RegFootprint::Some(reads) => {
                for reg in reads {
                    if reg < n_regs {
                        read_counts[reg] = read_counts[reg].saturating_add(1);
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }

    let mut pairs = Vec::new();
    for i in header..exit {
        let RegInstr::CallIntrinsic {
            dst: result_reg,
            intrinsic,
            ..
        } = &code[i]
        else {
            continue;
        };
        let Some(payload_intrinsic) = native_checked_payload_intrinsic(*intrinsic) else {
            continue;
        };
        let Some(RegInstr::TryResult {
            dst: payload_reg,
            src,
            ..
        }) = code.get(i + 1)
        else {
            continue;
        };
        if i + 1 >= exit || *src != *result_reg || read_counts[*result_reg] != 1 {
            continue;
        }

        pairs.push(NativeCheckedPayloadPair {
            call_ip: i,
            payload_reg: *payload_reg,
            payload_intrinsic,
        });
    }

    Some(pairs)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_checked_payload_rewrite_ips_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<Vec<bool>> {
    let pairs = native_checked_payload_pairs_in_region(code, n_regs, header, exit)?;
    let mut rewrite_ips = vec![false; code.len()];
    for pair in pairs {
        if pair.call_ip < rewrite_ips.len() {
            rewrite_ips[pair.call_ip] = true;
        }
        if pair.call_ip + 1 < rewrite_ips.len() {
            rewrite_ips[pair.call_ip + 1] = true;
        }
    }
    Some(rewrite_ips)
}

/// Rewrite `fallible_intrinsic(...) ?` into a checked native payload call.
///
/// The source intrinsic normally returns `Result<T, JsonError>`. When its result
/// register is consumed only by the immediately following `TryResult`, native code
/// can call a payload helper directly: success writes `T`, failure sets the native
/// bail flag. On bail the VM reruns the original bytecode from the region entry, so
/// cleanup and heap `Err` construction still happen in the interpreter exactly as
/// before. Unpaired fallible calls stay untouched and therefore remain outside the
/// native subset.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_lower_checked_payload_intrinsics_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }

    let pairs = native_checked_payload_pairs_in_region(code, n_regs, header, exit)?;
    if pairs.is_empty() {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    let mut out = code.to_vec();
    for pair in pairs {
        let RegInstr::CallIntrinsic { args, .. } = &code[pair.call_ip] else {
            continue;
        };
        out[pair.call_ip] = RegInstr::CallIntrinsic {
            dst: pair.payload_reg,
            intrinsic: pair.payload_intrinsic,
            args: args.clone(),
        };
        out[pair.call_ip + 1] = RegInstr::Move {
            dst: pair.payload_reg,
            src: pair.payload_reg,
        };
    }

    let ip_map: Vec<usize> = (0..code.len()).collect();
    Some((out, n_regs, ip_map))
}
