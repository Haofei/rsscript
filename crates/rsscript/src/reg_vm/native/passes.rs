//! Native-JIT analysis, eligibility, fold, scalar-replacement, inlining and
//! closure-sink passes. Pure code-movement out of `reg_vm::mod` (Phase 2);
//! every item retains its original `#[cfg(feature = "native-jit")]` attribute
//! verbatim (now redundant under the cfg-gated `native` module, but harmless).
#![allow(unused_imports)]

use super::super::*;
use super::*;

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
            // Booleans are stored as `i64` 0/1, like integers.
            NativeTy::Int | NativeTy::Bool => vm_jit::JitValueType::Int,
            NativeTy::Float => vm_jit::JitValueType::Float,
            NativeTy::Handle => vm_jit::JitValueType::Handle,
            NativeTy::FlatInt | NativeTy::FlatIntMut => vm_jit::JitValueType::FlatInt,
            NativeTy::FlatFloat | NativeTy::FlatFloatMut => vm_jit::JitValueType::FlatFloat,
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
                vm_jit::JitValueType::Float => NativeTy::Float,
                vm_jit::JitValueType::Handle => NativeTy::Handle,
                vm_jit::JitValueType::FlatInt => NativeTy::FlatInt,
                vm_jit::JitValueType::FlatFloat => NativeTy::FlatFloat,
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

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeFact<T: Copy + Eq> {
    Unreached,
    Unknown,
    Known(T),
}

#[cfg(feature = "native-jit")]
impl<T: Copy + Eq> NativeFact<T> {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (NativeFact::Unreached, value) | (value, NativeFact::Unreached) => value,
            (NativeFact::Unknown, NativeFact::Unknown) => NativeFact::Unknown,
            (NativeFact::Known(lhs), NativeFact::Known(rhs)) if lhs == rhs => {
                NativeFact::Known(lhs)
            }
            _ => NativeFact::Unknown,
        }
    }

    fn is_unreached(self) -> bool {
        matches!(self, NativeFact::Unreached)
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeControlFlow {
    Fallthrough,
    Jump(usize),
    Branch { target: usize },
    Split { first: usize, second: usize },
    Terminal,
}

#[cfg(feature = "native-jit")]
impl Default for NativeControlFlow {
    fn default() -> Self {
        Self::Fallthrough
    }
}

#[cfg(feature = "native-jit")]
impl NativeControlFlow {
    fn successors(self, ip: usize, code_len: usize, mut push: impl FnMut(usize)) {
        let fallthrough = ip + 1;
        match self {
            NativeControlFlow::Fallthrough => {
                if fallthrough < code_len {
                    push(fallthrough);
                }
            }
            NativeControlFlow::Jump(target) => push(target),
            NativeControlFlow::Branch { target } => {
                push(target);
                if fallthrough < code_len {
                    push(fallthrough);
                }
            }
            NativeControlFlow::Split { first, second } => {
                push(first);
                push(second);
            }
            NativeControlFlow::Terminal => {}
        }
    }

    fn is_boundary(self) -> bool {
        !matches!(self, NativeControlFlow::Fallthrough)
    }
}

/// A conservative register footprint: either an EXACT set of register operands, or
/// `All` meaning "could touch every register" (used for instruction variants we do
/// not fully model, so an omission is sound — it over-approximates liveness and can
/// only cause more OSR bails, never a missed escape).
#[cfg(feature = "native-jit")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum RegFootprint {
    Some(Vec<usize>),
    All,
}

#[cfg(feature = "native-jit")]
impl Default for RegFootprint {
    fn default() -> Self {
        Self::All
    }
}

#[cfg(feature = "native-jit")]
fn native_instr_successors(instr: &RegInstr, ip: usize, code_len: usize, push: impl FnMut(usize)) {
    native_instr_semantics(instr)
        .control
        .successors(ip, code_len, push);
}

#[cfg(feature = "native-jit")]
fn native_instr_is_control_boundary(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).control.is_boundary()
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct NativeRegionCfg {
    entry: usize,
    exit: usize,
    successors: Vec<Vec<usize>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionCfg {
    pub(in crate::reg_vm) fn new(code: &[RegInstr], entry: usize, exit: usize) -> Option<Self> {
        if entry >= exit || exit > code.len() {
            return None;
        }
        let mut successors = vec![Vec::new(); exit - entry];
        for ip in entry..exit {
            native_instr_successors(&code[ip], ip, code.len(), |target| {
                if target >= entry && target < exit {
                    successors[ip - entry].push(target);
                }
            });
            successors[ip - entry].sort_unstable();
            successors[ip - entry].dedup();
        }
        Some(Self {
            entry,
            exit,
            successors,
        })
    }

    pub(in crate::reg_vm) fn prefix(code: &[RegInstr], exit: usize) -> Option<Self> {
        Self::new(code, 0, exit)
    }

    fn contains(&self, ip: usize) -> bool {
        ip >= self.entry && ip < self.exit
    }

    fn slot(&self, ip: usize) -> Option<usize> {
        self.contains(ip).then_some(ip - self.entry)
    }

    pub(in crate::reg_vm) fn successors(&self, ip: usize) -> Option<&[usize]> {
        self.slot(ip)
            .and_then(|slot| self.successors.get(slot).map(Vec::as_slice))
    }

    pub(in crate::reg_vm) fn backedges_to(&self, header: usize) -> Vec<usize> {
        if !self.contains(header) {
            return Vec::new();
        }
        let mut backedges = Vec::new();
        for ip in self.entry..self.exit {
            let Some(successors) = self.successors(ip) else {
                continue;
            };
            if ip >= header && successors.iter().any(|&target| target == header) {
                backedges.push(ip);
            }
        }
        backedges
    }

    pub(in crate::reg_vm) fn backedges(&self) -> Vec<(usize, usize)> {
        let mut backedges = Vec::new();
        for ip in self.entry..self.exit {
            let Some(successors) = self.successors(ip) else {
                continue;
            };
            for &target in successors {
                if target <= ip {
                    backedges.push((ip, target));
                }
            }
        }
        backedges
    }

    fn reachable_ips(&self) -> Vec<usize> {
        let mut reachable = vec![false; self.exit - self.entry];
        let mut stack = vec![self.entry];
        let mut out = Vec::new();
        while let Some(ip) = stack.pop() {
            let Some(slot) = self.slot(ip) else {
                continue;
            };
            if reachable[slot] {
                continue;
            }
            reachable[slot] = true;
            out.push(ip);
            for &successor in &self.successors[slot] {
                stack.push(successor);
            }
        }
        out
    }

    fn reachable_mask(&self) -> Vec<bool> {
        let mut reachable = vec![false; self.exit - self.entry];
        for ip in self.reachable_ips() {
            if let Some(slot) = self.slot(ip) {
                reachable[slot] = true;
            }
        }
        reachable
    }
}

#[cfg(feature = "native-jit")]
fn native_clear_defined_fact<T: Copy + Eq>(instr: &RegInstr, facts: &mut [NativeFact<T>]) {
    if let Some(dst) = native_subset_dst(instr) {
        if let Some(fact) = facts.get_mut(dst) {
            *fact = NativeFact::Unknown;
        }
    } else if let RegInstr::CallIntrinsic { dst, .. } | RegInstr::CallTypedIntrinsic { dst, .. } =
        instr
        && let Some(fact) = facts.get_mut(*dst)
    {
        *fact = NativeFact::Unknown;
    }
}

#[cfg(feature = "native-jit")]
fn native_forward_reg_facts<T: Copy + Eq>(
    cfg: &NativeRegionCfg,
    code: &[RegInstr],
    n_regs: usize,
    mut transfer: impl FnMut(usize, &RegInstr, &mut [NativeFact<T>]),
) -> Vec<Vec<NativeFact<T>>> {
    let mut input = vec![vec![NativeFact::Unreached; n_regs]; cfg.exit - cfg.entry];
    input[0] = vec![NativeFact::Unknown; n_regs];

    let mut changed = true;
    while changed {
        changed = false;
        for ip in cfg.entry..cfg.exit {
            let slot = ip - cfg.entry;
            if input[slot].iter().all(|fact| fact.is_unreached()) {
                continue;
            }

            let mut out = input[slot].clone();
            transfer(ip, &code[ip], &mut out);

            for successor in &cfg.successors[slot] {
                let successor_slot = successor - cfg.entry;
                let mut merged = input[successor_slot].clone();
                let mut merged_changed = false;
                for (dst, incoming) in merged.iter_mut().zip(out.iter().copied()) {
                    let next = dst.merge(incoming);
                    if *dst != next {
                        *dst = next;
                        merged_changed = true;
                    }
                }
                if merged_changed {
                    input[successor_slot] = merged;
                    changed = true;
                }
            }
        }
    }

    input
}

#[cfg(feature = "native-jit")]
fn native_forward_definite_regs(
    cfg: &NativeRegionCfg,
    code: &[RegInstr],
    n_regs: usize,
    mut transfer: impl FnMut(usize, &RegInstr, &mut [bool]) -> Option<()>,
) -> Option<Vec<Option<Vec<bool>>>> {
    let mut input = vec![None; cfg.exit - cfg.entry];
    input[0] = Some(vec![false; n_regs]);
    let mut worklist = vec![cfg.entry];

    while let Some(ip) = worklist.pop() {
        let Some(mut assigned) = input[ip - cfg.entry].clone() else {
            continue;
        };
        transfer(ip, &code[ip], &mut assigned)?;

        let Some(successors) = cfg.successors(ip) else {
            continue;
        };
        for &successor in successors {
            let successor_slot = successor - cfg.entry;
            let changed = match &mut input[successor_slot] {
                Some(existing) => {
                    let mut changed = false;
                    for (dst, src) in existing.iter_mut().zip(assigned.iter().copied()) {
                        let merged = *dst && src;
                        if *dst != merged {
                            *dst = merged;
                            changed = true;
                        }
                    }
                    changed
                }
                slot @ None => {
                    *slot = Some(assigned.clone());
                    true
                }
            };
            if changed {
                worklist.push(successor);
            }
        }
    }

    Some(input)
}

#[cfg(feature = "native-jit")]
struct NativeRegionLiveness {
    cfg: NativeRegionCfg,
    live_in: Vec<Vec<bool>>,
    live_out: Vec<Vec<bool>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionLiveness {
    fn compute_with_cfg(code: &[RegInstr], n_regs: usize, cfg: NativeRegionCfg) -> Self {
        let len = cfg.exit - cfg.entry;
        let mut live_in = vec![vec![false; n_regs]; len];
        let mut live_out = vec![vec![false; n_regs]; len];

        let mut changed = true;
        while changed {
            changed = false;
            for ip in (cfg.entry..cfg.exit).rev() {
                let slot = ip - cfg.entry;

                let mut next_out = vec![false; n_regs];
                for &successor in &cfg.successors[slot] {
                    let successor_slot = successor - cfg.entry;
                    for (dst, src) in next_out
                        .iter_mut()
                        .zip(live_in[successor_slot].iter().copied())
                    {
                        *dst |= src;
                    }
                }

                let mut next_in = next_out.clone();
                native_liveness_transfer(&code[ip], n_regs, &mut next_in);

                if live_out[slot] != next_out {
                    live_out[slot] = next_out;
                    changed = true;
                }
                if live_in[slot] != next_in {
                    live_in[slot] = next_in;
                    changed = true;
                }
            }
        }

        Self {
            cfg,
            live_in,
            live_out,
        }
    }

    fn live_in(&self, ip: usize, reg: usize) -> Option<bool> {
        let slot = self.cfg.slot(ip)?;
        self.live_in.get(slot)?.get(reg).copied()
    }

    fn live_out(&self, ip: usize, reg: usize) -> Option<bool> {
        let slot = self.cfg.slot(ip)?;
        self.live_out.get(slot)?.get(reg).copied()
    }
}

#[cfg(feature = "native-jit")]
fn native_liveness_transfer(instr: &RegInstr, n_regs: usize, live: &mut [bool]) {
    match instr_written_reg(instr) {
        RegFootprint::Some(writes) => {
            for reg in writes {
                if reg < n_regs {
                    live[reg] = false;
                }
            }
        }
        RegFootprint::All => live.fill(false),
    }

    match instr_read_regs(instr) {
        RegFootprint::Some(reads) => {
            for reg in reads {
                if reg < n_regs {
                    live[reg] = true;
                }
            }
        }
        RegFootprint::All => live.fill(true),
    }
}

#[cfg(feature = "native-jit")]
fn native_footprint_touches_mask(
    footprint: RegFootprint,
    n_regs: usize,
    mask: &[bool],
) -> Option<bool> {
    match footprint {
        RegFootprint::Some(regs) => Some(
            regs.into_iter()
                .any(|reg| reg < n_regs && mask.get(reg).copied().unwrap_or(false)),
        ),
        RegFootprint::All => None,
    }
}

#[cfg(feature = "native-jit")]
fn native_region_external_footprint_touches(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    mask: &[bool],
    mut footprint: impl FnMut(&RegInstr) -> RegFootprint,
) -> Option<bool> {
    for (ip, instr) in code.iter().enumerate() {
        if ip >= header && ip < exit {
            continue;
        }
        if native_footprint_touches_mask(footprint(instr), n_regs, mask)? {
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(feature = "native-jit")]
fn native_region_external_reads_touch(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    mask: &[bool],
) -> Option<bool> {
    native_region_external_footprint_touches(code, n_regs, header, exit, mask, instr_read_regs)
}

#[cfg(feature = "native-jit")]
fn native_region_external_writes_touch(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    mask: &[bool],
) -> Option<bool> {
    native_region_external_footprint_touches(code, n_regs, header, exit, mask, instr_written_reg)
}

#[cfg(feature = "native-jit")]
fn native_region_external_reads_or_writes_touch(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    mask: &[bool],
) -> Option<bool> {
    Some(
        native_region_external_reads_touch(code, n_regs, header, exit, mask)?
            || native_region_external_writes_touch(code, n_regs, header, exit, mask)?,
    )
}

#[cfg(feature = "native-jit")]
fn native_global_def_counts(code: &[RegInstr], n_regs: usize) -> Option<Vec<usize>> {
    let mut counts = vec![0usize; n_regs];
    for instr in code {
        match instr_written_reg(instr) {
            RegFootprint::Some(writes) => {
                for reg in writes {
                    if reg < n_regs {
                        counts[reg] += 1;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    Some(counts)
}

#[cfg(feature = "native-jit")]
fn native_region_def_counts(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<Vec<usize>> {
    let mut counts = vec![0usize; n_regs];
    for instr in &code[header..exit] {
        match instr_written_reg(instr) {
            RegFootprint::Some(writes) => {
                for reg in writes {
                    if reg < n_regs {
                        counts[reg] += 1;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    Some(counts)
}

#[cfg(feature = "native-jit")]
struct NativeRegionValueFacts {
    cfg: NativeRegionCfg,
    const_int: Vec<Vec<NativeFact<i64>>>,
    list_len_source: Vec<Vec<NativeFact<(usize, usize)>>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionValueFacts {
    fn compute_with_cfg(code: &[RegInstr], n_regs: usize, cfg: NativeRegionCfg) -> Self {
        let const_int =
            native_forward_reg_facts(&cfg, code, n_regs, |_ip, instr, values| match instr {
                RegInstr::LoadInt { dst, value } if *dst < values.len() => {
                    values[*dst] = NativeFact::Known(*value);
                }
                RegInstr::Move { dst, src } if *dst < values.len() && *src < values.len() => {
                    values[*dst] = values[*src];
                }
                _ => native_clear_defined_fact(instr, values),
            });

        let list_len_source =
            native_forward_reg_facts(&cfg, code, n_regs, |ip, instr, sources| match instr {
                RegInstr::ListLen { dst, list } if *dst < sources.len() => {
                    sources[*dst] = NativeFact::Known((*list, ip));
                }
                RegInstr::Move { dst, src } if *dst < sources.len() && *src < sources.len() => {
                    sources[*dst] = sources[*src];
                }
                _ => native_clear_defined_fact(instr, sources),
            });

        Self {
            cfg,
            const_int,
            list_len_source,
        }
    }

    fn const_int_before(&self, reg: usize, ip: usize) -> Option<i64> {
        let slot = self.cfg.slot(ip)?;
        match *self.const_int.get(slot)?.get(reg)? {
            NativeFact::Known(value) => Some(value),
            NativeFact::Unreached | NativeFact::Unknown => None,
        }
    }

    fn list_len_source_before(&self, reg: usize, ip: usize) -> Option<(usize, usize)> {
        let slot = self.cfg.slot(ip)?;
        match *self.list_len_source.get(slot)?.get(reg)? {
            NativeFact::Known(value) => Some(value),
            NativeFact::Unreached | NativeFact::Unknown => None,
        }
    }
}

#[cfg(feature = "native-jit")]
struct NativeRegionEffects {
    roots_before: Vec<Vec<usize>>,
    list_writes: Vec<(usize, usize)>,
    move_alias_roots: Vec<usize>,
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct NativeRegionAnalysis {
    n_regs: usize,
    header: usize,
    exit: usize,
    values: NativeRegionValueFacts,
    liveness: NativeRegionLiveness,
    effects: NativeRegionEffects,
    global_def_counts: Option<Vec<usize>>,
    region_def_counts: Option<Vec<usize>>,
}

#[cfg(feature = "native-jit")]
#[derive(Default)]
pub(in crate::reg_vm) struct NativeProfileGuidance {
    pub(in crate::reg_vm) cold_blocks: Vec<u32>,
    pub(in crate::reg_vm) hot_branch_edges: std::collections::HashMap<usize, bool>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionAnalysis {
    pub(in crate::reg_vm) fn compute_prefix(
        code: &[RegInstr],
        n_regs: usize,
        header: usize,
        exit: usize,
    ) -> Option<Self> {
        if header >= exit || exit > code.len() {
            return None;
        }
        let cfg = NativeRegionCfg::prefix(code, exit)?;
        Some(Self {
            n_regs,
            header,
            exit,
            liveness: NativeRegionLiveness::compute_with_cfg(code, n_regs, cfg.clone()),
            values: NativeRegionValueFacts::compute_with_cfg(code, n_regs, cfg),
            effects: NativeRegionEffects::compute(code, n_regs, exit),
            global_def_counts: native_global_def_counts(code, n_regs),
            region_def_counts: native_region_def_counts(code, n_regs, header, exit),
        })
    }

    pub(in crate::reg_vm) fn compute_region(
        code: &[RegInstr],
        n_regs: usize,
        header: usize,
        exit: usize,
    ) -> Option<Self> {
        if header >= exit || exit > code.len() {
            return None;
        }
        let cfg = NativeRegionCfg::new(code, header, exit)?;
        Some(Self {
            n_regs,
            header,
            exit,
            liveness: NativeRegionLiveness::compute_with_cfg(code, n_regs, cfg.clone()),
            values: NativeRegionValueFacts::compute_with_cfg(code, n_regs, cfg),
            effects: NativeRegionEffects::compute(code, n_regs, exit),
            global_def_counts: native_global_def_counts(code, n_regs),
            region_def_counts: native_region_def_counts(code, n_regs, header, exit),
        })
    }

    fn const_int_before(&self, reg: usize, ip: usize) -> Option<i64> {
        self.values.const_int_before(reg, ip)
    }

    fn list_len_source_before(&self, reg: usize, ip: usize) -> Option<(usize, usize)> {
        self.values.list_len_source_before(reg, ip)
    }

    pub(in crate::reg_vm) fn live_in(&self, ip: usize, reg: usize) -> Option<bool> {
        self.liveness.live_in(ip, reg)
    }

    pub(in crate::reg_vm) fn live_out(&self, ip: usize, reg: usize) -> Option<bool> {
        self.liveness.live_out(ip, reg)
    }

    pub(in crate::reg_vm) fn profile_guidance(
        &self,
        code: &[RegInstr],
        profile: &FunctionProfile,
        ip_map: &[usize],
    ) -> NativeProfileGuidance {
        let mut cold_blocks = std::collections::BTreeSet::new();
        let mut hot_branch_edges = std::collections::HashMap::new();
        for ip in self.liveness.cfg.reachable_ips() {
            let Some(&original_ip) = ip_map.get(ip) else {
                continue;
            };
            let Some(hot_target) = profile
                .branch_feedback(original_ip)
                .and_then(BranchFeedback::hot_edge)
            else {
                continue;
            };
            let (target, hot_ip) = match code.get(ip) {
                Some(RegInstr::JumpIfBool { target, .. })
                | Some(RegInstr::JumpIfIntCompare { target, .. }) => {
                    if hot_target {
                        (*target, *target)
                    } else {
                        (*target, ip + 1)
                    }
                }
                _ => continue,
            };
            let cold_ip = if hot_target { ip + 1 } else { target };
            let Some(successors) = self.liveness.cfg.successors(ip) else {
                continue;
            };
            if successors.contains(&hot_ip) && successors.contains(&cold_ip) {
                hot_branch_edges.insert(ip, hot_target);
                cold_blocks.insert(cold_ip as u32);
            }
        }
        NativeProfileGuidance {
            cold_blocks: cold_blocks.into_iter().collect(),
            hot_branch_edges,
        }
    }

    pub(in crate::reg_vm) fn has_reachable_conditional_branch(&self, code: &[RegInstr]) -> bool {
        self.liveness.cfg.reachable_ips().into_iter().any(|ip| {
            matches!(
                code.get(ip),
                Some(RegInstr::JumpIfBool { .. } | RegInstr::JumpIfIntCompare { .. })
            )
        })
    }

    pub(in crate::reg_vm) fn reachable_mask(&self) -> Vec<bool> {
        self.liveness.cfg.reachable_mask()
    }

    fn root_before(&self, ip: usize, reg: usize) -> Option<usize> {
        self.effects.root_before(ip, reg)
    }

    fn writes_list_root(&self, start: usize, end: usize, list_root: usize) -> bool {
        self.effects.writes_list_root(start, end, list_root)
    }

    fn alias_class_readonly_for_list_slice(&self, code: &[RegInstr], slice_reg: usize) -> bool {
        self.effects
            .alias_class_readonly_for_list_slice(code, self.n_regs, slice_reg)
    }

    fn external_reads_touch(&self, code: &[RegInstr], mask: &[bool]) -> Option<bool> {
        native_region_external_reads_touch(code, self.n_regs, self.header, self.exit, mask)
    }

    fn external_reads_or_writes_touch(&self, code: &[RegInstr], mask: &[bool]) -> Option<bool> {
        native_region_external_reads_or_writes_touch(
            code,
            self.n_regs,
            self.header,
            self.exit,
            mask,
        )
    }

    fn mark_external_writes(&self, code: &[RegInstr], mask: &mut [bool]) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            if ip >= self.header && ip < self.exit {
                continue;
            }
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    for reg in writes {
                        if reg < self.n_regs {
                            mask[reg] = true;
                        }
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(())
    }

    fn global_def_count(&self, reg: usize) -> Option<usize> {
        if reg >= self.n_regs {
            return None;
        }
        Some(*self.global_def_counts.as_ref()?.get(reg)?)
    }

    fn region_def_count(&self, reg: usize) -> Option<usize> {
        if reg >= self.n_regs {
            return None;
        }
        Some(*self.region_def_counts.as_ref()?.get(reg)?)
    }

    fn single_def_ip_of(&self, code: &[RegInstr], reg: usize) -> Option<usize> {
        if self.global_def_count(reg)? != 1 {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) if writes.contains(&reg) => return Some(ip),
                RegFootprint::Some(_) => {}
                RegFootprint::All => return None,
            }
        }
        None
    }

    fn writer_ips_of(&self, code: &[RegInstr], reg: usize) -> Option<Vec<usize>> {
        if reg >= self.n_regs {
            return None;
        }
        let mut writers = Vec::new();
        for (ip, instr) in code.iter().enumerate() {
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    if writes.contains(&reg) {
                        writers.push(ip);
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(writers)
    }

    fn mark_external_reads_touching(
        &self,
        code: &[RegInstr],
        source: &[bool],
        out: &mut [bool],
    ) -> Option<()> {
        if source.len() < self.n_regs || out.len() < self.n_regs {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            if ip >= self.header && ip < self.exit {
                continue;
            }
            match instr_read_regs(instr) {
                RegFootprint::Some(reads) => {
                    for reg in reads {
                        if reg < self.n_regs && source[reg] {
                            out[reg] = true;
                        }
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(())
    }

    fn close_region_move_aliases(&self, code: &[RegInstr], mask: &mut [bool]) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        let mut changed = true;
        while changed {
            changed = false;
            for i in self.header..self.exit {
                match &code[i] {
                    RegInstr::Move { dst, src } if *dst < self.n_regs && *src < self.n_regs => {
                        if mask[*src] && !mask[*dst] {
                            mask[*dst] = true;
                            changed = true;
                        }
                    }
                    RegInstr::Move { .. } => return None,
                    _ => {}
                }
            }
        }
        Some(())
    }

    fn close_reachable_move_aliases(&self, code: &[RegInstr], mask: &mut [bool]) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        let reachable_ips = self.values.cfg.reachable_ips();
        let mut changed = true;
        while changed {
            changed = false;
            for &i in &reachable_ips {
                match &code[i] {
                    RegInstr::Move { dst, src } if *dst < self.n_regs && *src < self.n_regs => {
                        if mask[*src] && !mask[*dst] {
                            mask[*dst] = true;
                            changed = true;
                        }
                    }
                    RegInstr::Move { .. } => return None,
                    _ => {}
                }
            }
        }
        Some(())
    }

    fn reachable_defs_closed_under_moves(
        &self,
        code: &[RegInstr],
        mut seed_dst: impl FnMut(&RegInstr) -> Option<usize>,
    ) -> Option<Vec<bool>> {
        let mut mask = vec![false; self.n_regs];
        for ip in self.values.cfg.reachable_ips() {
            if let Some(dst) = seed_dst(&code[ip]) {
                if dst >= self.n_regs {
                    return None;
                }
                mask[dst] = true;
            }
        }
        self.close_reachable_move_aliases(code, &mut mask)?;
        Some(mask)
    }

    fn reachable_heap_read_defs_closed_under_moves(&self, code: &[RegInstr]) -> Option<Vec<bool>> {
        self.reachable_defs_closed_under_moves(code, |instr| match instr {
            RegInstr::GetFieldSlot { dst, .. } | RegInstr::ListGet { dst, .. } => Some(*dst),
            _ => None,
        })
    }

    fn reachable_make_closure_defs_closed_under_moves(
        &self,
        code: &[RegInstr],
    ) -> Option<Vec<bool>> {
        self.reachable_defs_closed_under_moves(code, |instr| match instr {
            RegInstr::MakeClosure { dst, .. } => Some(*dst),
            _ => None,
        })
    }

    fn native_readable_or_sinkable_closure_operands(&self, code: &[RegInstr]) -> Option<Vec<bool>> {
        let mut heap_reads = self.reachable_heap_read_defs_closed_under_moves(code)?;
        let make_closures = self.reachable_make_closure_defs_closed_under_moves(code)?;
        for (dst, src) in heap_reads.iter_mut().zip(make_closures.into_iter()) {
            *dst |= src;
        }
        Some(heap_reads)
    }

    fn forward_definite_regs(
        &self,
        code: &[RegInstr],
        transfer: impl FnMut(usize, &RegInstr, &mut [bool]) -> Option<()>,
    ) -> Option<Vec<Option<Vec<bool>>>> {
        native_forward_definite_regs(&self.values.cfg, code, self.n_regs, transfer)
    }
}

#[cfg(feature = "native-jit")]
impl NativeRegionEffects {
    fn compute(code: &[RegInstr], n_regs: usize, exit: usize) -> Self {
        let bounded_exit = exit.min(code.len());
        let mut roots: Vec<usize> = (0..n_regs).collect();
        let mut roots_before = Vec::with_capacity(bounded_exit + 1);
        let mut list_writes = Vec::new();
        let mut move_alias_roots: Vec<usize> = (0..n_regs).collect();

        for (ip, instr) in code.iter().take(bounded_exit).enumerate() {
            roots_before.push(roots.clone());
            let semantics = native_instr_semantics(instr);
            if let Some(write_list) = semantics.list_write
                && roots.get(write_list).copied().is_some()
            {
                list_writes.push((ip, roots[write_list]));
            }
            match instr {
                RegInstr::Move { dst, src } if *dst < n_regs && *src < n_regs => {
                    roots[*dst] = roots[*src];
                    native_union_roots(&mut move_alias_roots, *dst, *src);
                }
                _ => {
                    if let Some(dst) = semantics.dst
                        && dst < n_regs
                    {
                        roots[dst] = dst;
                    }
                }
            }
        }
        roots_before.push(roots);

        Self {
            roots_before,
            list_writes,
            move_alias_roots,
        }
    }

    fn root_before(&self, ip: usize, reg: usize) -> Option<usize> {
        self.roots_before.get(ip)?.get(reg).copied()
    }

    fn writes_list_root(&self, start: usize, end: usize, list_root: usize) -> bool {
        self.list_writes
            .iter()
            .any(|(ip, root)| *ip >= start && *ip < end && *root == list_root)
    }

    fn alias_mask(&self, reg: usize) -> Option<Vec<bool>> {
        let root = native_find_root_readonly(&self.move_alias_roots, reg)?;
        Some(
            (0..self.move_alias_roots.len())
                .map(|other| native_find_root_readonly(&self.move_alias_roots, other) == Some(root))
                .collect(),
        )
    }

    fn alias_class_readonly_for_list_slice(
        &self,
        code: &[RegInstr],
        n_regs: usize,
        slice_reg: usize,
    ) -> bool {
        if slice_reg >= n_regs {
            return false;
        }

        let Some(alias) = self.alias_mask(slice_reg) else {
            return false;
        };

        code.iter().all(|instr| {
            match instr {
                RegInstr::ListGet { list, .. } | RegInstr::ListLen { list, .. }
                    if *list < n_regs && alias[*list] =>
                {
                    return true;
                }
                RegInstr::DeepCopy { reg } if *reg < n_regs && alias[*reg] => return true,
                RegInstr::Move { dst, src }
                    if *dst < n_regs && *src < n_regs && alias[*dst] && alias[*src] =>
                {
                    return true;
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                } if *dst == slice_reg => {
                    return true;
                }
                instr => {
                    if let Some(dst) = native_subset_dst(instr)
                        && dst < n_regs
                        && alias[dst]
                    {
                        return false;
                    }
                }
            }

            match instr_read_regs(instr) {
                RegFootprint::Some(reads) => {
                    !reads.into_iter().any(|reg| reg < n_regs && alias[reg])
                }
                RegFootprint::All => false,
            }
        })
    }
}

#[cfg(feature = "native-jit")]
fn native_find_root_readonly(roots: &[usize], mut reg: usize) -> Option<usize> {
    if reg >= roots.len() {
        return None;
    }
    while roots[reg] != reg {
        reg = roots[reg];
        if reg >= roots.len() {
            return None;
        }
    }
    Some(reg)
}

#[cfg(feature = "native-jit")]
fn native_find_root_mut(roots: &mut [usize], reg: usize) -> usize {
    let parent = roots[reg];
    if parent != reg {
        let root = native_find_root_mut(roots, parent);
        roots[reg] = root;
        root
    } else {
        reg
    }
}

#[cfg(feature = "native-jit")]
fn native_union_roots(roots: &mut [usize], lhs: usize, rhs: usize) {
    let lhs_root = native_find_root_mut(roots, lhs);
    let rhs_root = native_find_root_mut(roots, rhs);
    if lhs_root != rhs_root {
        roots[lhs_root] = rhs_root;
    }
}

#[cfg(feature = "native-jit")]
fn native_full_list_slice_elision_candidate(
    code: &[RegInstr],
    header: usize,
    exit: usize,
    i: usize,
    analysis: &NativeRegionAnalysis,
) -> Option<(usize, usize)> {
    if i < header || i >= exit {
        return None;
    }
    let (dst, list, start, len) = match code.get(i)? {
        RegInstr::CallIntrinsic {
            dst,
            intrinsic: RegIntrinsic::ListSlice,
            args,
        }
        | RegInstr::CallTypedIntrinsic {
            dst,
            intrinsic: RegIntrinsic::ListSlice,
            args,
            ..
        } if args.len() == 3 => (*dst, args[0], args[1], args[2]),
        _ => return None,
    };

    if analysis.const_int_before(start, i) != Some(0) {
        return None;
    }
    if analysis.live_in(i, list) != Some(true) || analysis.live_out(i, dst) != Some(true) {
        return None;
    }
    let Some((len_source, len_ip)) = analysis.list_len_source_before(len, i) else {
        return None;
    };
    if len_source != list {
        return None;
    }
    if !analysis.alias_class_readonly_for_list_slice(code, dst) {
        return None;
    }

    let list_root = analysis.root_before(i, list)?;
    if analysis.writes_list_root(len_ip + 1, i, list_root)
        || analysis.writes_list_root(header, exit, list_root)
    {
        return None;
    }

    Some((dst, list))
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_region_has_readonly_full_list_slice_elision(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> bool {
    if header >= exit || exit > code.len() {
        return false;
    }
    let Some(analysis) = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit) else {
        return false;
    };
    (header..exit).any(|i| {
        native_full_list_slice_elision_candidate(code, header, exit, i, &analysis).is_some()
    })
}

/// Elide a materialized full-list slice when the slice is only used for read-only
/// list queries inside a native region:
///
/// ```text
/// tmp = List.slice(list, 0, List.len(list))
/// x   = List.get(tmp, i)
/// ```
///
/// becomes a handle alias:
///
/// ```text
/// tmp = list
/// x   = List.get(tmp, i)
/// ```
///
/// This is intentionally narrower than a general partial-slice view. A partial
/// slice needs an extra `i < slice_len` guard to preserve `List.get` failures, while
/// the full-slice case preserves the same bounds behavior through the original
/// source list. We also require the source list to remain unwritten in the region so
/// the shallow copy's read-only behavior is indistinguishable from an alias.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_elide_readonly_full_list_slices_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }

    let mut out = code.to_vec();
    let mut changed = false;
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    for i in header..exit {
        let Some((dst, list)) =
            native_full_list_slice_elision_candidate(code, header, exit, i, &analysis)
        else {
            continue;
        };

        out[i] = RegInstr::Move { dst, src: list };
        changed = true;
    }

    let ip_map: Vec<usize> = (0..code.len()).collect();
    if changed {
        Some((out, n_regs, ip_map))
    } else {
        Some((code.to_vec(), n_regs, ip_map))
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug, Default)]
struct NativeInstrSemantics {
    native_subset: bool,
    dst: Option<usize>,
    list_write: Option<usize>,
    heap_write: bool,
    field_slot_access: bool,
    control: NativeControlFlow,
    reads: RegFootprint,
    writes: RegFootprint,
}

#[cfg(feature = "native-jit")]
fn native_instr_semantics(instr: &RegInstr) -> NativeInstrSemantics {
    use RegFootprint::Some as S;

    let list_write = match instr {
        RegInstr::ListSet { list, .. }
        | RegInstr::ListPush { list, .. }
        | RegInstr::ListAppend { list, .. }
        | RegInstr::ListClear { list, .. }
        | RegInstr::ListPop { list, .. }
        | RegInstr::ListSort { list, .. }
        | RegInstr::ListRemoveAt { list, .. } => Some(*list),
        _ => None,
    };

    let heap_write = matches!(
        instr,
        RegInstr::SetFieldSlot { .. }
            | RegInstr::ListSet { .. }
            | RegInstr::ListPush { .. }
            | RegInstr::ListAppend { .. }
            | RegInstr::ListClear { .. }
            | RegInstr::ListPop { .. }
            | RegInstr::ListSort { .. }
            | RegInstr::ListRemoveAt { .. }
            | RegInstr::MapInsert { .. }
            | RegInstr::SetInsert { .. }
            | RegInstr::SortedSetInsert { .. }
            | RegInstr::SortedMapInsert { .. }
            | RegInstr::DequePushBack { .. }
            | RegInstr::DequePushFront { .. }
            | RegInstr::DequePopFront { .. }
            | RegInstr::DequePopBack { .. }
    );

    let field_slot_access = matches!(
        instr,
        RegInstr::GetFieldSlot { .. } | RegInstr::SetFieldSlot { .. }
    );

    let control = match instr {
        RegInstr::Jump { target } => NativeControlFlow::Jump(*target),
        RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
            NativeControlFlow::Branch { target: *target }
        }
        RegInstr::MatchOption {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchMapGet {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchSortedMapGet {
            some_ip, none_ip, ..
        } => NativeControlFlow::Split {
            first: *some_ip,
            second: *none_ip,
        },
        RegInstr::MatchResult { ok_ip, err_ip, .. } => NativeControlFlow::Split {
            first: *ok_ip,
            second: *err_ip,
        },
        RegInstr::MatchVariant {
            match_ip, else_ip, ..
        } => NativeControlFlow::Split {
            first: *match_ip,
            second: *else_ip,
        },
        RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => NativeControlFlow::Terminal,
        _ => NativeControlFlow::Fallthrough,
    };

    let reads = match instr {
        RegInstr::LoadUnit { .. }
        | RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::LoadNone { .. }
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. } => S(vec![]),
        RegInstr::Move { src, .. }
        | RegInstr::Manage { src, .. }
        | RegInstr::DeepCopy { reg: src }
        | RegInstr::UnwrapSome { src, .. }
        | RegInstr::UnwrapVariantValue { src, .. }
        | RegInstr::AwaitJoin { src, .. } => S(vec![*src]),
        RegInstr::ResourceDrop { resource } => S(vec![*resource]),
        RegInstr::GetField { base, .. } | RegInstr::GetFieldSlot { base, .. } => S(vec![*base]),
        RegInstr::NativeGuardClosureId { closure, .. }
        | RegInstr::NativeClosureId { closure, .. }
        | RegInstr::NativeClosureCapture { closure, .. } => S(vec![*closure]),
        RegInstr::NativeFieldClosureId { base, .. }
        | RegInstr::NativeFieldClosureCapture { base, .. } => S(vec![*base]),
        RegInstr::SetField { base, value, .. } | RegInstr::SetFieldSlot { base, value, .. } => {
            S(vec![*base, *value])
        }
        RegInstr::AddInt { lhs, rhs, .. }
        | RegInstr::SubInt { lhs, rhs, .. }
        | RegInstr::MulInt { lhs, rhs, .. }
        | RegInstr::DivInt { lhs, rhs, .. }
        | RegInstr::ModInt { lhs, rhs, .. }
        | RegInstr::BitAndInt { lhs, rhs, .. }
        | RegInstr::BitOrInt { lhs, rhs, .. }
        | RegInstr::BitXorInt { lhs, rhs, .. }
        | RegInstr::ShiftLeftInt { lhs, rhs, .. }
        | RegInstr::ShiftRightInt { lhs, rhs, .. }
        | RegInstr::LessInt { lhs, rhs, .. }
        | RegInstr::LessEqualInt { lhs, rhs, .. }
        | RegInstr::GreaterInt { lhs, rhs, .. }
        | RegInstr::GreaterEqualInt { lhs, rhs, .. }
        | RegInstr::Equal { lhs, rhs, .. }
        | RegInstr::NotEqual { lhs, rhs, .. }
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => S(vec![*lhs, *rhs]),
        RegInstr::JumpIfBool { cond, .. } => S(vec![*cond]),
        RegInstr::MatchOption { src, .. }
        | RegInstr::MatchResult { src, .. }
        | RegInstr::MatchVariant { src, .. } => S(vec![*src]),
        RegInstr::TryResult { src, cleanup, .. } => {
            let mut reads = Vec::with_capacity(cleanup.len() + 1);
            reads.push(*src);
            reads.extend(cleanup.iter().copied());
            S(reads)
        }
        RegInstr::MatchMapGet { map, key, .. } | RegInstr::MatchSortedMapGet { map, key, .. } => {
            S(vec![*map, *key])
        }
        RegInstr::ListLen { list, .. } => S(vec![*list]),
        RegInstr::ListGet { list, index, .. } => S(vec![*list, *index]),
        RegInstr::ListSet {
            list, index, value, ..
        } => S(vec![*list, *index, *value]),
        RegInstr::ListPush { list, value, .. } => S(vec![*list, *value]),
        RegInstr::ListSort { list, .. } => S(vec![*list]),
        RegInstr::MapInsert {
            map, key, value, ..
        } => S(vec![*map, *key, *value]),
        RegInstr::SetInsert { set, value, .. } => S(vec![*set, *value]),
        RegInstr::SortedSetInsert { set, value, .. } => S(vec![*set, *value]),
        RegInstr::SortedMapInsert {
            map, key, value, ..
        } => S(vec![*map, *key, *value]),
        RegInstr::DequePushBack { deque, value, .. }
        | RegInstr::DequePushFront { deque, value, .. } => S(vec![*deque, *value]),
        RegInstr::DequePopFront { deque, .. } | RegInstr::DequePopBack { deque, .. } => {
            S(vec![*deque])
        }
        RegInstr::MakeSome { value, .. } => S(vec![*value]),
        RegInstr::StringConcat { left, right, .. } => S(vec![*left, *right]),
        RegInstr::Return { src } => S(vec![*src]),
        // Call / construction families: their value operands are the arg/field/item
        // register vectors (the `dst` is written, not read).
        RegInstr::CallKnown { args, .. }
        | RegInstr::CallDynamic { args, .. }
        | RegInstr::SpawnTask { args, .. }
        | RegInstr::CallNative { args, .. }
        | RegInstr::CallIntrinsic { args, .. }
        | RegInstr::CallTypedIntrinsic { args, .. } => S(args.clone()),
        RegInstr::CallClosure { closure, args, .. } => {
            let mut v = args.clone();
            v.push(*closure);
            S(v)
        }
        RegInstr::MakeList { items, .. } => S(items.clone()),
        RegInstr::MakeStruct { fields, .. }
        | RegInstr::MakeVariant { fields, .. }
        | RegInstr::MakeObject { fields, .. } => S(fields.iter().map(|(_, r)| *r).collect()),
        RegInstr::MakeMap { entries, .. } => {
            S(entries.iter().flat_map(|(k, v)| [*k, *v]).collect())
        }
        RegInstr::MakeClosure { captures, .. } => S(captures.clone()),
        // Anything not modelled above (collection mutators, select, try, …) ⇒
        // conservatively "reads everything".
        _ => RegFootprint::All,
    };

    let dst = match instr {
        RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::Move { dst, .. }
        | RegInstr::DeepCopy { reg: dst, .. }
        | RegInstr::AddInt { dst, .. }
        | RegInstr::SubInt { dst, .. }
        | RegInstr::MulInt { dst, .. }
        | RegInstr::DivInt { dst, .. }
        | RegInstr::ModInt { dst, .. }
        | RegInstr::BitAndInt { dst, .. }
        | RegInstr::BitOrInt { dst, .. }
        | RegInstr::BitXorInt { dst, .. }
        | RegInstr::ShiftLeftInt { dst, .. }
        | RegInstr::ShiftRightInt { dst, .. }
        | RegInstr::LessInt { dst, .. }
        | RegInstr::LessEqualInt { dst, .. }
        | RegInstr::GreaterInt { dst, .. }
        | RegInstr::GreaterEqualInt { dst, .. }
        | RegInstr::Equal { dst, .. }
        | RegInstr::NotEqual { dst, .. }
        | RegInstr::GetFieldSlot { dst, .. }
        | RegInstr::ListLen { dst, .. }
        | RegInstr::ListGet { dst, .. }
        | RegInstr::ListSet { dst, .. }
        | RegInstr::ListPush { dst, .. }
        | RegInstr::ListSort { dst, .. }
        | RegInstr::MapInsert { dst, .. }
        | RegInstr::SetInsert { dst, .. }
        | RegInstr::SortedSetInsert { dst, .. }
        | RegInstr::SortedMapInsert { dst, .. }
        | RegInstr::DequePushBack { dst, .. }
        | RegInstr::DequePushFront { dst, .. }
        | RegInstr::DequePopFront { dst, .. }
        | RegInstr::DequePopBack { dst, .. }
        | RegInstr::StringConcat { dst, .. }
        | RegInstr::NativeClosureId { dst, .. }
        | RegInstr::NativeClosureCapture { dst, .. }
        | RegInstr::NativeFieldClosureId { dst, .. }
        | RegInstr::NativeFieldClosureCapture { dst, .. }
        | RegInstr::CallIntrinsic { dst, .. }
        | RegInstr::CallTypedIntrinsic { dst, .. } => Some(*dst),
        RegInstr::MatchMapGet { value_dst, .. } | RegInstr::MatchSortedMapGet { value_dst, .. } => {
            Some(*value_dst)
        }
        RegInstr::SetFieldSlot { base, .. } => Some(*base),
        _ => None,
    };

    let writes = match instr {
        RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::MatchOption { .. }
        | RegInstr::MatchResult { .. }
        | RegInstr::MatchVariant { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::NativeGuardClosureId { .. }
        | RegInstr::ResourceDrop { .. }
        | RegInstr::DeepCopy { .. }
        | RegInstr::Return { .. } => S(vec![]),
        RegInstr::LoadUnit { dst }
        | RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::LoadString { dst, .. }
        | RegInstr::LoadNone { dst }
        | RegInstr::Move { dst, .. }
        | RegInstr::Manage { dst, .. }
        | RegInstr::TryResult { dst, .. }
        | RegInstr::GetField { dst, .. }
        | RegInstr::GetFieldSlot { dst, .. }
        | RegInstr::SetField { dst, .. }
        | RegInstr::SetFieldSlot { dst, .. }
        | RegInstr::MakeStruct { dst, .. }
        | RegInstr::MakeVariant { dst, .. }
        | RegInstr::MakeList { dst, .. }
        | RegInstr::MakeObject { dst, .. }
        | RegInstr::MakeMap { dst, .. }
        | RegInstr::MakeClosure { dst, .. }
        | RegInstr::MakeSome { dst, .. }
        | RegInstr::AddInt { dst, .. }
        | RegInstr::SubInt { dst, .. }
        | RegInstr::MulInt { dst, .. }
        | RegInstr::DivInt { dst, .. }
        | RegInstr::ModInt { dst, .. }
        | RegInstr::BitAndInt { dst, .. }
        | RegInstr::BitOrInt { dst, .. }
        | RegInstr::BitXorInt { dst, .. }
        | RegInstr::ShiftLeftInt { dst, .. }
        | RegInstr::ShiftRightInt { dst, .. }
        | RegInstr::LessInt { dst, .. }
        | RegInstr::LessEqualInt { dst, .. }
        | RegInstr::GreaterInt { dst, .. }
        | RegInstr::GreaterEqualInt { dst, .. }
        | RegInstr::Equal { dst, .. }
        | RegInstr::NotEqual { dst, .. }
        | RegInstr::UnwrapSome { dst, .. }
        | RegInstr::UnwrapVariantValue { dst, .. }
        | RegInstr::ListLen { dst, .. }
        | RegInstr::ListGet { dst, .. }
        | RegInstr::ListSet { dst, .. }
        | RegInstr::ListPush { dst, .. }
        | RegInstr::ListSort { dst, .. }
        | RegInstr::MapInsert { dst, .. }
        | RegInstr::SetInsert { dst, .. }
        | RegInstr::SortedSetInsert { dst, .. }
        | RegInstr::SortedMapInsert { dst, .. }
        | RegInstr::DequePushBack { dst, .. }
        | RegInstr::DequePushFront { dst, .. }
        | RegInstr::DequePopFront { dst, .. }
        | RegInstr::DequePopBack { dst, .. }
        | RegInstr::CallKnown { dst, .. }
        | RegInstr::CallDynamic { dst, .. }
        | RegInstr::SpawnTask { dst, .. }
        | RegInstr::AwaitJoin { dst, .. }
        | RegInstr::CallNative { dst, .. }
        | RegInstr::CallClosure { dst, .. }
        | RegInstr::CallIntrinsic { dst, .. }
        | RegInstr::CallTypedIntrinsic { dst, .. }
        | RegInstr::NativeClosureId { dst, .. }
        | RegInstr::NativeClosureCapture { dst, .. }
        | RegInstr::NativeFieldClosureId { dst, .. }
        | RegInstr::NativeFieldClosureCapture { dst, .. }
        | RegInstr::StringConcat { dst, .. } => S(vec![*dst]),
        RegInstr::MatchMapGet { value_dst, .. } | RegInstr::MatchSortedMapGet { value_dst, .. } => {
            S(vec![*value_dst])
        }
        // `SelectWait` writes winner/value; calls with `mut_args` write back to arg
        // registers — all modelled conservatively as `All` so OSR × J3 bails rather
        // than risk a missed boundary write.
        _ => RegFootprint::All,
    };

    let native_subset = match instr {
        RegInstr::CallIntrinsic {
            intrinsic, args, ..
        } => {
            native_host_typed_intrinsic(*intrinsic, None)
                .is_some_and(|spec| args.len() == spec.arg_tys().len())
                || native_inline_convert_intrinsic(*intrinsic)
                    .is_some_and(|arity| args.len() == arity)
        }
        RegInstr::CallTypedIntrinsic {
            intrinsic,
            type_arg,
            args,
            ..
        } => native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
            .is_some_and(|spec| args.len() == spec.arg_tys().len()),
        RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::Move { .. }
        | RegInstr::DeepCopy { .. }
        | RegInstr::AddInt { .. }
        | RegInstr::SubInt { .. }
        | RegInstr::MulInt { .. }
        | RegInstr::DivInt { .. }
        | RegInstr::ModInt { .. }
        | RegInstr::BitAndInt { .. }
        | RegInstr::BitOrInt { .. }
        | RegInstr::BitXorInt { .. }
        | RegInstr::ShiftLeftInt { .. }
        | RegInstr::ShiftRightInt { .. }
        | RegInstr::LessInt { .. }
        | RegInstr::LessEqualInt { .. }
        | RegInstr::GreaterInt { .. }
        | RegInstr::GreaterEqualInt { .. }
        | RegInstr::Equal { .. }
        | RegInstr::NotEqual { .. }
        | RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::Return { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::StringConcat { .. }
        | RegInstr::SetFieldSlot { .. }
        | RegInstr::GetFieldSlot { .. }
        | RegInstr::ListLen { .. }
        | RegInstr::ListGet { .. }
        | RegInstr::ListSet { .. }
        | RegInstr::ListPush { .. }
        | RegInstr::ListSort { .. }
        | RegInstr::MapInsert { .. }
        | RegInstr::SetInsert { .. }
        | RegInstr::SortedSetInsert { .. }
        | RegInstr::SortedMapInsert { .. }
        | RegInstr::DequePushBack { .. }
        | RegInstr::DequePushFront { .. }
        | RegInstr::DequePopFront { .. }
        | RegInstr::DequePopBack { .. }
        | RegInstr::MatchMapGet { .. }
        | RegInstr::MatchSortedMapGet { .. }
        | RegInstr::NativeGuardClosureId { .. }
        | RegInstr::NativeClosureId { .. }
        | RegInstr::NativeClosureCapture { .. }
        | RegInstr::NativeFieldClosureId { .. }
        | RegInstr::NativeFieldClosureCapture { .. } => true,
        _ => false,
    };

    NativeInstrSemantics {
        native_subset,
        dst,
        list_write,
        heap_write,
        field_slot_access,
        control,
        reads,
        writes,
    }
}

/// Whether an instruction is in the *native* JIT subset (integer/boolean/control
/// core, no heap/calls/async/floats). Tighter than [`jit_supported_instruction`].
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_subset_instruction(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).native_subset
}

/// The register a native-subset instruction definitely writes (its `dst`), if any.
/// Control/jump instructions and pure side-effect-free reads-with-no-dst write
/// nothing. Used by the OSR flat-list pass to detect loop-invariant (never-written)
/// list registers. Conservative: an instruction whose write shape isn't modeled here
/// (i.e. not in the native subset) never appears in a translated OSR loop, so a
/// `None` for it cannot cause a non-invariant register to be misclassified.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_subset_dst(instr: &RegInstr) -> Option<usize> {
    native_instr_semantics(instr).dst
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instruction_has_heap_write(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).heap_write
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instruction_touches_field_slot(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).field_slot_access
}

/// Assign type `t` to register `reg`; return `false` on a conflicting reassignment.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_set_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    t: NativeTy,
    changed: &mut bool,
) -> bool {
    match ty[reg] {
        Some(existing) => existing == t,
        None => {
            ty[reg] = Some(t);
            *changed = true;
            true
        }
    }
}

/// Unify two registers' types (they must end up equal). Propagates a known type
/// to an unknown one — this is how *parameter* types are inferred from the typed
/// operands they're combined with. `false` on a conflict.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_unify(
    ty: &mut [Option<NativeTy>],
    a: usize,
    b: usize,
    changed: &mut bool,
) -> bool {
    match (ty[a], ty[b]) {
        (Some(x), Some(y)) => x == y,
        (Some(x), None) => native_set_ty(ty, b, x, changed),
        (None, Some(y)) => native_set_ty(ty, a, y, changed),
        (None, None) => true,
    }
}

/// Mark which instructions are reachable from `ip == 0` along the control-flow
/// graph (sequential fallthrough, jumps, conditional branches). Used to ignore
/// the lowerer's unreachable defensive tail when judging native eligibility.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
    if code.is_empty() {
        return Vec::new();
    }
    NativeRegionCfg::prefix(code, code.len())
        .map(|cfg| cfg.reachable_mask())
        .unwrap_or_else(|| vec![false; code.len()])
}

/// Clone a *pure* (branch-free, call-free) native-subset instruction with every
/// register shifted by `base` — used to splice a callee body into the caller's
/// register window during inlining. `None` for anything outside that pure subset.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_offset_regs(instr: &RegInstr, b: usize) -> Option<RegInstr> {
    Some(match instr {
        RegInstr::LoadInt { dst, value } => RegInstr::LoadInt {
            dst: dst + b,
            value: *value,
        },
        RegInstr::LoadFloat { dst, value } => RegInstr::LoadFloat {
            dst: dst + b,
            value: *value,
        },
        RegInstr::LoadBool { dst, value } => RegInstr::LoadBool {
            dst: dst + b,
            value: *value,
        },
        RegInstr::Move { dst, src } => RegInstr::Move {
            dst: dst + b,
            src: src + b,
        },
        RegInstr::DeepCopy { reg } => RegInstr::DeepCopy { reg: reg + b },
        RegInstr::GetFieldSlot { dst, base, slot } => RegInstr::GetFieldSlot {
            dst: dst + b,
            base: base + b,
            slot: *slot,
        },
        RegInstr::SetFieldSlot {
            dst,
            base,
            slot,
            value,
        } => RegInstr::SetFieldSlot {
            dst: dst + b,
            base: base + b,
            slot: *slot,
            value: value + b,
        },
        RegInstr::NativeFieldClosureId { dst, base, slot } => RegInstr::NativeFieldClosureId {
            dst: dst + b,
            base: base + b,
            slot: *slot,
        },
        RegInstr::NativeFieldClosureCapture {
            dst,
            base,
            slot,
            index,
        } => RegInstr::NativeFieldClosureCapture {
            dst: dst + b,
            base: base + b,
            slot: *slot,
            index: *index,
        },
        RegInstr::ListLen { dst, list } => RegInstr::ListLen {
            dst: dst + b,
            list: list + b,
        },
        RegInstr::ListGet { dst, list, index } => RegInstr::ListGet {
            dst: dst + b,
            list: list + b,
            index: index + b,
        },
        RegInstr::ListSet {
            dst,
            list,
            index,
            value,
        } => RegInstr::ListSet {
            dst: dst + b,
            list: list + b,
            index: index + b,
            value: value + b,
        },
        RegInstr::ListPush { dst, list, value } => RegInstr::ListPush {
            dst: dst + b,
            list: list + b,
            value: value + b,
        },
        RegInstr::ListSort { dst, list } => RegInstr::ListSort {
            dst: dst + b,
            list: list + b,
        },
        RegInstr::AddInt { dst, lhs, rhs } => RegInstr::AddInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::SubInt { dst, lhs, rhs } => RegInstr::SubInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::MulInt { dst, lhs, rhs } => RegInstr::MulInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::DivInt { dst, lhs, rhs } => RegInstr::DivInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ModInt { dst, lhs, rhs } => RegInstr::ModInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitAndInt { dst, lhs, rhs } => RegInstr::BitAndInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitOrInt { dst, lhs, rhs } => RegInstr::BitOrInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitXorInt { dst, lhs, rhs } => RegInstr::BitXorInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ShiftLeftInt { dst, lhs, rhs } => RegInstr::ShiftLeftInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ShiftRightInt { dst, lhs, rhs } => RegInstr::ShiftRightInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::LessInt { dst, lhs, rhs } => RegInstr::LessInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::LessEqualInt { dst, lhs, rhs } => RegInstr::LessEqualInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::GreaterInt { dst, lhs, rhs } => RegInstr::GreaterInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::GreaterEqualInt { dst, lhs, rhs } => RegInstr::GreaterEqualInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::Equal { dst, lhs, rhs } => RegInstr::Equal {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::NotEqual { dst, lhs, rhs } => RegInstr::NotEqual {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        _ => return None,
    })
}

/// Register-offset an instruction for the OSR×inline path, which (unlike the plain
/// native subset) ALSO accepts the J3-dissolvable value ops: a callee that builds /
/// destructures a non-escaping `Option`/variant/struct is spliced into the loop body
/// so the J3 region passes can dissolve it to scalars. The branch-shaped match ops
/// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`) are remapped by the
/// splicer (they carry callee ip targets), not here. `None` if neither the native
/// subset nor a J3 value op.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_offset_regs_j3(instr: &RegInstr, b: usize) -> Option<RegInstr> {
    if let Some(offset) = native_offset_regs(instr, b) {
        return Some(offset);
    }
    Some(match instr {
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
        } => RegInstr::MakeVariant {
            dst: dst + b,
            layout: layout.clone(),
            fields: fields.iter().map(|(n, r)| (n.clone(), r + b)).collect(),
        },
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        } => RegInstr::MakeStruct {
            dst: dst + b,
            layout: layout.clone(),
            fields: fields.iter().map(|(n, r)| (n.clone(), r + b)).collect(),
        },
        RegInstr::UnwrapVariantValue { dst, src, expected } => RegInstr::UnwrapVariantValue {
            dst: dst + b,
            src: src + b,
            expected: expected.clone(),
        },
        RegInstr::MakeSome { dst, value } => RegInstr::MakeSome {
            dst: dst + b,
            value: value + b,
        },
        RegInstr::LoadNone { dst } => RegInstr::LoadNone { dst: dst + b },
        RegInstr::UnwrapSome { dst, src } => RegInstr::UnwrapSome {
            dst: dst + b,
            src: src + b,
        },
        _ => return None,
    })
}

/// Whether `intrinsic` is a PURE, side-effect-free heap value-builder that the
/// deopt-before-heap cold-arm classifier is allowed to see inside a cold arm. These
/// allocate a fresh `String` from their (read-only) operands and observe/mutate
/// nothing else — so re-running the arm on the interpreter after a native `Bail`
/// reproduces it exactly (Execution Spec §7.2). Deliberately a tight whitelist: any
/// intrinsic that touches I/O, the environment, collections, time, or RNG is impure
/// and must NOT appear in a bailable cold arm. Unknown ⇒ false (reject).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn cold_arm_pure_intrinsic(intrinsic: &RegIntrinsic) -> bool {
    // Classification reads the central registry's two cold-arm whitelists: the pure heap
    // BUILDERS (`cold_arm_pure_builder` — StringCopy/StringFrom*/StringSlice/StringPadLeft/
    // BytesFromString/BytesSlice) and the pure first-order scalar READERS
    // (`cold_arm_pure_reader` — String.count/contains/index_of/starts_with). Both are
    // side-effect-free and faithfully re-runnable on the interpreter after a native
    // `Bail`; the cold-arm pass keeps its exact arm-detection mechanism.
    let d = intrinsic_descriptor(*intrinsic);
    d.cold_arm_pure_builder || d.cold_arm_pure_reader
}

/// Whether `instr` is a pure, side-effect-free value-construction instruction that
/// the deopt-before-heap cold-arm classifier permits inside a bailable cold arm. It
/// covers the J3-dissolvable value ops (`native_offset_regs_j3`-class), plus the
/// recognized pure HEAP value-builders a cold arm may use to construct its returned
/// value: `LoadString`, a whitelisted pure `CallIntrinsic` (e.g. `String.copy`),
/// `StringConcat`, and `MakeVariant`/`MakeStruct` (the `Err(..)` / record the arm
/// returns). Every such op only READS its operands and writes its single `dst`; none
/// has an observable effect, so an arm built from them can be discarded by a native
/// `Bail` and faithfully re-run on the interpreter. `Move` is included (scalar copy).
/// Branches, returns, calls, suspends, and collection mutators are NOT pure here.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn cold_arm_pure_value_op(instr: &RegInstr) -> bool {
    if native_offset_regs_j3(instr, 0).is_some() {
        return true;
    }
    match instr {
        RegInstr::LoadString { .. }
        | RegInstr::LoadUnit { .. }
        | RegInstr::StringConcat { .. }
        | RegInstr::MakeVariant { .. }
        | RegInstr::MakeStruct { .. }
        | RegInstr::MakeSome { .. }
        // Arm-LOCAL collection build/mutate/read: a freshly-built `List` the arm pushes to
        // and queries, e.g. `let t = []; t.push(x); return List.len(t)`. `ListPush` is a
        // heap WRITE, but it is SAFE in a deopt-replaceable cold arm because native bails
        // at the arm start `s` and NEVER executes it — the interpreter re-runs the whole
        // arm and performs the push itself. `deopt_replaceable_cold_arms` additionally
        // requires every mutated collection register to be DEFINED INSIDE the arm
        // (non-escaping / not caller-aliased) so the abandon-and-reinterpret fallback has
        // no aliased-heap interaction to reason about. `MakeMap`/`ListAppend` etc. are
        // intentionally NOT yet admitted — extend per-case with a directed repro.
        | RegInstr::MakeList { .. }
        | RegInstr::ListPush { .. }
        | RegInstr::ListLen { .. } => true,
        RegInstr::CallIntrinsic { intrinsic, .. } => cold_arm_pure_intrinsic(intrinsic),
        _ => false,
    }
}

/// Deopt-before-heap classifier. Identify every **deopt-replaceable cold arm** in a
/// callee about to be inlined into an OSR loop: a maximal straight-line suffix
/// `[s..=e]` such that
///   1. `code[e]` is a `Return`,
///   2. every instruction in `[s..=e]` is a pure value-construction op
///      ([`cold_arm_pure_value_op`]) — including the recognized pure HEAP builders —
///      and none falls outside that set (no branch/call/suspend/collection-mutation),
///   3. control enters the arm ONLY at `s` via a branch boundary: no reachable
///      instruction OUTSIDE `[s..=e]` jumps or falls into any ip in `[s..=e]` (the
///      interior is private to the arm, and `s` is reached purely as the taken/
///      not-taken edge of a preceding branch — never by the native path falling
///      straight through), and
///   4. register isolation: no register written inside `[s..=e]` is read by any
///      reachable instruction OUTSIDE `[s..=e]`.
///
/// Such an arm is replaced at splice time by a single native `Bail` (a `RuntimeError`
/// sentinel) at `s`: because every op in the arm is side-effect-free, native does
/// NOTHING observable before bailing, so the existing abandon-and-reinterpret-the-
/// loop fallback re-runs the whole loop on the interpreter — which rebuilds the heap
/// value itself (Execution Spec §7.2 holds unchanged; no rollback, no resume-ip).
///
/// Returns `(cold, arm_start)` where `cold[i]` marks every ip in some cold arm and
/// `arm_start[i]` marks the single `s` of each arm (where the `Bail` is emitted). The
/// caller treats a reachable, non-cold instruction that is neither native-subset nor
/// a J3 op / match / branch as a veto (no inline). Conservative throughout: anything
/// not provably a clean pure tail is simply NOT marked cold (so it must be otherwise
/// classifiable, else the inline is vetoed).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn deopt_replaceable_cold_arms(
    code: &[RegInstr],
    reachable: &[bool],
) -> (Vec<bool>, Vec<bool>) {
    let n = code.len();
    let mut cold = vec![false; n];
    let mut arm_start = vec![false; n];

    // Find each reachable `Return` and walk backward to the maximal pure-value
    // straight-line prefix. A candidate arm `[s..=e]` is only ACCEPTED after the
    // entry + isolation checks below; failure leaves it unmarked (the inline gate
    // then decides whether the leaf is still classifiable some other way).
    for e in 0..n {
        if !reachable[e] || !matches!(code[e], RegInstr::Return { .. }) {
            continue;
        }
        if cold[e] {
            continue; // already part of an accepted arm (returns can't overlap)
        }
        // Extend `s` downward while the predecessor is a pure value op. Stop at the
        // first non-pure / non-reachable instruction (or the function start).
        let mut s = e;
        while s > 0 && reachable[s - 1] && cold_arm_pure_value_op(&code[s - 1]) {
            s -= 1;
        }
        // The arm must build a heap value that the J3 region passes CANNOT dissolve —
        // otherwise it is a SUPPORTED arm (e.g. the `Ok(scalar)` arm, a
        // `MakeVariant`/`MakeStruct`/`MakeSome` with a scalar payload) that should
        // inline normally and dissolve, NOT bail. The undissolvable builders are the
        // String/heap-scalar producers (`LoadString`, `StringConcat`, a `CallIntrinsic`
        // such as `String.copy`): a `MakeVariant{Err, [String]}` is only cold because
        // its payload flows from one of these. Require at least one such op in
        // `[s..=e]`; otherwise leave the arm to the normal J3 path (do not mark cold).
        let has_undissolvable_heap_builder = (s..=e).any(|j| {
            matches!(
                &code[j],
                RegInstr::LoadString { .. }
                    | RegInstr::StringConcat { .. }
                    | RegInstr::CallIntrinsic { .. }
                    | RegInstr::CallTypedIntrinsic { .. }
                    | RegInstr::MakeList { .. }
            )
        });
        if !has_undissolvable_heap_builder {
            continue;
        }
        // Entry/interior isolation: no reachable instruction OUTSIDE `[s..=e]` may
        // transfer control into the interior `(s..=e]`, and the native path must not
        // fall straight into `s`. We check both by enumerating every reachable
        // instruction's control-flow successors and rejecting any that lands in
        // `(s..=e]` from outside, plus requiring the textual predecessor `s-1` (if
        // reachable) to be a NON-fallthrough terminator/branch so the native path
        // cannot fall into `s`.
        let in_arm = |ip: usize| ip >= s && ip <= e;
        let mut ok = true;
        // `s` must be entered only via an explicit branch edge: its textual
        // predecessor must not fall through into it. A `JumpIf*`/`Match*` whose
        // not-taken/fallthrough edge is `s` IS an explicit branch boundary (the arm
        // is the cold side), which is exactly the shape we want — those are allowed.
        // A plain pure op or `Jump`/`Return`/`RuntimeError` predecessor: a pure op
        // would fall through into `s` on the native path (reject); `Jump`/`Return`/
        // `RuntimeError`/match/branch do not fall through, so `s` is only reached via
        // an explicit target/edge (allow). `s == 0` is a function entry (allow only
        // if entry is genuinely a branch target, which for a leaf it is not — but a
        // whole-body cold function would have been native-translated already; be
        // conservative and reject `s == 0`).
        if s == 0 {
            ok = false;
        } else if reachable[s - 1] {
            if !native_instr_is_control_boundary(&code[s - 1]) {
                ok = false;
            }
        }
        // No external control-flow edge into the interior `(s..=e]`. (An edge to `s`
        // itself from a preceding branch is fine — that is the cold-arm entry.)
        if ok {
            for (ip, instr) in code.iter().enumerate() {
                if !reachable[ip] || in_arm(ip) {
                    continue;
                }
                let lands_inside = |t: usize| t > s && t <= e;
                let mut hits = false;
                native_instr_successors(instr, ip, n, |target| {
                    if lands_inside(target) {
                        hits = true;
                    }
                });
                if hits {
                    ok = false;
                    break;
                }
            }
        }
        // Register isolation: every register the arm WRITES must be dead outside the
        // arm (read by no reachable out-of-arm instruction). A write we cannot model
        // (`RegFootprint::All`) cannot occur — the arm is all pure value ops — but be
        // defensive and reject if it ever appears.
        if ok {
            let mut written: Vec<usize> = Vec::new();
            for j in s..=e {
                match instr_written_reg(&code[j]) {
                    RegFootprint::Some(ws) => written.extend(ws),
                    RegFootprint::All => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                'outer: for (ip, instr) in code.iter().enumerate() {
                    if !reachable[ip] || in_arm(ip) {
                        continue;
                    }
                    match instr_read_regs(instr) {
                        RegFootprint::Some(rs) => {
                            for r in rs {
                                if written.contains(&r) {
                                    ok = false;
                                    break 'outer;
                                }
                            }
                        }
                        RegFootprint::All => {
                            ok = false;
                            break 'outer;
                        }
                    }
                }
            }
        }
        // Arm-LOCAL mutation guard: every heap WRITE op in the arm must mutate a
        // collection register that is DEFINED INSIDE the arm (built by an in-arm op such
        // as `MakeList`), never a caller-aliased / live-in register. Native never executes
        // the arm (it bails at `s`), so the interpreter re-runs the write on
        // abandon-and-reinterpret; restricting writes to non-escaping arm-local
        // collections keeps that fallback free of any aliased-heap reasoning (no
        // interaction with native in-place writes elsewhere in the loop). Currently the
        // only admitted write is `ListPush` (its mutated register is `list`).
        if ok {
            let mut defined_in_arm: Vec<usize> = Vec::new();
            for j in s..=e {
                if let RegFootprint::Some(ws) = instr_written_reg(&code[j]) {
                    defined_in_arm.extend(ws);
                }
            }
            for j in s..=e {
                if let RegInstr::ListPush { list, .. } = &code[j] {
                    if !defined_in_arm.contains(&(*list as usize)) {
                        ok = false; // caller-aliased / live-in mutation — reject
                        break;
                    }
                }
            }
        }
        if ok {
            for j in s..=e {
                cold[j] = true;
            }
            arm_start[s] = true;
        }
    }

    (cold, arm_start)
}

/// Whether `callee` can be inlined into the OSR loop body: like
/// [`native_callee_inlinable`] but ALSO permitting the J3-dissolvable value ops
/// ([`native_offset_regs_j3`]) and the branch-shaped match ops, so a leaf that
/// builds/destructures a non-escaping `Option`/variant/struct (e.g.
/// `make_shape`/`area`) qualifies — the value becomes loop-local once inlined and
/// the J3 region passes dissolve it. Still captureless, arity-matched, side-effect-
/// free (no calls/suspends/heap-collection/runtime-error/non-J3 ops).
///
/// Deopt-before-heap extension: a reachable instruction that is part of a
/// **deopt-replaceable cold arm** ([`deopt_replaceable_cold_arms`]) is also accepted
/// — at splice time that arm is replaced by a native `Bail`, so a leaf whose COLD arm
/// builds a heap value (e.g. `Err(String.copy(..))`) qualifies as long as its
/// SUPPORTED (non-cold) arms are fully native/J3-dissolvable. When unsure, REJECT.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable_j3(callee: &RegFunction, n_args: usize) -> bool {
    if callee.captures != 0 || callee.params != n_args {
        return false;
    }
    let reachable = native_reachable_instructions(&callee.code);
    let (cold, _arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
    callee.code.iter().enumerate().all(|(i, instr)| {
        let ok = !reachable[i]
            || cold[i]
            || matches!(
                instr,
                RegInstr::Jump { .. }
                    | RegInstr::JumpIfBool { .. }
                    | RegInstr::JumpIfIntCompare { .. }
                    | RegInstr::Return { .. }
                    | RegInstr::RuntimeError { .. }
                    | RegInstr::MatchOption { .. }
                    | RegInstr::MatchResult { .. }
                    | RegInstr::MatchVariant { .. }
                    | RegInstr::MatchMapGet { .. }
                    | RegInstr::MatchSortedMapGet { .. }
            )
            || native_offset_regs_j3(instr, 0).is_some();
        ok
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable_j3_with_spawns(
    unit: &RegUnit,
    callee: &RegFunction,
    n_args: usize,
) -> bool {
    fn go(unit: &RegUnit, callee: &RegFunction, n_args: usize, depth: usize) -> bool {
        if depth > unit.functions.len() || callee.captures != 0 || callee.params != n_args {
            return false;
        }
        let reachable = native_reachable_instructions(&callee.code);
        let (cold, _arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
        let mut direct_spawn_results: Vec<usize> = Vec::new();
        let mut direct_await_results: Vec<usize> = Vec::new();
        for (i, instr) in callee.code.iter().enumerate() {
            if !reachable[i] || cold[i] {
                continue;
            }
            match instr {
                RegInstr::SpawnTask {
                    dst,
                    function,
                    args,
                } => {
                    let Some(spawned) = unit.functions.get(*function) else {
                        return false;
                    };
                    if !go(unit, spawned, args.len(), depth + 1) {
                        return false;
                    }
                    direct_spawn_results.push(*dst);
                }
                RegInstr::AwaitJoin { dst, src } if direct_spawn_results.contains(src) => {
                    direct_await_results.push(*dst);
                }
                RegInstr::Move { dst, src } if direct_spawn_results.contains(src) => {
                    direct_spawn_results.push(*dst);
                }
                RegInstr::TryResult { src, cleanup, .. }
                    if cleanup.is_empty() && direct_await_results.contains(src) => {}
                _ => {
                    let ok = matches!(
                        instr,
                        RegInstr::Jump { .. }
                            | RegInstr::JumpIfBool { .. }
                            | RegInstr::JumpIfIntCompare { .. }
                            | RegInstr::Return { .. }
                            | RegInstr::RuntimeError { .. }
                            | RegInstr::MatchOption { .. }
                            | RegInstr::MatchResult { .. }
                            | RegInstr::MatchVariant { .. }
                            | RegInstr::MatchMapGet { .. }
                            | RegInstr::MatchSortedMapGet { .. }
                    ) || native_offset_regs_j3(instr, 0).is_some();
                    if !ok {
                        return false;
                    }
                }
            }
        }
        true
    }

    go(unit, callee, n_args, 0)
}

/// Return a copy of `callee` with the string-length-law fold applied to its WHOLE body,
/// or `None` if the fold is a no-op (the common case — use the original callee then).
///
/// Motivation (#7 foldable cold-arm sub-case): a leaf whose only heap is a measured
/// throwaway string (e.g. an `if`-arm `return String.len(String.from_int(x))`) is NOT
/// leaf-inlinable as written — the heap builder makes `deopt_replaceable_cold_arms`
/// reject the arm (its `Return` value is live), so the loop calling it declines OSR.
/// The string-fold (already used in-region, semantics-preserving) dissolves
/// `String.len`-of-foldable into digit-count/byte-length arithmetic and DELETES the dead
/// allocation, turning such a body into pure native-subset scalar code. Folding the
/// callee BEFORE the inlinability check + splice lets it inline and the loop OSR, with NO
/// deopt involved (there is no longer a heap arm to bail on). The fold is
/// semantics-preserving, so splicing the folded body is always correct; at worst an
/// un-foldable body returns `None` and nothing changes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_string_folded_callee(callee: &RegFunction) -> Option<RegFunction> {
    if callee.code.is_empty() {
        return None;
    }
    // Chain the semantics-preserving length-law folds over the WHOLE callee body: first
    // the string-length fold (`String.len`-of-foldable → digit/concat/slice arithmetic),
    // then the Bytes-length fold (`Bytes.len`-of-foldable → byte-length arithmetic) on its
    // result. Both DELETE the dissolved allocation, so a measured-throwaway string OR bytes
    // cold arm becomes pure native-subset scalar code. Each is a no-op for a body lacking
    // its pattern, returning the input unchanged.
    let (s_code, s_regs, _s_map) =
        native_string_length_fold_in_region(&callee.code, callee.regs, 0, callee.code.len())?;
    let (folded_code, folded_regs, _b_map) =
        native_bytes_length_fold_in_region(&s_code, s_regs, 0, s_code.len())?;
    // No-op chain ⇒ original (a real fold shrinks the stream and/or grows the reg file;
    // equal length AND regs vs the ORIGINAL ⇒ nothing folded by either pass).
    if folded_code.len() == callee.code.len() && folded_regs == callee.regs {
        return None;
    }
    let mut folded = callee.clone();
    folded.code = folded_code;
    folded.regs = folded_regs;
    Some(folded)
}

/// Whether `callee` can be inlined into a native function: captureless, arity
/// matches, and every reachable instruction is a pure native-subset op, native
/// control flow (jump/branch), or a `Return`. Unlike the original straight-line
/// restriction this permits internal branches and loops; calls, suspends,
/// matches, heap ops and runtime errors still make the caller fall back.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable(callee: &RegFunction, n_args: usize) -> bool {
    if callee.captures != 0 || callee.params != n_args {
        return false;
    }
    let reachable = native_reachable_instructions(&callee.code);
    callee.code.iter().enumerate().all(|(i, instr)| {
        !reachable[i]
            || matches!(
                instr,
                RegInstr::Jump { .. }
                    | RegInstr::JumpIfBool { .. }
                    | RegInstr::JumpIfIntCompare { .. }
                    | RegInstr::Return { .. }
            )
            || native_offset_regs(instr, 0).is_some()
    })
}

#[cfg(feature = "native-jit")]
fn native_callee_direct_callable_structurally(
    functions: &[RegFunction],
    function: usize,
    n_args: usize,
    stack: &mut Vec<usize>,
) -> bool {
    if stack.contains(&function) {
        return false;
    }
    let Some(callee) = functions.get(function) else {
        return false;
    };
    if callee.captures != 0 || callee.params != n_args || callee.params > callee.regs {
        return false;
    }
    stack.push(function);
    let reachable = native_reachable_instructions(&callee.code);
    let ok = callee.code.iter().enumerate().all(|(i, instr)| {
        if !reachable[i] || native_subset_instruction(instr) {
            return true;
        }
        let RegInstr::CallKnown {
            function,
            args,
            mut_args,
            ..
        } = instr
        else {
            return false;
        };
        let Some(nested) = functions.get(*function) else {
            return false;
        };
        (mut_args.is_empty() && native_callee_inlinable(nested, args.len()))
            || native_callee_direct_callable_structurally(functions, *function, args.len(), stack)
    });
    stack.pop();
    ok
}

/// Like [`native_callee_inlinable`] but permits a **capturing** closure callee
/// (OSR × J2): every capture must be materialized as a scalar at the inline site
/// (the gate enforces scalarity via the profile's `captures_all_scalar` bit), so
/// the body addresses its capture registers `0..captures` exactly like ordinary
/// scalar params. `n_args` is the call's argument count, which must equal the
/// callee's PARAM count (captures are bound separately). Every reachable
/// instruction must still be a pure native-subset op / native control flow /
/// `Return`; the uniform `base`-offset splice places capture reg `k` at `base + k`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_capturing_callee_inlinable(
    callee: &RegFunction,
    n_args: usize,
) -> bool {
    if callee.params != n_args {
        return false;
    }
    let reachable = native_reachable_instructions(&callee.code);
    callee.code.iter().enumerate().all(|(i, instr)| {
        !reachable[i]
            || matches!(
                instr,
                RegInstr::Jump { .. }
                    | RegInstr::JumpIfBool { .. }
                    | RegInstr::JumpIfIntCompare { .. }
                    | RegInstr::Return { .. }
            )
            || native_offset_regs(instr, 0).is_some()
    })
}

/// A cheap structural prefilter for native eligibility. It is deliberately a
/// necessary condition, not a full duplicate of [`translate_to_native_jit`]: false
/// means translation cannot currently succeed, true means "try the real translator".
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_may_translate_structurally(
    functions: &[RegFunction],
    func: &RegFunction,
) -> bool {
    if func.captures != 0 || func.params > func.regs {
        return false;
    }
    let reachable = native_reachable_instructions(&func.code);
    func.code.iter().enumerate().all(|(i, instr)| {
        if !reachable[i] || native_subset_instruction(instr) {
            return true;
        }
        match instr {
            RegInstr::CallKnown {
                function,
                args,
                mut_args,
                ..
            } => {
                let Some(callee) = functions.get(*function) else {
                    return false;
                };
                (mut_args.is_empty() && native_callee_inlinable(callee, args.len()))
                    || native_callee_direct_callable_structurally(
                        functions,
                        *function,
                        args.len(),
                        &mut Vec::new(),
                    )
            }
            // J2: a `CallClosure` whose closure is a native-readable handle (a param,
            // or a stored closure fetched via `GetFieldSlot`/`ListGet`) and which has
            // no `mut` write-backs *may* become inlinable once J1 profiles it as
            // monomorphic/polymorphic. We can't resolve the callee cold (its identity
            // is only known at runtime via the profile), so accept the shape here and
            // let the real translator decide per-profile. This keeps the function off
            // the predictably-ineligible fast-path so it can tier up after warming.
            RegInstr::CallClosure {
                closure, mut_args, ..
            } => {
                mut_args.is_empty()
                    && native_readable_or_sinkable_closure_operand_candidate(func, *closure)
            }
            RegInstr::MakeClosure { .. } => true,
            // J3/J4: aggregate ops *may* dissolve into the native subset via scalar
            // replacement if the value is non-escaping with scalar payload/fields. We
            // can't run the full escape analysis cheaply here (it needs the inlined
            // body), so accept the shape and let the real translator decide. This
            // keeps functions that construct/match Options, Results, user variants, or
            // structs off the predictably-ineligible fast-path so they can tier up.
            RegInstr::ListMap { .. } | RegInstr::ListFilter { .. } | RegInstr::ListFold { .. } => {
                true
            }
            _ if is_option_op(instr) => true,
            _ if is_variant_op(instr) => true,
            RegInstr::MatchResult { .. } => true,
            _ if is_make_struct_op(instr) => true,
            _ => false,
        }
    })
}

/// Cache functions that are predictably outside the native subset before the VM
/// starts running, so hot ineligible functions skip native tiering/translation on
/// their first call instead of after one failed translation attempt.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn mark_predictably_native_ineligible(functions: &[RegFunction]) {
    for func in functions {
        if !native_may_translate_structurally(functions, func) {
            func.native_status.set(NATIVE_STATUS_NOT_ELIGIBLE);
        }
    }
}

/// Inline `CallKnown`s to [`native_callee_inlinable`] callees into `func`,
/// returning the rewritten code and new register count — this is what makes a
/// function that calls small helpers native-eligible (the calls vanish). Callees
/// may now contain internal branches/loops: each is spliced into a fresh register
/// window, its internal jump targets are remapped, and every `Return` becomes a
/// `Move` of the result into the call's destination plus a jump to the join point
/// just past the spliced block. `None` if any call target is not inlinable (the
/// function then falls back).
/// If the `CallClosure` at instruction index `i` in `func` qualifies for J2
/// profile-guided monomorphic inlining, return the observed callee's function id
/// `k` (into `unit.functions`); otherwise `None`.
///
/// All of the following must hold (any failure ⇒ leave the site on its normal,
/// interpreter path — behavior unchanged):
/// - `func.code[i]` is a `CallClosure` with **no `mut` args** and a closure operand
///   that is a **parameter** (`< func.params`), i.e. a native-visible handle (the
///   higher-order "take a closure, call it" shape);
/// - J1's profile for site `i` is **Monomorphic** with exactly one observed callee
///   key `k` (so the identity guard has a single speculated target);
/// - `unit.functions[k]` is **non-capturing** and [`native_callee_inlinable`] at the
///   call's arity (side-effect-free, splice-able).
///
/// Read-only over the profile (a `try_borrow`, never a panic); never mutates state
/// or feeds a computed value.
/// Whether `closure` is a **native-readable closure handle** operand for a
/// `CallClosure` (Pending #1, stored-closure broadening). Either:
/// - a **parameter** handle (`closure < func.params`) — the shipped higher-order
///   "take a closure, call it" shape (marshalled into the heap-arg window); or
/// - a register produced by an in-function native-subset heap read — a
///   `GetFieldSlot` (`f = op.apply`) or `ListGet` (`op = List.get(ops, i)`) — i.e.
///   a **stored** closure fetched each iteration. Such a register is lowered to a
///   `FieldHandle`/`ListGetHandle` read (a fresh heap-table index) and consumed by
///   the closure guard/dispatch, so the closure identity is checked at runtime and
///   a wrong handle simply bails. We require the producer to be exactly one of those
///   reads so the operand is provably a fetched heap value (never a scalar register
///   reinterpreted as a handle).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_readable_closure_operand(
    func: &RegFunction,
    closure: usize,
) -> bool {
    if closure < func.params {
        return true;
    }
    // A non-param operand must be produced by a reachable native-subset heap read
    // (`GetFieldSlot`/`ListGet`), possibly through `Move` temporaries the lowerer
    // introduces. This shares the same CFG-backed alias closure as the other native
    // eligibility analyses.
    let Some(analysis) =
        NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())
    else {
        return false;
    };
    let Some(read_regs) = analysis.reachable_heap_read_defs_closed_under_moves(&func.code) else {
        return false;
    };
    read_regs.get(closure).copied().unwrap_or(false)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_readable_or_sinkable_closure_operand_candidate(
    func: &RegFunction,
    closure: usize,
) -> bool {
    if closure < func.params {
        return true;
    }
    let Some(analysis) =
        NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())
    else {
        return false;
    };
    let Some(value_regs) = analysis.native_readable_or_sinkable_closure_operands(&func.code) else {
        return false;
    };
    value_regs.get(closure).copied().unwrap_or(false)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn monomorphic_closure_inline_target(
    unit: &RegUnit,
    func: &RegFunction,
    i: usize,
) -> Option<usize> {
    let (closure, args, mut_args) = match func.code.get(i)? {
        RegInstr::CallClosure {
            closure,
            args,
            mut_args,
            ..
        } => (*closure, args, mut_args),
        _ => return None,
    };
    // Conservative shape gate: side-effect-free call (no write-backs) whose closure
    // is a native-readable handle (a parameter, or a stored closure fetched via a
    // `GetFieldSlot`/`ListGet` heap read).
    if !mut_args.is_empty() || !native_readable_closure_operand(func, closure) {
        return None;
    }
    // J1 profile: compile only after the bounded sampling window freezes. Before
    // then a one-callee observation can still mature into a polymorphic site, and
    // caching an early monomorphic native body would permanently hide that shape.
    if func.call_count.get() < PROFILE_RECORD_LIMIT {
        return None;
    }
    // Frozen profile: this site must have settled on exactly one callee.
    let profile = func.profile.try_borrow().ok()?;
    let feedback = profile.as_ref()?.call_sites.get(&i)?;
    if feedback.state() != MonoState::Monomorphic {
        return None;
    }
    // Monomorphic ⇒ exactly one observed callee key (its function id).
    let &(key, _) = feedback.observed.first()?;
    if feedback.observed.len() != 1 {
        return None;
    }
    let k = usize::try_from(key).ok()?;
    let callee = unit.functions.get(k)?;
    // Non-capturing callee: the original (shipped) path. A capturing callee is
    // allowed ONLY when every observed capture at this site was scalar (the
    // profile's monotone `captures_all_scalar` bit) — then each capture is
    // materialized as a scalar at the inline site via the `closure_capture` host
    // helper. A heap capture (or a profile that ever saw one) leaves the site on
    // its interpreter path: no inline, no OSR.
    if callee.captures == 0 {
        if !native_callee_inlinable(callee, args.len()) {
            return None;
        }
    } else if !feedback.captures_all_scalar
        || !native_capturing_callee_inlinable(callee, args.len())
    {
        return None;
    }
    Some(k)
}

/// J2.2 polymorphic inline cache gate. If the `CallClosure` at instruction index
/// `i` qualifies, return its 2–3 observed callee ids (into `unit.functions`);
/// otherwise `None`. Sibling to [`monomorphic_closure_inline_target`] — after the
/// bounded J1 sampling window freezes, a site is EITHER mono (single-guard path)
/// OR poly (this dispatch path), never both.
///
/// All of the following must hold (any failure ⇒ leave the site on its normal
/// interpreter path — behavior unchanged):
/// - `func.code[i]` is a `CallClosure` with **no `mut` args** and a closure operand
///   that is a **parameter** (`< func.params`), i.e. a native-visible handle (same
///   shape gate as the monomorphic case);
/// - J1's profile for site `i` is **Polymorphic** — by construction (J1 caps the
///   observed set at 4 and marks >3 as Megamorphic) this means **2 or 3** distinct
///   observed callee keys;
/// - **EVERY** observed callee is **non-capturing** and [`native_callee_inlinable`]
///   at the call's arity. If any single observed callee fails, the WHOLE site is
///   disqualified (no partial inlining): a no-match bail must be able to re-run the
///   exact same side-effect-free subset on the interpreter.
///
/// Read-only over the profile (a `try_borrow`, never a panic); never mutates state.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn polymorphic_closure_inline_targets(
    unit: &RegUnit,
    func: &RegFunction,
    i: usize,
) -> Option<Vec<usize>> {
    let (closure, args, mut_args) = match func.code.get(i)? {
        RegInstr::CallClosure {
            closure,
            args,
            mut_args,
            ..
        } => (*closure, args, mut_args),
        _ => return None,
    };
    // Same conservative shape gate as the monomorphic case.
    if !mut_args.is_empty() || !native_readable_closure_operand(func, closure) {
        return None;
    }
    // J1 profile: compile only after the bounded sampling window freezes, so the
    // 2- or 3-target PIC is derived from a stable observed set.
    if func.call_count.get() < PROFILE_RECORD_LIMIT {
        return None;
    }
    let profile = func.profile.try_borrow().ok()?;
    let feedback = profile.as_ref()?.call_sites.get(&i)?;
    if feedback.state() != MonoState::Polymorphic {
        return None;
    }
    // Polymorphic ⇒ 2 or 3 distinct observed callees (J1 caps at 4 / >3 ⇒ Mega).
    let n = feedback.observed.len();
    if !(2..=3).contains(&n) {
        return None;
    }
    let mut ranked: Vec<(usize, u32, usize)> = Vec::with_capacity(n);
    for (first_seen, &(key, count)) in feedback.observed.iter().enumerate() {
        let k = usize::try_from(key).ok()?;
        let callee = unit.functions.get(k)?;
        // EVERY observed callee must be inlinable, else disqualify the whole site
        // (no partial inlining — a no-match bail must re-run the exact same side-
        // effect-free subset on the interpreter). A CAPTURING callee is allowed
        // ONLY when every observed capture at this site was scalar (the profile's
        // monotone `captures_all_scalar` bit), so each capture materializes via the
        // `closure_capture` host helper. A non-scalar (heap) capture, or any callee
        // not native-inlinable at the call's arity, disqualifies the whole site.
        let ok = if callee.captures == 0 {
            native_callee_inlinable(callee, args.len())
        } else {
            feedback.captures_all_scalar && native_capturing_callee_inlinable(callee, args.len())
        };
        if !ok {
            return None;
        }
        ranked.push((k, count, first_seen));
    }
    // Hottest-first PIC arm order: the common callee gets the first compare/branch.
    // Preserve first-seen order for equal counts so the emitted code is stable.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    Some(ranked.into_iter().map(|(k, _, _)| k).collect())
}

/// True if `func` contains a `CallClosure` that *structurally* could become J2-
/// inlinable (non-`mut`, closure operand is a parameter handle, observed/observable
/// callee native-inlinable) but whose J1 profile has **not yet frozen** on a stable
/// mono/poly decision. While such a site is pending, native translation must be
/// RETRIED on a later (warmer) call rather than cached as permanently ineligible —
/// otherwise a function that tiers up to native before its profile warms would be
/// disabled forever and J2 could never fire.
///
/// Returns false once the profile is frozen ([`PROFILE_RECORD_LIMIT`] reached): the
/// decision (mono → single guard, poly → PIC, mega/ineligible → no inline) is then
/// stable and the usual NOT_ELIGIBLE caching is sound.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_translation_pending_on_profile(
    _unit: &RegUnit,
    func: &RegFunction,
) -> bool {
    // Frozen profile ⇒ no further state change ⇒ never pending.
    if func.call_count.get() >= PROFILE_RECORD_LIMIT {
        return false;
    }
    func.code.iter().enumerate().any(|(i, instr)| match instr {
        RegInstr::CallClosure {
            closure, mut_args, ..
        } => {
            if !mut_args.is_empty() || !native_readable_closure_operand(func, *closure) {
                return false;
            }
            // Structurally could still settle on a qualifying mono/poly decision:
            // either no samples yet, or a non-megamorphic profile that hasn't frozen
            // (a mono site may stay mono or grow into a qualifying 2–3-callee poly
            // site; a poly site may add a 3rd inlinable callee). Use a `try_borrow`
            // to stay panic-free; a contended borrow conservatively counts as
            // pending. A Megamorphic site is permanently disqualified.
            match func.profile.try_borrow() {
                Ok(profile) => match profile.as_ref().and_then(|p| p.call_sites.get(&i)) {
                    None => true,
                    Some(feedback) => feedback.state() != MonoState::Megamorphic,
                },
                Err(_) => true,
            }
        }
        _ => false,
    })
}

/// Whether `instr` is one of the four `Option` register-ops that the J3 scalar-
/// replacement pre-pass dissolves into tag + payload scalar registers.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_option_op(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::MakeSome { .. }
            | RegInstr::LoadNone { .. }
            | RegInstr::MatchOption { .. }
            | RegInstr::UnwrapSome { .. }
    )
}

#[cfg(feature = "native-jit")]
fn option_regs_definitely_assigned_before_region_reads(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    opt: &[bool],
) -> bool {
    let Some(analysis) = NativeRegionAnalysis::compute_region(code, n_regs, header, exit) else {
        return false;
    };

    fn mark_option_defs(instr: &RegInstr, opt: &[bool], assigned: &mut [bool]) {
        match instr {
            RegInstr::LoadNone { dst } | RegInstr::MakeSome { dst, .. } if opt[*dst] => {
                assigned[*dst] = true;
            }
            RegInstr::DequePopFront { dst, .. } | RegInstr::DequePopBack { dst, .. }
                if opt[*dst] =>
            {
                assigned[*dst] = true;
            }
            RegInstr::Move { dst, src } if opt[*dst] && opt[*src] => {
                assigned[*dst] = true;
            }
            _ => {}
        }
    }

    analysis
        .forward_definite_regs(code, |_ip, instr, assigned| {
            let reads = match instr_read_regs(instr) {
                RegFootprint::Some(reads) => reads,
                RegFootprint::All => return None,
            };
            if reads
                .into_iter()
                .any(|reg| reg < n_regs && opt[reg] && !assigned[reg])
            {
                return None;
            }
            mark_option_defs(instr, opt, assigned);
            Some(())
        })
        .is_some()
}

/// The register *read* positions (value operands) of an instruction that is in the
/// native subset ([`native_subset_instruction`]) or is one of the four `Option`
/// ops ([`is_option_op`]). Used by J3 escape analysis to find every use of an
/// Option register. Deliberately NOT a full enumeration of every `RegInstr` — the
/// scalar-replacement pass only ever calls it after confirming every reachable
/// instruction is in exactly this set, so a future enum addition outside the subset
/// cannot silently reach here (the pass bails first). `None` for any instruction
/// outside that set, which the caller treats as conservatively escaping.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn subset_or_option_reads(instr: &RegInstr) -> Option<Vec<usize>> {
    Some(match instr {
        RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::LoadNone { .. } => vec![],
        RegInstr::Move { src, .. } => vec![*src],
        RegInstr::DeepCopy { reg } => vec![*reg],
        RegInstr::AddInt { lhs, rhs, .. }
        | RegInstr::SubInt { lhs, rhs, .. }
        | RegInstr::MulInt { lhs, rhs, .. }
        | RegInstr::DivInt { lhs, rhs, .. }
        | RegInstr::ModInt { lhs, rhs, .. }
        | RegInstr::BitAndInt { lhs, rhs, .. }
        | RegInstr::BitOrInt { lhs, rhs, .. }
        | RegInstr::BitXorInt { lhs, rhs, .. }
        | RegInstr::ShiftLeftInt { lhs, rhs, .. }
        | RegInstr::ShiftRightInt { lhs, rhs, .. }
        | RegInstr::LessInt { lhs, rhs, .. }
        | RegInstr::LessEqualInt { lhs, rhs, .. }
        | RegInstr::GreaterInt { lhs, rhs, .. }
        | RegInstr::GreaterEqualInt { lhs, rhs, .. }
        | RegInstr::Equal { lhs, rhs, .. }
        | RegInstr::NotEqual { lhs, rhs, .. }
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => vec![*lhs, *rhs],
        RegInstr::JumpIfBool { cond, .. } => vec![*cond],
        RegInstr::Return { src } => vec![*src],
        RegInstr::GetFieldSlot { base, .. } => vec![*base],
        RegInstr::SetFieldSlot { base, value, .. } => vec![*base, *value],
        RegInstr::ListLen { list, .. } => vec![*list],
        RegInstr::ListGet { list, index, .. } => vec![*list, *index],
        RegInstr::ListSet {
            list, index, value, ..
        } => vec![*list, *index, *value],
        RegInstr::ListPush { list, value, .. } => vec![*list, *value],
        RegInstr::ListSort { list, .. } => vec![*list],
        RegInstr::MapInsert {
            map, key, value, ..
        } => vec![*map, *key, *value],
        RegInstr::SetInsert { set, value, .. } => vec![*set, *value],
        RegInstr::SortedSetInsert { set, value, .. } => vec![*set, *value],
        RegInstr::SortedMapInsert {
            map, key, value, ..
        } => vec![*map, *key, *value],
        RegInstr::DequePushBack { deque, value, .. }
        | RegInstr::DequePushFront { deque, value, .. } => vec![*deque, *value],
        RegInstr::DequePopFront { deque, .. } | RegInstr::DequePopBack { deque, .. } => {
            vec![*deque]
        }
        RegInstr::MatchMapGet { map, key, .. } | RegInstr::MatchSortedMapGet { map, key, .. } => {
            vec![*map, *key]
        }
        RegInstr::StringConcat { left, right, .. } => vec![*left, *right],
        RegInstr::CallIntrinsic { args, .. } | RegInstr::CallTypedIntrinsic { args, .. } => {
            args.clone()
        }
        RegInstr::NativeGuardClosureId { closure, .. } => vec![*closure],
        RegInstr::NativeClosureId { closure, .. } => vec![*closure],
        RegInstr::NativeClosureCapture { closure, .. } => vec![*closure],
        RegInstr::NativeFieldClosureId { base, .. }
        | RegInstr::NativeFieldClosureCapture { base, .. } => vec![*base],
        // Option ops and `?` success projection (value operands).
        RegInstr::MakeSome { value, .. } => vec![*value],
        RegInstr::MatchOption { src, .. } => vec![*src],
        RegInstr::UnwrapSome { src, .. } => vec![*src],
        RegInstr::TryResult { src, cleanup, .. } => {
            let mut reads = Vec::with_capacity(cleanup.len() + 1);
            reads.push(*src);
            reads.extend(cleanup.iter().copied());
            reads
        }
        // Variant ops (value operands). `MakeVariant`'s value operands are its
        // field registers; `MatchVariant`/`UnwrapVariantValue` read `src`.
        RegInstr::MakeVariant { fields, .. } => fields.iter().map(|(_, r)| *r).collect(),
        RegInstr::MatchVariant { src, .. } => vec![*src],
        RegInstr::UnwrapVariantValue { src, .. } => vec![*src],
        _ => return None,
    })
}

/// Whether `instr` is one of the three variant register-ops that the J3 scalar-
/// replacement dissolves into a tag + payload scalar register pair.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_variant_op(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::MakeVariant { .. }
            | RegInstr::MatchVariant { .. }
            | RegInstr::UnwrapVariantValue { .. }
    )
}

/// Whether `instr` is a struct construction op that the J3 struct scalar-replacement
/// dissolves into one scalar register per field slot. `GetFieldSlot` is already in
/// the native subset (it is also a heap-read on a handle param), so the struct pass
/// only needs to additionally accept `MakeStruct` inside the region.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_make_struct_op(instr: &RegInstr) -> bool {
    matches!(instr, RegInstr::MakeStruct { .. })
}

/// J3 (escape analysis + scalar replacement). Identify `Option` registers that are
/// NON-ESCAPING with a *scalar* payload and rewrite the function's `RegInstr` code
/// so each such Option is dissolved into two scalar registers — `tag` (Bool,
/// true=Some / false=None) and `payload` — leaving only native-subset ops (so the
/// function then compiles through the existing native path with no heap allocation).
///
/// Returns `Some((code, n_regs, payload_regs))` when the function contains either no
/// Option ops (code returned essentially unchanged) OR only scalar-replaceable ones.
/// `payload_regs` are the freshly-allocated payload registers, which the caller must
/// verify (post type-inference) are scalar (Int/Float/Bool) — a Handle/flat payload
/// is a heap value and disqualifies the function.
///
/// Returns `None` (⇒ leave the WHOLE function on its current interpreter path, never
/// partially transformed) if the function has any Option op that is NOT scalar-
/// replaceable. Conservative: any unrecognized use of an Option register, or any
/// non-subset / non-Option instruction in the body, makes it escaping.
///
/// Non-escaping criterion for an Option register `R` (defined by `MakeSome`/
/// `LoadNone`, transitively including `Move`-aliases): EVERY use of `R` is one of
/// `MatchOption{src:R}`, `UnwrapSome{src:R}`, `TryResult{src:R}`, or `Move{src:R}`
/// (whose dst is itself an Option register); `R` is never a value operand of
/// anything else; and `R`'s only definitions are `MakeSome`/`LoadNone`/`Move`-
/// from-Option. A `MakeSome` payload that is itself an Option register is
/// non-scalar ⇒ escaping.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_options(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<usize>)> {
    let reachable = native_reachable_instructions(code);

    // Fast path: no Option ops at all — nothing to do, no payload regs to verify.
    if !code
        .iter()
        .enumerate()
        .any(|(i, instr)| reachable[i] && is_option_op(instr))
    {
        // Identity ip-map: transformed code == original code, so each transformed
        // ip maps to itself.
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, Vec::new(), ip_map));
    }

    // Every reachable instruction must be either in the native subset or one of the
    // four Option ops; anything else makes the function ineligible anyway (and would
    // also defeat the read-enumeration below). Bail (interpreter path) if not.
    for (i, instr) in code.iter().enumerate() {
        if reachable[i]
            && !native_subset_instruction(instr)
            && !is_option_op(instr)
            && !matches!(instr, RegInstr::TryResult { .. })
        {
            return None;
        }
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, 0, code.len())?;

    // OPT = the set of registers that carry an Option value. Seed with every
    // `MakeSome`/`LoadNone` destination, then close under `Move` aliasing
    // (`Move{dst,src}` with `src` ∈ OPT ⇒ `dst` ∈ OPT).
    let mut opt = vec![false; n_regs];
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        match instr {
            RegInstr::MakeSome { dst, .. } | RegInstr::LoadNone { dst } => opt[*dst] = true,
            _ => {}
        }
    }
    analysis.close_reachable_move_aliases(code, &mut opt)?;

    // Validate: every OPT register is only ever DEFINED by `MakeSome`/`LoadNone`/
    // `Move`-from-OPT, and only ever USED by the recognized consumers. Any other
    // definition or use ⇒ escaping ⇒ bail (leave the whole function on its path).
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        match instr {
            // Recognized definitions of OPT registers.
            RegInstr::LoadNone { dst } if opt[*dst] => {}
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                // The payload must be a scalar — an Option payload is non-scalar.
                if opt[*value] {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if opt[*dst] => {
                // A Move that DEFINES an OPT register must copy from an OPT register
                // (pure alias). `src` ∈ OPT by the fixpoint unless `dst` was seeded
                // by a non-Move def and is also Move-assigned a non-Option — reject.
                if !opt[*src] {
                    return None;
                }
            }
            // Recognized uses (consumers) of OPT registers — fine.
            RegInstr::MatchOption { src, .. } if opt[*src] => {}
            RegInstr::UnwrapSome { src, dst } if opt[*src] => {
                // The unwrapped payload `dst` must NOT itself be an Option register
                // (would mean a non-scalar payload slipped through).
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if opt[*src] => {
                // A Move that READS an OPT register: its dst is in OPT (fixpoint), so
                // it is a recognized alias and handled by the def arm above.
            }
            // Any OTHER instruction must not touch an OPT register at all.
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| opt[r]) {
                    return None; // an OPT register escapes into a non-recognized use
                }
                // It also must not (re)define an OPT register through an
                // unrecognized destination. The only writers of OPT registers are
                // handled above; a non-recognized instruction whose dst is an OPT
                // register would mean the register is not purely an Option, so bail.
                if let RegInstr::UnwrapSome { dst, .. }
                | RegInstr::MakeSome { dst, .. }
                | RegInstr::LoadNone { dst } = other
                {
                    if opt[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // Allocate two fresh registers per OPT register: tag (Int) and payload.
    let mut tag_reg = vec![0usize; n_regs];
    let mut payload_reg = vec![0usize; n_regs];
    let mut payload_regs: Vec<usize> = Vec::new();
    let mut next_reg = n_regs;
    for (reg, is_opt) in opt.iter().enumerate() {
        if *is_opt {
            tag_reg[reg] = next_reg;
            payload_reg[reg] = next_reg + 1;
            payload_regs.push(next_reg + 1);
            next_reg += 2;
        }
    }

    // Rewrite, remapping jump targets (indices shift because `MakeSome`/
    // `MatchOption` expand to two instructions). Same index-map + fixup discipline
    // as `native_inline_leaf_calls`.
    enum Fix {
        Target(usize),
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                // tag = Some (Bool true). The tag is a `Bool` (stored as i64 0/1) so
                // it can drive a native `JumpIfBool` directly; `LoadBool true` is the
                // native-subset form of "tag = 1".
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *value,
                });
            }
            RegInstr::LoadNone { dst } if opt[*dst] => {
                // None: tag = false. Payload left undefined; the None arm never reads it.
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: false,
                });
            }
            RegInstr::Move { dst, src } if opt[*dst] => {
                // Alias copy: move both tag and payload.
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } if opt[*src] => {
                // tag true (Some) → some_ip; else fall to an explicit jump to none_ip.
                fixups.push((new_code.len(), Fix::Target(*some_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*none_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapSome { dst, src } if opt[*src] => {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                let some_target = new_code.len() + 2;
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: some_target,
                });
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            // Copy-through, remapping any jump target (these never touch OPT regs).
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, Fix::Target(t)) in fixups {
        let target = index_map[t];
        match &mut new_code[pos] {
            RegInstr::Jump { target: dst }
            | RegInstr::JumpIfBool { target: dst, .. }
            | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
            _ => {}
        }
    }
    // Inverse ip-map: `ip_map[transformed_ip] = original_ip`. Each original
    // instruction `i` expanded to the consecutive transformed range
    // `[index_map[i], index_map[i+1])` (or `..new_code.len()` for the last), so
    // every transformed ip in that range maps back to `i`. Each original
    // instruction always emits at least one transformed instruction (the rewrite
    // loop pushes unconditionally), so the ranges tile `0..new_code.len()`
    // exactly. A rewritten Option op (e.g. `MatchOption` → `JumpIfBool;Jump`)
    // thus maps every fragment to the original op's index; copy-through
    // instructions map one-to-one.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, payload_regs, ip_map))
}

/// The interned `Ok { value }` variant layout, used by the combinator-expansion
/// pass when lowering `Result.map` into `MakeVariant{Ok,[mapped]}` (the same shape
/// the lowerer and the interpreter's `value_ok` produce).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn result_ok_layout() -> Rc<crate::vm_value::TypeLayout> {
    crate::vm_value::intern_layout(Rc::from("Ok"), vec![Rc::from("value")])
}

/// The interned `Err(value)` layout — the `Err` analog of [`result_ok_layout`] (the
/// lowerer builds both `Ok` and `Err` with a single field named `value`). Used to
/// reconstruct a live-after two-armed Result's `Err` arm at OSR-exit.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn result_err_layout() -> Rc<crate::vm_value::TypeLayout> {
    crate::vm_value::intern_layout(Rc::from("Err"), vec![Rc::from("value")])
}

/// Whether `intrinsic` is one of the six Option/Result combinator intrinsics that
/// the combinator-expansion pass (deopt-before-heap Slice 2) lowers into primitive
/// match/construct form with the mapper closure inlined. Recognition now reads the
/// central [`intrinsic_descriptor`] table's `combinator_kind`; the pass keeps the
/// exact per-kind match/construct lowering.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn combinator_intrinsic_kind(
    intrinsic: RegIntrinsic,
) -> Option<CombinatorKind> {
    intrinsic_descriptor(intrinsic).combinator_kind
}

#[cfg(feature = "native-jit")]
impl CombinatorKind {
    /// Whether `arg[1]` is a mapper *closure* (map/and_then) rather than a scalar
    /// default value (unwrap_or).
    fn has_mapper(self) -> bool {
        !matches!(
            self,
            CombinatorKind::OptionUnwrapOr | CombinatorKind::ResultUnwrapOr
        )
    }
}

/// OSR × J3 (deopt-before-heap, Slice 2): expand the six Option/Result combinator
/// intrinsics (`Option.map`/`and_then`/`unwrap_or`, `Result.map`/`and_then`/
/// `unwrap_or`) that appear inside the loop region `[header, exit)` into primitive
/// match/construct form, leaving the mapper closure call as an in-region
/// `CallClosure{closure: mapper_reg, args:[payload]}`. The downstream
/// [`native_inline_leaf_calls`] then SINKS each loop-local mapper `MakeClosure`
/// (inlining its body), and the Option / Result scalar-replacement passes dissolve
/// the per-iteration Option/Result values — so the combinator chain becomes pure
/// scalar code and the loop OSRs.
///
/// The lowering replicates the interpreter's exact combinator semantics
/// (`exec_option_intrinsics` / `exec_result_intrinsics`):
/// - `OptionMap(o, f)`     → `match o { Some(v) => MakeSome(f(v)), None => LoadNone }`
/// - `OptionAndThen(o, f)` → `match o { Some(v) => f(v) /*already Option*/, None => LoadNone }`
/// - `OptionUnwrapOr(o,d)` → `match o { Some(v) => v, None => d }`
/// - `ResultMap(r, f)`     → `match r { Ok(v) => MakeVariant{Ok,[f(v)]}, Err(_) => Bail }`
/// - `ResultAndThen(r, f)` → `match r { Ok(v) => f(r) /*already Result*/, Err(_) => Bail }`
/// - `ResultUnwrapOr(r,d)` → `match r { Ok(v) => v, Err(_) => d }`
///
/// The `Result` `Err` arm rebuilds a HEAP `Err` in the interpreter; building heap is
/// forbidden on the native path (Exec Spec §7.2), so that arm becomes a native
/// `Bail` (a `RuntimeError` sentinel — identical to the Slice-1 cold-arm splice).
/// Because the inlined `checked` leaf's own `Err` arm already bailed, the `Result`
/// reaching `ResultMap`/`ResultAndThen` is statically always-`Ok` after inlining,
/// so Slice-1 Result scalar-replacement dissolves it (the `Err` arm goes dead). The
/// `Ok` arm constructs `MakeVariant{Ok,[scalar]}`, exactly the shape Result-SR
/// dissolves. `ResultUnwrapOr`'s `Err` arm only moves the scalar default (no heap),
/// so it need not bail; but a live heap `Err` reaching it would have been dissolved
/// upstream — if not, the surrounding Result-SR/escape gates bail.
///
/// Conservative — returns `None` (⇒ no OSR; the loop stays on the interpreter)
/// when ANY in-region combinator's mapper is not a loop-local, native-inlinable
/// `MakeClosure` (a stored/param `Fn`, a capturing-with-heap closure, …): such a
/// mapper cannot be sunk/inlined, so leaving a bare `CallClosure` would block the
/// native subset anyway. A non-combinator body returns the code unchanged with an
/// identity ip-map (byte-for-byte the old path). Escape / dead-at-boundary of the
/// produced Option/Result values is enforced by the downstream SR passes, exactly
/// as for a hand-written `match`.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→
/// original `ip_map` discipline as the sibling region passes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_expand_option_result_combinators_in_region(
    unit: &RegUnit,
    _func: &RegFunction,
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no combinator intrinsic in the region ⇒ identity transform.
    let has_combinator = (header..exit).any(|i| {
        matches!(
            &code[i],
            RegInstr::CallIntrinsic { intrinsic, .. } | RegInstr::CallTypedIntrinsic { intrinsic, .. }
                if combinator_intrinsic_kind(*intrinsic).is_some()
        )
    });
    if !has_combinator {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // For a mapper-bearing combinator, the mapper arg register MUST be the dst of an
    // in-region `MakeClosure` whose callee is native-inlinable at arity 1 (the
    // single payload argument). The closure is loop-local and only fed into the
    // combinator, so `loop_local_sinkable_closures` will sink + inline it. If we
    // cannot prove this, bail the whole expansion (no OSR): a bare `CallClosure`
    // would block the native subset and a non-inlinable mapper is genuinely opaque.
    let mapper_callee = |mapper_reg: usize| -> Option<usize> {
        // Single defining `MakeClosure` in-region (no copy-Move forwarding needed for
        // the literal-closure shape the lowerer emits; a forwarded/redefined mapper
        // conservatively fails this lookup and bails).
        let mut found: Option<usize> = None;
        for (mi, instr) in code.iter().enumerate() {
            if let RegInstr::MakeClosure { dst, function, .. } = instr {
                if *dst == mapper_reg {
                    if !in_region(mi) || found.is_some() {
                        return None;
                    }
                    found = Some(*function);
                }
            }
        }
        let k = found?;
        let callee = unit.functions.get(k)?;
        // The mapper is invoked with exactly one argument (the matched payload).
        let inlinable = if callee.captures == 0 {
            native_callee_inlinable_j3(callee, 1)
        } else {
            // A capturing mapper needs all-scalar captures to be sinkable; we cannot
            // see the profile's `captures_all_scalar` bit here, so accept only
            // captureless mappers (the common literal `|v| {...}` shape). A capturing
            // mapper bails the expansion (conservative ⇒ no OSR).
            false
        };
        if !inlinable {
            return None;
        }
        Some(k)
    };

    // Validate every in-region combinator up front; bail the whole pass on the first
    // one we cannot expand. Collect the (operand, mapper/default) regs for the rewrite.
    for i in header..exit {
        let (intrinsic, args) = match &code[i] {
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => (*intrinsic, args),
            _ => continue,
        };
        let Some(kind) = combinator_intrinsic_kind(intrinsic) else {
            continue;
        };
        if args.len() != 2 {
            return None;
        }
        if kind.has_mapper() {
            mapper_callee(args[1])?;
        }
    }

    // Rewrite the WHOLE code: each in-region combinator becomes a primitive
    // match/construct fragment (fresh temp + payload regs allocated above `n_regs`);
    // everything else copies through with jump/match targets remapped through the
    // index map (identical discipline to the sibling SR passes).
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        // A forward jump to the join point AFTER the combinator fragment at original
        // index `orig`: resolved to the start of the next original instruction.
        JoinAfter(usize),
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len() + 16);
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    let mut next_reg = n_regs;

    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        let combinator = if region {
            match instr {
                RegInstr::CallIntrinsic {
                    intrinsic,
                    args,
                    dst,
                }
                | RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    args,
                    dst,
                    ..
                } => combinator_intrinsic_kind(*intrinsic).map(|k| (k, args.clone(), *dst)),
                _ => None,
            }
        } else {
            None
        };
        if let Some((kind, args, dst)) = combinator {
            let operand = args[0];
            let other = args[1]; // mapper closure OR scalar default
            match kind {
                CombinatorKind::OptionMap
                | CombinatorKind::OptionAndThen
                | CombinatorKind::OptionUnwrapOr => {
                    // match operand { Some(v) => <some arm> ; None => <none arm> }
                    let payload = next_reg;
                    next_reg += 1;
                    // MatchOption operand → some_ip(next) / none_ip(patched)
                    let match_pos = new_code.len();
                    new_code.push(RegInstr::MatchOption {
                        src: operand,
                        some_ip: 0,
                        none_ip: 0,
                    });
                    // --- Some arm (falls through from the match's some_ip) ---
                    let some_ip = new_code.len();
                    new_code.push(RegInstr::UnwrapSome {
                        dst: payload,
                        src: operand,
                    });
                    match kind {
                        CombinatorKind::OptionMap => {
                            // dst = Some(mapper(payload))
                            let mapped = next_reg;
                            next_reg += 1;
                            new_code.push(RegInstr::CallClosure {
                                dst: mapped,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                            new_code.push(RegInstr::MakeSome { dst, value: mapped });
                        }
                        CombinatorKind::OptionAndThen => {
                            // dst = mapper(payload) (already an Option)
                            new_code.push(RegInstr::CallClosure {
                                dst,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                        }
                        CombinatorKind::OptionUnwrapOr => {
                            // dst = payload (the Some value)
                            new_code.push(RegInstr::Move { dst, src: payload });
                        }
                        _ => unreachable!(),
                    }
                    // jump to join (after this combinator)
                    fixups.push((new_code.len(), Fix::JoinAfter(i)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    // --- None arm ---
                    let none_ip = new_code.len();
                    match kind {
                        CombinatorKind::OptionMap | CombinatorKind::OptionAndThen => {
                            new_code.push(RegInstr::LoadNone { dst });
                        }
                        CombinatorKind::OptionUnwrapOr => {
                            new_code.push(RegInstr::Move { dst, src: other });
                        }
                        _ => unreachable!(),
                    }
                    // (falls through to join)
                    if let RegInstr::MatchOption {
                        some_ip: s,
                        none_ip: nn,
                        ..
                    } = &mut new_code[match_pos]
                    {
                        *s = some_ip;
                        *nn = none_ip;
                    }
                }
                CombinatorKind::ResultMap
                | CombinatorKind::ResultAndThen
                | CombinatorKind::ResultUnwrapOr => {
                    let payload = next_reg;
                    next_reg += 1;
                    let match_pos = new_code.len();
                    new_code.push(RegInstr::MatchResult {
                        src: operand,
                        ok_ip: 0,
                        err_ip: 0,
                    });
                    // --- Ok arm ---
                    let ok_ip = new_code.len();
                    new_code.push(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src: operand,
                        expected: "Ok".to_string(),
                    });
                    match kind {
                        CombinatorKind::ResultMap => {
                            // dst = Ok(mapper(payload))
                            let mapped = next_reg;
                            next_reg += 1;
                            new_code.push(RegInstr::CallClosure {
                                dst: mapped,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                            new_code.push(RegInstr::MakeVariant {
                                dst,
                                layout: result_ok_layout(),
                                fields: vec![("value".to_string(), mapped)],
                            });
                        }
                        CombinatorKind::ResultAndThen => {
                            // dst = mapper(payload) (already a Result)
                            new_code.push(RegInstr::CallClosure {
                                dst,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                        }
                        CombinatorKind::ResultUnwrapOr => {
                            new_code.push(RegInstr::Move { dst, src: payload });
                        }
                        _ => unreachable!(),
                    }
                    fixups.push((new_code.len(), Fix::JoinAfter(i)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    // --- Err arm ---
                    let err_ip = new_code.len();
                    match kind {
                        CombinatorKind::ResultMap | CombinatorKind::ResultAndThen => {
                            // Heap Err rebuild ⇒ native Bail (Slice-1 cold-arm path).
                            new_code.push(RegInstr::RuntimeError {
                                message: String::new(),
                            });
                        }
                        CombinatorKind::ResultUnwrapOr => {
                            // dst = default (scalar; no heap build).
                            new_code.push(RegInstr::Move { dst, src: other });
                        }
                        _ => unreachable!(),
                    }
                    if let RegInstr::MatchResult {
                        ok_ip: o,
                        err_ip: e,
                        ..
                    } = &mut new_code[match_pos]
                    {
                        *o = ok_ip;
                        *e = err_ip;
                    }
                }
            }
            continue;
        }
        // Copy-through, remapping jump/match targets (same as the SR passes).
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::JoinAfter(orig) => {
                // The instruction AFTER the combinator at `orig` (its successor); for
                // the last instruction this is one-past-the-end (handled below).
                let target = if orig + 1 < code.len() {
                    index_map[orig + 1]
                } else {
                    new_code.len()
                };
                if let RegInstr::Jump { target: dst } = &mut new_code[pos] {
                    *dst = target;
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map: every fragment of a combinator at original index `i` maps back
    // to `i` (a deopt inside the expanded fragment resumes by re-running the original
    // combinator on the interpreter); copy-through maps one-to-one.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × J3 (string length-law folding): a non-escaping string register `s`
/// inside the loop region `[header, exit)` whose EVERY use is `String.len(s)`,
/// another foldable producer's operand, or a `Move` to a foldable register, can
/// have its allocation DELETED — every `String.len(s)` is replaced by arithmetic
/// on operand lengths, and the producer instruction(s) are dropped. This stays
/// READ-ONLY (it removes allocations, never performs a heap write — Exec Spec
/// §7.2 holds), so a length-only string loop becomes pure-scalar and OSRs.
///
/// VERIFIED length laws (against the interpreter's exact `String` semantics):
/// - `String.len` is the BYTE length (`str::len`, see [`RegIntrinsic::StringLen`]).
/// - `String.concat` is byte concatenation (see [`RegInstr::StringConcat`]), so
///   `len(concat(a,b)) = len(a) + len(b)` exactly, REGARDLESS of encoding.
/// - `String.from_int(k)` is `i64::to_string()`: ASCII decimal digits with a
///   leading `-` for negatives, `"0"` for zero, all bytes 1-wide. Its byte length
///   is the decimal-digit count (`+1` for the sign when `k < 0`), computed natively
///   by a forward branch ladder that handles `0`, negatives, and `i64::MIN` (which
///   cannot be negated) by comparing `k` directly against ± powers of ten.
/// - `String.slice(s, start, n)` (see [`string_slice_range`]) clamps to CHAR
///   boundaries in BYTE units: `bs = clamp_cb(s, max(start,0))`,
///   `be = clamp_cb(s, min(bs + max(n,0), len(s)))`, result byte-length `be - bs`.
///   The char-boundary clamp depends on the actual bytes of `s`, so the law is only
///   provable when `s` is ASCII (every byte is a boundary ⇒ clamp is identity):
///   `len = min(min(max(start,0), L) + max(n,0), L) - min(max(start,0), L)` with
///   `L = len(s)`. A slice of a NON-ASCII (unprovably-ASCII) string ⇒ NOT foldable.
/// - `LoadString` / string `Move`: constant byte length / alias of the source.
///
/// Conservative bails (when unsure REJECT ⇒ no OSR, never unsound): any escaping
/// string use (stored / returned / captured / compared / passed to a non-`len`
/// intrinsic / live at a loop boundary); a `String.slice` of an unprovably-ASCII
/// string; a `String.len` whose source is not a fully-foldable producer. A leaf
/// non-foldable `String.len` is simply left un-folded — the `StringLen` host helper
/// IS native-subset (a plain `String.len` loop OSRs), so it runs as a host call rather
/// than blocking OSR; this pass only declines to FOLD it. `RegFootprint::All` ⇒ bail.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→
/// original `ip_map` discipline as the sibling region passes. Identity (no
/// foldable `String.len`) ⇒ code unchanged with an identity ip-map.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_string_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // The only thing this pass targets is an in-region `String.len`. Without one,
    // there is nothing to fold ⇒ identity (plain OSR, byte-for-byte the old path).
    // The length-query *classification* reads the central registry's
    // `string_fold_role`; the single-arg shape check stays here.
    let is_string_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::LengthQuery)
        )
    };
    if !(header..exit).any(|i| is_string_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // Classify each in-region string producer. A producer register is a candidate
    // iff it is defined EXACTLY ONCE in-region by a foldable op and never defined
    // out-of-region. `ascii` records whether the produced string is provably ASCII
    // (needed only for the slice length law).
    #[derive(Clone)]
    enum Producer {
        Literal {
            len: i64,
            ascii: bool,
        },
        FromInt {
            src: usize,
        },
        Concat {
            left: usize,
            right: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        }, // string `Move`
    }
    let mut producer: Vec<Option<Producer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];

    let slice_args = |args: &[usize]| -> Option<(usize, usize, usize)> {
        if args.len() == 3 {
            Some((args[0], args[1], args[2]))
        } else {
            None
        }
    };
    for i in header..exit {
        let dst_prod: Option<(usize, Producer)> = match &code[i] {
            RegInstr::LoadString { dst, value } => Some((
                *dst,
                Producer::Literal {
                    len: value.len() as i64,
                    ascii: value.is_ascii(),
                },
            )),
            RegInstr::StringConcat { dst, left, right } => Some((
                *dst,
                Producer::Concat {
                    left: *left,
                    right: *right,
                },
            )),
            // Foldable string *producer* intrinsics — recognized via the central
            // registry's `string_fold_role`; the per-role operand extraction and the
            // length laws stay here. (`String.from_int` is recognized only in the
            // untyped `CallIntrinsic` form, exactly as before; `String.slice` in both
            // the untyped and typed forms.)
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerFromInt)
                && args.len() == 1 =>
            {
                Some((*dst, Producer::FromInt { src: args[0] }))
            }
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerSlice) =>
            {
                slice_args(args)
                    .map(|(src, start, len)| (*dst, Producer::Slice { src, start, len }))
            }
            // A `Move` whose src is a (candidate) string is a potential alias; we
            // only mark it a producer if the src is itself a string producer (below,
            // after all defs are seen). Record it provisionally as an Alias.
            RegInstr::Move { dst, src } => Some((*dst, Producer::Alias { src: *src })),
            _ => None,
        };
        if let Some((dst, prod)) = dst_prod {
            if dst >= n_regs {
                return None;
            }
            if analysis.region_def_count(dst)? > 1 {
                multiply_defined[dst] = true;
            }
            producer[dst] = Some(prod);
        }
    }

    // A register defined out-of-region, or defined more than once in-region, cannot
    // be a sound single-producer string ⇒ drop it from the candidate set. (Out-of-
    // region defs would change the value the loop observes.)
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in 0..n_regs {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // Resolve foldability with a fixpoint: a producer is FOLDABLE iff all the
    // operands its length law needs are themselves foldable (or, for slice/from_int,
    // satisfy the ASCII / integer requirements). `Move`/`Concat`/`Slice` of a non-
    // foldable string is itself non-foldable. `ascii` is tracked alongside.
    let mut foldable = vec![false; n_regs];
    let mut ascii = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..n_regs {
            if foldable[r] {
                continue;
            }
            let Some(prod) = &producer[r] else { continue };
            let (ok, is_ascii) = match prod {
                Producer::Literal { ascii, .. } => (true, *ascii),
                // `from_int` is always ASCII; its operand is an Int (not a string),
                // so no string dependency.
                Producer::FromInt { .. } => (true, true),
                Producer::Concat { left, right } => {
                    let ok =
                        *left < n_regs && *right < n_regs && foldable[*left] && foldable[*right];
                    (ok, ok && ascii[*left] && ascii[*right])
                }
                Producer::Slice { src, .. } => {
                    // The slice length law needs the SOURCE to be provably ASCII
                    // (so the char-boundary clamp is the identity). A slice of an
                    // ASCII string is itself ASCII.
                    let ok = *src < n_regs && foldable[*src] && ascii[*src];
                    (ok, ok)
                }
                Producer::Alias { src } => {
                    let ok = *src < n_regs && foldable[*src];
                    (ok, ok && ascii[*src])
                }
            };
            if ok && !foldable[r] {
                foldable[r] = true;
                ascii[r] = is_ascii;
                changed = true;
            }
        }
    }

    // STRING = registers we (provisionally) treat as string-valued and intend to
    // dissolve: every foldable producer register. For soundness we now require that
    // EVERY use of a STRING register is itself foldable — i.e. an operand of another
    // foldable producer, a `Move` to a foldable register, or a `String.len`. Any
    // other in-region use, or ANY out-of-region use, ESCAPES ⇒ that register cannot
    // be dissolved. We don't need partial dissolution: if a `String.len` source is
    // foldable but the foldable register also escapes elsewhere, the producer must
    // stay live, so we cannot delete it — bail that whole register out of `foldable`
    // and re-resolve, then finally require every in-region `String.len` to be
    // foldable (else bail the pass: a live `StringLen` is not native-subset).
    //
    // Compute "escapes": a foldable register read by an instruction that is neither
    // (a) a foldable producer consuming it as a string operand, nor (b) a
    // `String.len`. Iterate to a fixpoint (dropping an escaping register can make a
    // consumer's operand non-foldable, propagating).
    loop {
        let mut escaped = vec![false; n_regs];
        // Out-of-region reads of any foldable register ⇒ escape.
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        // In-region uses: each read of a foldable register must be a sanctioned
        // string consumer.
        for i in header..exit {
            match &code[i] {
                // Sanctioned: foldable producers consuming foldable string operands.
                RegInstr::StringConcat { dst, left, right } if foldable[*dst] => {
                    // operands consumed as strings — fine (handled by being foldable)
                    let _ = (left, right);
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && args.len() == 3
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::ProducerSlice) =>
                {
                    // start/len (args[1], args[2]) are Int operands, not strings.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    // alias of a foldable string into a foldable register — fine.
                }
                // Sanctioned: the query itself.
                _ if is_string_len(&code[i]) => {
                    // its single arg is consumed as a string — fine.
                }
                // Any other instruction: any foldable register it reads escapes.
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => {
                        for r in rs {
                            if r < n_regs && foldable[r] {
                                escaped[r] = true;
                            }
                        }
                    }
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        // Drop escaped registers and re-resolve foldability (a dropped operand can
        // un-fold its consumers).
        for r in 0..n_regs {
            if escaped[r] {
                foldable[r] = false;
                ascii[r] = false;
            }
        }
        let mut changed2 = true;
        while changed2 {
            changed2 = false;
            for r in 0..n_regs {
                if !foldable[r] {
                    continue;
                }
                let Some(prod) = &producer[r] else { continue };
                let still = match prod {
                    Producer::Literal { .. } | Producer::FromInt { .. } => true,
                    Producer::Concat { left, right } => foldable[*left] && foldable[*right],
                    Producer::Slice { src, .. } => foldable[*src] && ascii[*src],
                    Producer::Alias { src } => foldable[*src],
                };
                if !still {
                    foldable[r] = false;
                    ascii[r] = false;
                    changed2 = true;
                }
            }
        }
    }

    // A `String.len` whose source is NOT foldable is left UNCHANGED: the real
    // `StringLen` intrinsic IS in the native subset (it lowers to the host `string_len`
    // helper), so a surviving `StringLen` does NOT block OSR — only foldable-source
    // lengths are dissolved below (the rewrite arm guards on `foldable[args[0]]`). This
    // is what lets an EXPANDED-path loop (e.g. a two-armed `Result<String,_>` whose arms
    // call `String.len` on a live heap payload) OSR instead of declining the whole loop.

    // Nothing dissolvable after escape analysis ⇒ identity (no fold): every in-region
    // `String.len` had a non-foldable source (all left in place), or there was no
    // foldable producer at all.
    if !foldable.iter().any(|&f| f) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // Allocate a fresh Int `len_reg` per foldable string register: it will hold that
    // string's byte length, computed at the producer site. `from_int` needs scratch
    // registers for its digit-count ladder; reserve them lazily.
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for r in 0..n_regs {
        if foldable[r] {
            len_reg[r] = next_reg;
            next_reg += 1;
        }
    }

    // Emit the byte-length computation for `from_int(src)` into `out`, appending to
    // `out_code` (using absolute new-code indices for intra-fragment forward jumps,
    // back-patched after emission). Allocates scratch registers from `next_reg`.
    //
    // Matches `i64::to_string().len()`: `1` for `0`; `digits(k)` for `k > 0`;
    // `1 + digits(|k|)` for `k < 0`. Avoids negating `i64::MIN` by comparing `k`
    // directly against negative powers of ten.
    fn emit_from_int_len(out_code: &mut Vec<RegInstr>, out: usize, k: usize, next_reg: &mut usize) {
        // result accumulator is `out`. Strategy:
        //   if k >= 0:  out = pos_digits(k)
        //   else:       out = 1 + neg_digits(k)
        // pos_digits(k): 1 + count of thresholds {10,100,...,1e18} that k >= t.
        // neg_digits(k): 1 + count of thresholds {-10,...,-1e18} that k <= t.
        // Both are computed branchlessly-by-cascade using comparisons that write a
        // Bool then a conditional add — but Bool isn't Int-addable, so we instead use
        // a forward branch ladder: for k >= 0, test largest threshold first and on
        // the first hit LoadInt the digit count and Jump to the merge.
        let zero = *next_reg;
        *next_reg += 1;
        let thr = *next_reg;
        *next_reg += 1;
        // Positive powers of ten (10^1 .. 10^18); 10^19 overflows i64, so 19-digit
        // positives (>= 10^18) are the final else of the positive ladder.
        const POW10: [i64; 18] = [
            10,
            100,
            1_000,
            10_000,
            100_000,
            1_000_000,
            10_000_000,
            100_000_000,
            1_000_000_000,
            10_000_000_000,
            100_000_000_000,
            1_000_000_000_000,
            10_000_000_000_000,
            100_000_000_000_000,
            1_000_000_000_000_000,
            10_000_000_000_000_000,
            100_000_000_000_000_000,
            1_000_000_000_000_000_000,
        ];
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // Branch: if k < 0 jump to neg-ladder.
        let neg_start_patch = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: k,
            rhs: zero,
            op: RegIntCompare::Less,
            expected: true,
            target: 0, // back-patched
        });
        // --- positive (and zero) ladder: emit largest threshold first ---
        // For d in 19..=2: if k >= 10^(d-1) -> out = d; Jump merge.
        let mut to_merge: Vec<usize> = Vec::new();
        for d in (2..=19usize).rev() {
            let t = POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            // if k >= t -> set out=d, jump merge
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::GreaterEqual,
                expected: false, // if NOT (k>=t) skip the assignment
                target: 0,       // back-patched to the next threshold test
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            to_merge.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> merge
            // back-patch skip to here (next threshold test / final else)
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        // positive final else: k in [0,10) -> 1 digit.
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        to_merge.push(out_code.len());
        out_code.push(RegInstr::Jump { target: 0 }); // -> merge
        // --- negative ladder ---
        let neg_start = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[neg_start_patch] {
            *target = neg_start;
        }
        // out (magnitude digits) then +1 for sign. For d in 19..=2: if k <= -10^(d-1)
        // -> magnitude d. Final else -> magnitude 1.
        let mut neg_to_add: Vec<usize> = Vec::new();
        for d in (2..=19usize).rev() {
            let t = -POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::LessEqual,
                expected: false, // if NOT (k<=t) skip
                target: 0,
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            neg_to_add.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> add-sign
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        // fallthrough to add-sign
        let add_sign = out_code.len();
        for p in neg_to_add {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = add_sign;
            }
        }
        // out = out + 1 (sign byte). Reuse `thr` as the constant 1.
        out_code.push(RegInstr::LoadInt { dst: thr, value: 1 });
        out_code.push(RegInstr::AddInt {
            dst: out,
            lhs: out,
            rhs: thr,
        });
        // fallthrough to merge
        let merge = out_code.len();
        for p in to_merge {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = merge;
            }
        }
    }

    // Emit the slice byte-length law for an ASCII source into `out_code`, writing
    // `out`. `l_src` is the source's length register; `start`,`len` the Int operands.
    //   sc = max(start,0); s_clamp = min(sc, L); ec = s_clamp + max(len,0);
    //   e_clamp = min(ec, L); out = e_clamp - s_clamp.
    fn emit_slice_len(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let sclamp = *next_reg + 2;
        let lc = *next_reg + 3;
        let ec = *next_reg + 4;
        *next_reg += 5;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // sc = max(start, 0)
        emit_max(out_code, sc, start, zero, next_reg);
        // s_clamp = min(sc, L)
        emit_min(out_code, sclamp, sc, l_src, next_reg);
        // lc = max(len, 0)
        emit_max(out_code, lc, len, zero, next_reg);
        // ec = s_clamp + lc
        out_code.push(RegInstr::AddInt {
            dst: ec,
            lhs: sclamp,
            rhs: lc,
        });
        // e_clamp = min(ec, L)  -> reuse `ec`
        emit_min(out_code, ec, ec, l_src, next_reg);
        // out = e_clamp - s_clamp
        out_code.push(RegInstr::SubInt {
            dst: out,
            lhs: ec,
            rhs: sclamp,
        });
    }

    // out = max(a, b): if a >= b -> out=a else out=b (forward branch).
    fn emit_max(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // out = min(a, b): if a <= b -> out=a else out=b.
    fn emit_min(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::LessEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // Rewrite the whole stream. In-region foldable producers are replaced by the
    // length computation writing `len_reg[dst]` (the original heap allocation is
    // DELETED); each `String.len(s)` becomes `Move(dst, len_reg[s])`. Everything
    // else is copied through, remapping inter-instruction jump/match targets through
    // `index_map`. Intra-fragment jumps emitted by the helpers above already carry
    // absolute new-code positions (back-patched at emit time) and must NOT be
    // remapped, so producer fragments are spliced AFTER recording `index_map[i]` and
    // the fragment's internal jumps are left untouched.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        MapGet { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        // A foldable producer in-region: emit its length computation, drop the alloc.
        if region {
            let folded = match instr {
                RegInstr::LoadString { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Literal { len, .. }) = &producer[*dst] {
                        new_code.push(RegInstr::LoadInt {
                            dst: len_reg[*dst],
                            value: *len,
                        });
                    }
                    true
                }
                RegInstr::StringConcat { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Concat { left, right }) = &producer[*dst] {
                        new_code.push(RegInstr::AddInt {
                            dst: len_reg[*dst],
                            lhs: len_reg[*left],
                            rhs: len_reg[*right],
                        });
                    }
                    true
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringFromInt,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::FromInt { src }) = &producer[*dst] {
                        emit_from_int_len(&mut new_code, len_reg[*dst], *src, &mut next_reg);
                    }
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::Slice { src, start, len }) = &producer[*dst] {
                        emit_slice_len(
                            &mut new_code,
                            len_reg[*dst],
                            len_reg[*src],
                            *start,
                            *len,
                            &mut next_reg,
                        );
                    }
                    true
                }
                _ if is_string_len(instr)
                    && matches!(instr,
                        RegInstr::CallIntrinsic { args, .. }
                        | RegInstr::CallTypedIntrinsic { args, .. }
                        if args[0] < n_regs && foldable[args[0]]) =>
                {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    new_code.push(RegInstr::Move {
                        dst,
                        src: len_reg[src],
                    });
                    true
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
        // Copy-through, remapping jump/match targets to the new index space.
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::MapGet {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = sa;
                        *err_ip = sb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = sa;
                        *else_ip = sb;
                    }
                    _ => {}
                }
            }
            Fix::MapGet { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map: every fragment instruction maps back to the producer's
    // original index (`String.len` → its own index; copy-through 1:1).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × J3 for BYTES LENGTH-LAW FOLDING (the read-only sibling of
/// [`native_string_length_fold_in_region`]): dissolve a non-escaping Bytes value built
/// ONLY to be measured (`Bytes.len` of `Bytes.slice`/`Bytes.from_string`/`Move`/a
/// constant-length source) into byte-length arithmetic, DELETING the now-dead Bytes
/// allocation. Read-only (no heap write; Exec Spec §7.2 holds), turning a length-only
/// Bytes loop into pure-scalar Int code the native subset accepts.
///
/// Why a separate pass from the String fold: Bytes are RAW bytes with NO char/grapheme
/// boundary, so the slice length law is the EXACT clamp arithmetic of [`bytes_slice`]
/// with NO ASCII gate — verified identical: `bytes_slice` does `s'=max(start,0); if
/// s'>=L {0} else { min(s'+max(len,0), L) - s' }`, which is precisely the
/// `emit_slice_len` law (`sc=max(start,0); sclamp=min(sc,L); ec=sclamp+max(len,0);
/// e=min(ec,L); out=e-sclamp` — for `sc>=L` it yields `L-L=0`). `Bytes.len` is
/// `value.len()` (raw byte count) and `Bytes.from_string(s)` is `s.as_bytes().len()`,
/// so a from-string's byte length equals the source String's byte length.
///
/// A length source may be (a) an in-region foldable Bytes producer, or (b) ANY register
/// whose byte length resolves to a COMPILE-TIME CONSTANT through its unique global def
/// (covers a loop-invariant `Bytes.from_string(<literal>)` defined before the loop —
/// the `bytes_scan` shape). The constant is materialized as an in-region `LoadInt` so it
/// is available after the native OSR entry (which lands AT the loop header, skipping any
/// pre-header transformed code). The constant trace requires the register be defined
/// EXACTLY ONCE in the whole function by a chain of immutable constant ops, so the
/// length is loop-invariant and exact.
///
/// Conservative bail: any escaping foldable Bytes register, any in-region `Bytes.len`
/// whose source does not resolve, or any unmodelled read/write footprint ⇒ `None`
/// (no fold; the interpreter runs the loop). A body with no foldable `Bytes.len`
/// returns the code unchanged with an identity ip-map (byte-for-byte the old path).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_bytes_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    let is_bytes_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::LengthQuery)
        )
    };
    // No in-region Bytes.len ⇒ nothing to fold ⇒ identity (plain OSR, byte-for-byte).
    if !(header..exit).any(|i| is_bytes_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    // Preserve this pass's old whole-function conservatism: an unknown write footprint
    // makes the unique-def tracer unverifiable, so the fold does not fire.
    analysis.global_def_counts.as_ref()?;

    // --- Constant byte-length tracer -------------------------------------------------
    // `const_len[r] = Some(L)` iff register `r` is defined EXACTLY ONCE in the whole
    // function by a chain of immutable constant ops whose byte length is the compile-
    // time constant `L`. Multiply-defined (or `All`-footprint) registers get `None`.
    //
    //   LoadString lit                 -> lit.as_bytes().len()      (== lit.len())
    //   Bytes.from_string(s)           -> const_len[s]
    //   Bytes.slice(s, start, len)     -> clamp(const_len[s], start, len)  [const args]
    //   Move(dst, src)                 -> const_len[src]
    //
    // `start`/`len` for a constant slice must themselves be constant Ints (LoadInt) for
    // the result to be a compile-time constant. Anything else ⇒ not a constant.
    // Single-def constant-Int values (for slice start/len constant args).
    let int_const = |r: usize| -> Option<i64> {
        let ip = analysis.single_def_ip_of(code, r)?;
        match &code[ip] {
            RegInstr::LoadInt { dst, value } if *dst == r => Some(*value),
            _ => None,
        }
    };
    // Resolve a register's constant byte length with a depth-bounded trace over its
    // unique def. Depth bound guards against any pathological chain; immutability is
    // guaranteed by `analysis.global_def_count(r) == 1` at each hop.
    fn const_byte_len(
        r: usize,
        depth: usize,
        code: &[RegInstr],
        n_regs: usize,
        analysis: &NativeRegionAnalysis,
        int_const: &dyn Fn(usize) -> Option<i64>,
    ) -> Option<i64> {
        if depth == 0 || r >= n_regs || analysis.global_def_count(r)? != 1 {
            return None;
        }
        let def = &code[analysis.single_def_ip_of(code, r)?];
        match def {
            RegInstr::LoadString { value, .. } => Some(value.len() as i64),
            RegInstr::Move { src, .. } => {
                const_byte_len(*src, depth - 1, code, n_regs, analysis, int_const)
            }
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => {
                match intrinsic_descriptor(*intrinsic).bytes_fold_role {
                    Some(BytesFoldRole::ProducerFromString) if args.len() == 1 => {
                        const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)
                    }
                    Some(BytesFoldRole::ProducerSlice) if args.len() == 3 => {
                        let l =
                            const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)?;
                        let start = int_const(args[1])?;
                        let len = int_const(args[2])?;
                        // Mirror `bytes_slice` exactly on the constant operands.
                        let sc = start.max(0);
                        if sc >= l {
                            return Some(0);
                        }
                        let lc = len.max(0);
                        let end = sc.saturating_add(lc).min(l);
                        Some(end - sc)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let mut const_len: Vec<Option<i64>> = vec![None; n_regs];
    for r in 0..n_regs {
        const_len[r] = const_byte_len(r, 32, code, n_regs, &analysis, &int_const);
    }

    // --- In-region foldable Bytes producers ------------------------------------------
    #[derive(Clone)]
    enum BProducer {
        FromString {
            src: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        },
    }
    let mut producer: Vec<Option<BProducer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];
    for i in header..exit {
        let dst_prod: Option<(usize, BProducer)> = match &code[i] {
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerFromString)
                && args.len() == 1 =>
            {
                Some((*dst, BProducer::FromString { src: args[0] }))
            }
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerSlice)
                && args.len() == 3 =>
            {
                Some((
                    *dst,
                    BProducer::Slice {
                        src: args[0],
                        start: args[1],
                        len: args[2],
                    },
                ))
            }
            RegInstr::Move { dst, src } => Some((*dst, BProducer::Alias { src: *src })),
            _ => None,
        };
        if let Some((dst, prod)) = dst_prod {
            if dst >= n_regs {
                return None;
            }
            if analysis.region_def_count(dst)? > 1 {
                multiply_defined[dst] = true;
            }
            producer[dst] = Some(prod);
        }
    }
    // A register defined out-of-region or multiply-defined in-region is not a sound
    // single-producer in-region Bytes value (its out-of-region def may differ). Drop
    // it — but it can still serve as a CONSTANT-length source via `const_len`.
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in 0..n_regs {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // A length source resolves iff it is an in-region foldable producer OR has a
    // constant byte length. `foldable[r]` marks the in-region producers we will
    // dissolve; `const_len` sources stay live (cheap loop-invariant) and supply a
    // `LoadInt`. Resolve in-region producer foldability by fixpoint.
    let resolves = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some())
    };
    let mut foldable = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..n_regs {
            if foldable[r] {
                continue;
            }
            let ok = match &producer[r] {
                Some(BProducer::FromString { src }) => resolves(*src, &foldable),
                Some(BProducer::Slice { src, .. }) => resolves(*src, &foldable),
                Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                None => false,
            };
            if ok {
                foldable[r] = true;
                changed = true;
            }
        }
    }

    // Escape analysis: every use of a foldable (to-be-dissolved) Bytes register must be
    // a sanctioned Bytes consumer — a foldable producer operand, a `Move` to a foldable
    // register, or a `Bytes.len`. Any other read (in OR out of region) escapes; drop it
    // and re-resolve to a fixpoint. (Constant-length sources are NOT dissolved, so their
    // other uses — e.g. the still-live `data` slice arg — do not need to be sanctioned.)
    loop {
        let mut escaped = vec![false; n_regs];
        let note_reads = |rs: &[usize], escaped: &mut Vec<bool>| {
            for &r in rs {
                if r < n_regs && foldable[r] {
                    escaped[r] = true;
                }
            }
        };
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        for i in header..exit {
            match &code[i] {
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerFromString)
                    && args.len() == 1 => {}
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerSlice)
                    && args.len() == 3 =>
                {
                    // args[0] is the (foldable or constant) source consumed as Bytes;
                    // args[1]/args[2] are Int operands. Fine.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {}
                _ if is_bytes_len(&code[i]) => {}
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => note_reads(&rs, &mut escaped),
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        for r in 0..n_regs {
            if escaped[r] {
                foldable[r] = false;
            }
        }
        let mut c2 = true;
        while c2 {
            c2 = false;
            for r in 0..n_regs {
                if !foldable[r] {
                    continue;
                }
                let still = match &producer[r] {
                    Some(BProducer::FromString { src })
                    | Some(BProducer::Slice { src, .. })
                    | Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                    None => false,
                };
                if !still {
                    foldable[r] = false;
                    c2 = true;
                }
            }
        }
    }

    // Every in-region `Bytes.len` MUST now resolve, else a real (non-subset) `BytesLen`
    // survives and blocks OSR ⇒ bail.
    for i in header..exit {
        if is_bytes_len(&code[i]) {
            let src = match &code[i] {
                RegInstr::CallIntrinsic { args, .. }
                | RegInstr::CallTypedIntrinsic { args, .. } => args[0],
                _ => unreachable!(),
            };
            if !resolves(src, &foldable) {
                return None;
            }
        }
    }
    // Nothing to dissolve (e.g. the only `Bytes.len` was directly on a constant-length
    // source with no in-region producer) is still a WIN: we replace the `Bytes.len`
    // with a `LoadInt`/`Move`. Only bail if no `Bytes.len` resolves at all — but the
    // loop above already guaranteed every `Bytes.len` resolves, so proceed.

    // --- Length registers + constant materialization ---------------------------------
    // One fresh Int `len_reg` per resolvable register (foldable producer OR constant
    // source). Constant sources get a `LoadInt` materialized at the region head so the
    // value is live after the native OSR entry (which lands at `header`).
    let needs_len = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some())
    };
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    let mut const_sources: Vec<usize> = Vec::new();
    for r in 0..n_regs {
        if needs_len(r, &foldable) {
            len_reg[r] = next_reg;
            next_reg += 1;
            if !foldable[r] {
                // a pure constant source (not an in-region producer we dissolve)
                if const_len[r].is_some() {
                    const_sources.push(r);
                }
            }
        }
    }

    fn emit_max_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }
    fn emit_min_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::LessEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }
    // out = clamp slice length, byte-exact mirror of `bytes_slice`:
    //   sc = max(start,0); s_clamp = min(sc, L); ec = s_clamp + max(len,0);
    //   e_clamp = min(ec, L); out = e_clamp - s_clamp.
    fn emit_slice_len_b(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let sclamp = *next_reg + 2;
        let lc = *next_reg + 3;
        let ec = *next_reg + 4;
        *next_reg += 5;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        emit_max_b(out_code, sc, start, zero);
        emit_min_b(out_code, sclamp, sc, l_src);
        emit_max_b(out_code, lc, len, zero);
        out_code.push(RegInstr::AddInt {
            dst: ec,
            lhs: sclamp,
            rhs: lc,
        });
        emit_min_b(out_code, ec, ec, l_src);
        out_code.push(RegInstr::SubInt {
            dst: out,
            lhs: ec,
            rhs: sclamp,
        });
    }

    // --- Rewrite the stream ----------------------------------------------------------
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        MapGet { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        // Materialize the constant-length sources AT the header position, emitted BEFORE
        // the header instruction is pushed but AFTER `index_map[header]` is set — so
        // `index_map[header]` points at these `LoadInt`s. This makes them dominate EVERY
        // in-region use INCLUDING the header's own folded instruction: a loop whose
        // CONDITION reads a folded `Bytes.len` (e.g. `while i < Bytes.len(data)`) lowers
        // the header to read `len_reg[src]`, which must already be initialized when the
        // header runs. Native OSR entry lands at the header's mapped ip
        // (`index_map[header]`), and the loop back-edge also targets it, so on every
        // iteration (incl. the first OSR one) these constants run before the condition.
        // Re-running these idempotent `LoadInt`s per iteration is cheap.
        if i == header {
            for &r in &const_sources {
                if let Some(value) = const_len[r] {
                    new_code.push(RegInstr::LoadInt {
                        dst: len_reg[r],
                        value,
                    });
                }
            }
        }
        if in_region(i) {
            let folded = match instr {
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerFromString) =>
                {
                    if let Some(BProducer::FromString { src }) = &producer[*dst] {
                        new_code.push(RegInstr::Move {
                            dst: len_reg[*dst],
                            src: len_reg[*src],
                        });
                    }
                    true
                }
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerSlice) =>
                {
                    if let Some(BProducer::Slice { src, start, len }) = &producer[*dst] {
                        emit_slice_len_b(
                            &mut new_code,
                            len_reg[*dst],
                            len_reg[*src],
                            *start,
                            *len,
                            &mut next_reg,
                        );
                    }
                    true
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                _ if is_bytes_len(instr) => {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    new_code.push(RegInstr::Move {
                        dst,
                        src: len_reg[src],
                    });
                    true
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::MapGet {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = sa;
                        *err_ip = sb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = sa;
                        *else_ip = sb;
                    }
                    _ => {}
                }
            }
            Fix::MapGet { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × J3: scalar-replace non-escaping scalar `Option`s that live entirely
/// inside the loop region `[header, exit)` of an otherwise native-INELIGIBLE
/// function (one whose pre/post-loop code does I/O — calls, `Log.write`, …, which
/// the whole-function [`native_scalar_replace_options`] would reject).
///
/// Soundness model (region-scoped, conservative): an `Option` register is
/// scalar-replaced only if EVERY one of its definitions and uses lies strictly
/// inside `[header, exit)`, every in-region instruction is native-subset, one of
/// the four Option ops, or a `TryResult` consuming a scalar-replaced Option, and
/// the register never appears outside the region (so it is dead at both OSR
/// boundaries and the non-subset I/O outside never touches it). Anything we cannot
/// prove ⇒ `None` (no OSR; the interpreter runs the loop). The loop-carried
/// (boundary) registers keep their original indices; only fresh tag/payload regs
/// (>= `n_regs`) are added, and they are loop-internal.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` where
/// `ip_map[transformed_ip] = original_ip` (each rewritten Option op's fragments map
/// to that op's original index; copy-through maps one-to-one). Out-of-region
/// instructions are copied through verbatim, so the I/O before/after the loop is
/// preserved exactly.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_options_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<(usize, usize)>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no Option op inside the region ⇒ identity transform (plain OSR).
    if !(header..exit).any(|i| is_option_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    // Every in-region instruction must be native-subset or one of the four Option
    // ops; otherwise the loop body cannot become a native loop anyway — bail.
    for i in header..exit {
        if !native_subset_instruction(&code[i])
            && !is_option_op(&code[i])
            && !matches!(&code[i], RegInstr::TryResult { .. })
        {
            return None;
        }
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // OPT = registers carrying an Option value: seed from in-region
    // `MakeSome`/`LoadNone` dsts, close under in-region `Move` aliasing.
    let mut opt = vec![false; n_regs];
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeSome { dst, .. }
            | RegInstr::LoadNone { dst }
            | RegInstr::DequePopFront { dst, .. }
            | RegInstr::DequePopBack { dst, .. } => opt[*dst] = true,
            _ => {}
        }
    }
    analysis.close_region_move_aliases(code, &mut opt)?;

    // Validate in-region uses/defs of OPT registers (identical recognition rules to
    // the whole-function pass), and require a SCALAR payload.
    for i in header..exit {
        match &code[i] {
            RegInstr::LoadNone { dst } if opt[*dst] => {}
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                if opt[*value] {
                    return None; // Option payload ⇒ non-scalar
                }
            }
            RegInstr::DequePopFront { dst, .. } | RegInstr::DequePopBack { dst, .. }
                if opt[*dst] => {}
            RegInstr::Move { dst, src } if opt[*dst] => {
                if !opt[*src] {
                    return None;
                }
            }
            RegInstr::MatchOption { src, .. } if opt[*src] => {}
            RegInstr::UnwrapSome { dst, src } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if opt[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| opt[r]) {
                    return None;
                }
                if let RegInstr::UnwrapSome { dst, .. }
                | RegInstr::MakeSome { dst, .. }
                | RegInstr::LoadNone { dst }
                | RegInstr::DequePopFront { dst, .. }
                | RegInstr::DequePopBack { dst, .. } = other
                {
                    if opt[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // CRITICAL boundary soundness. After scalar replacement the ORIGINAL Option
    // register `o` is NEVER written inside the transformed region (its defs became
    // tag/payload writes), so the interpreter slot for `o` is stale after a clean OSR
    // exit. That is still sound when:
    //
    //   1. every in-region read of an OPT register is definitely assigned by an
    //      in-region Option def before the read, so native never needs a live-in
    //      interpreter Option value, and
    //   2. no out-of-region instruction reads an OPT register after native exits.
    //
    // Out-of-region writes are harmless under those two facts: pre-loop writes are
    // overwritten before every in-loop read, and post-loop writes overwrite the stale
    // interpreter slot before it can be observed. This matters for OSR × inlining,
    // where a call result register is commonly reused outside the loop even though
    // the Option value produced by the inlined callee is loop-local.
    if !option_regs_definitely_assigned_before_region_reads(code, n_regs, header, exit, &opt) {
        return None;
    }

    // J0.1(b) live-after always-`Some` Option reconstruction (the Option analog of the
    // Result pass). Originally ANY out-of-region read of an OPT register bailed. We now
    // allow a read AFTER the region by reconstructing `Some(payload)` at OSR-exit from
    // its scalar payload register, with the same soundness obligations as the Result
    // pass plus the always-`Some` requirement:
    //   * no OPT register written at ip >= exit (pre-loop init, ip < header, is fine);
    //   * a read BEFORE the region (live-in Option) is out of scope;
    //   * the OPT register is always-`Some` (no in-region `LoadNone` def) — a `None`
    //     outcome has no scalar payload to reconstruct and would make the payload only
    //     maybe-assigned; and
    //   * a single in-region `MakeSome` def reached UNCONDITIONALLY each iteration (so
    //     the payload is definitely-assigned after >=1 iteration).
    // (Conservative register footprints still hold: an unanalyzable instruction reports
    // `RegFootprint::All`, which bails. The scalar-payload-TYPE check is deferred to the
    // OsrEntry build site.)
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if i >= header && i < exit {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && opt[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && opt[r] {
                        if i < header {
                            return None; // live-in Option (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        // Always-`Some`: no in-region `LoadNone` def for this register.
        if (header..exit).any(|i| matches!(&code[i], RegInstr::LoadNone { dst } if *dst == reg)) {
            return None;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| i >= header && i < exit)
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in header..def_ip {
            match &code[i] {
                RegInstr::JumpIfBool { target, .. }
                | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate fresh tag/payload regs per OPT register.
    let mut tag_reg = vec![0usize; n_regs];
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_opt) in opt.iter().enumerate() {
        if *is_opt {
            tag_reg[reg] = next_reg;
            payload_reg[reg] = next_reg + 1;
            next_reg += 2;
        }
    }

    // J0.1(b) Some-Option reconstruction recipes: (opt_reg, payload_reg).
    let option_recipes: Vec<(usize, usize)> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| (reg, payload_reg[reg]))
        .collect();

    // Rewrite the WHOLE code, scalar-replacing in-region Option ops and copying
    // everything else through verbatim; remap all jump/match targets through the
    // index map. (Out-of-region jumps keep pointing at the right place after the
    // region's instructions expand.)
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeSome { dst, value } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *value,
                });
            }
            RegInstr::LoadNone { dst } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: false,
                });
            }
            RegInstr::DequePopFront { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopFront {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::DequePopBack { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopBack {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::Move { dst, src } if region && opt[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } if region && opt[*src] => {
                fixups.push((new_code.len(), Fix::Target(*some_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*none_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapSome { dst, src } if region && opt[*src] => {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::TryResult { dst, src, .. } if region && opt[*src] => {
                let some_target = new_code.len() + 2;
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: some_target,
                });
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            // Copy-through, remapping jump targets (covers both in-region native
            // branches and the pre/post-loop control flow). `MatchOption` outside the
            // region (or on a non-OPT src) is copied with BOTH targets remapped.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, option_recipes))
}

/// Whether a `MakeVariant` layout name is a `Result` constructor (`Ok`/`Err`). These
/// are reserved by the language for `Result`, are matched by the dedicated
/// `MatchResult` op (not `MatchVariant`), and are dissolved by the Result region pass
/// — never by the user-variant pass.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_result_ctor_name(name: &str) -> bool {
    name == "Ok" || name == "Err"
}

/// OSR × J3 for RESULTS (deopt-before-heap, Slice 1): scalar-replace a non-escaping
/// `Result<Scalar, _>` that is **statically always-`Ok`** on the native path and
/// lives entirely inside the loop region `[header, exit)` of an otherwise native-
/// INELIGIBLE function. Mirrors [`native_scalar_replace_options_in_region`] but for
/// the `Result` shape (`MakeVariant{Ok|Err}` + `MatchResult` +
/// `UnwrapVariantValue`/`TryResult`).
///
/// KEY (the deopt-before-heap interplay): when a leaf's `Err` arm built a heap value,
/// [`native_inline_leaf_calls`] already replaced that arm with a native `Bail`, so the
/// inlined stream contains NO reachable `MakeVariant{Err}` — the only constructor of
/// the Result register is `MakeVariant{Ok, [scalar]}`. The Result is therefore
/// statically always-`Ok`, and this pass dissolves it to a single scalar **payload**
/// register (no tag needed): every `MatchResult{src:R}` becomes an unconditional
/// `Jump → ok_ip` (the `Err` arm goes dead) and every `UnwrapVariantValue{src:R,
/// expected:"Ok"}` becomes a `Move` from the payload. A LIVE heap `Err` (a reachable
/// `MakeVariant{Err}` def of `R`) ⇒ BAIL the pass (leave the loop on the interpreter):
/// such a Result is genuinely two-armed with a heap payload and cannot be scalarized.
///
/// Soundness: identical region discipline to the sibling passes. `R` is replaced only
/// if every def/use lies strictly inside `[header, exit)`, the `Ok` payload is scalar,
/// and `R` is dead at both OSR boundaries (`instr_read_regs`/`instr_written_reg`, with
/// `RegFootprint::All ⇒ bail`). The always-`Ok` rewrite is sound because the only way
/// the program reaches the dissolved `MatchResult` is by having built an `Ok` (the
/// `Err` constructor bailed to the interpreter before any heap op — Exec Spec §7.2), so
/// the `Err` arm is dynamically unreachable on the native path; rewriting it to a
/// statically-dead `Jump`/`Bail` cannot change observable behavior.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other region passes.
///
/// A live-after Result reconstruction recipe:
/// `(variant_reg, ok_payload_reg, err_payload_reg, tag_reg)`. `tag_reg` is `None` for an
/// always-`Ok` Result (reconstruct `Ok(ok_payload)`; `err_payload` is unused, set equal
/// to `ok_payload`) and `Some(tag)` for a two-armed Result (reconstruct
/// `Ok(ok_payload)` if the tag's live value is non-zero, else `Err(err_payload)`).
/// PER-ARM payloads: the `Ok` and `Err` arms keep SEPARATE payload registers so arms
/// of different native types (e.g. `Result<Int, String>`) don't force a single payload
/// register into conflicting types. Only the arm matching the live `tag` is read at
/// reconstruction, so the other (possibly stale) payload is never observed.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) type ResultRecipe = (usize, usize, usize, Option<usize>);

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<ResultRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no `MatchResult` and no `Result` constructor in the region ⇒ nothing
    // for THIS pass to do (identity transform; preserves the byte-for-byte old path).
    let has_result_op = (header..exit).any(|i| {
        matches!(&code[i], RegInstr::MatchResult { .. })
            || matches!(&code[i], RegInstr::MakeVariant { layout, .. } if is_result_ctor_name(&layout.name))
    });
    if !has_result_op {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // RES = registers carrying a (replaceable) Result value: seed from in-region
    // `MakeVariant{Ok|Err}` dsts, close under in-region `Move` aliasing.
    let mut res = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, layout, .. } = &code[i] {
            if is_result_ctor_name(&layout.name) {
                res[*dst] = true;
            }
        }
    }
    analysis.close_region_move_aliases(code, &mut res)?;
    // A `MatchResult{src}` whose `src` is not (yet) a RES register means a Result we
    // cannot see being constructed in-region (it flows in from outside, e.g. a heap
    // Result param) ⇒ this pass cannot dissolve it. Bail so the loop stays on the
    // interpreter (the boundary/escape gates below would also catch it, but bailing
    // early is clearer and conservative).
    for i in header..exit {
        if let RegInstr::MatchResult { src, .. } = &code[i] {
            if !res[*src] {
                return None;
            }
        }
    }

    // Two-armed (J0.1 #7 follow-up): a reachable `MakeVariant{Err}` on a RES register
    // means the Result is NOT statically always-`Ok`. Instead of bailing, dissolve it
    // with an explicit `Ok`/`Err` tag + a shared scalar payload register (the tag routes
    // the `MatchResult` and selects which arm's `UnwrapVariantValue` reads the payload).
    // Scoped to dead-at-boundary: a two-armed RES that is live-after, or short-circuited
    // by `?` (`TryResult`), bails (live-after Ok/Err reconstruction stays future).
    let two_armed = (header..exit).any(|i| {
        matches!(&code[i],
            RegInstr::MakeVariant { dst, layout, .. }
                if res[*dst] && layout.name.as_ref() == "Err")
    });
    if two_armed {
        return native_scalar_replace_two_armed_results_in_region(code, n_regs, header, exit, &res);
    }

    // Validate every in-region def/use of a RES register. The Result must be
    // statically always-`Ok`: a reachable `MakeVariant{Err}` def is a LIVE heap Err
    // ⇒ bail. `Ok` payload must be scalar (not itself a RES register). Recognized uses:
    // `MatchResult{src:R}`, `UnwrapVariantValue{src:R}` (Ok scalar payload, or the dead
    // Err-arm unwrap which the rewrite drops), `TryResult{src:R}` (the `?` success
    // projection), and `Move` aliases. Anything else that touches a RES register ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if res[*dst] => {
                if layout.name.as_ref() == "Err" {
                    return None; // live heap Err ⇒ not always-Ok ⇒ leave on interpreter
                }
                // Ok constructor: exactly one scalar field `value`.
                if fields.len() != 1 || fields.iter().any(|(_, r)| res[*r]) {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if res[*dst] => {
                if !res[*src] {
                    return None;
                }
            }
            RegInstr::MatchResult { src, .. } if res[*src] => {}
            RegInstr::TryResult { dst, src, .. } if res[*src] => {
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if res[*src] => {
                // The Ok-arm unwrap yields the scalar payload; its `dst` must not be a
                // RES register (a Result payload would be non-scalar). The Err-arm
                // unwrap (`expected == "Err"`) lies on the statically-dead arm — its
                // `dst` is unused on the native path; allow it (rewritten to a Bail).
                let _ = expected;
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if res[*src] => {}
            other => {
                // Any other instruction must not read a RES register, nor (re)define one
                // through an unrecognized destination.
                match instr_read_regs(other) {
                    RegFootprint::Some(reads) => {
                        if reads.into_iter().any(|r| r < n_regs && res[r]) {
                            return None;
                        }
                    }
                    RegFootprint::All => return None,
                }
                if let RegInstr::UnwrapVariantValue { dst, .. }
                | RegInstr::MakeVariant { dst, .. } = other
                {
                    if res[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // Boundary soundness + J0.1(b) live-after reconstruction. Originally every RES
    // register had to be DEAD outside `[header, exit)`. We now also allow a RES
    // register that is only READ after the region by reconstructing `Ok(payload)` at
    // OSR-exit from its scalar payload register, because the pass already proved every
    // RES register is always-`Ok` with a scalar `Ok` payload (a heap `Err` became a
    // native `Bail`), so a completed native loop guarantees the value is `Ok(payload)`.
    // Conditions to keep it sound:
    //   * No RES register may be WRITTEN at ip >= exit (post-loop reassignment is out
    //     of scope). A write BEFORE the region (pre-loop `let mut r = Ok(..)`) is fine:
    //     native never touches the original RES slot and reconstruction overwrites it
    //     at exit, or — after 0 native iterations — the pre-loop value already in the
    //     slot is exactly correct.
    //   * A RES register read BEFORE the region (a live-in Result) is out of scope.
    //   * Each live-after RES register needs a single in-region definition reached
    //     UNCONDITIONALLY each iteration (no branch between the header and the def
    //     except the header's own loop-exit condition), so its payload register is
    //     definitely-assigned after >=1 iteration (hence present in the OSR-exit deopt
    //     live set). A conditional/multiply-defined RES register would leave the
    //     payload only maybe-assigned ⇒ bail (conservative).
    // The scalar-payload-TYPE check is deferred to the OsrEntry build site (where
    // native register types are known); a non-scalar `Ok` payload declines OSR there.
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && res[r]) {
                    return None; // post-loop reassignment of a dissolved Result
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && res[r] {
                        if i < header {
                            return None; // live-in Result (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    // Require a single, unconditionally-reached in-region def for every RES register we
    // must reconstruct.
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| in_region(i))
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in header..def_ip {
            match &code[i] {
                // The header's loop-exit condition (target outside the loop) is fine.
                RegInstr::JumpIfBool { target, .. }
                | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                // Any other branch/match/return between the header and the def could
                // skip the def on some iteration ⇒ payload not definitely-assigned.
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate one fresh payload register per RES register (always-Ok ⇒ no tag).
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_res) in res.iter().enumerate() {
        if *is_res {
            payload_reg[reg] = next_reg;
            next_reg += 1;
        }
    }

    // J0.1(b) recipes for each live-after RES register. Always-`Ok` recipes carry no
    // tag (`None`) ⇒ reconstruct `Ok(payload)`; this path has a single payload register,
    // so the (unused) err-payload slot mirrors it.
    let recipes: Vec<ResultRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| (reg, payload_reg[reg], payload_reg[reg], None))
        .collect();

    // Rewrite the WHOLE code, dissolving in-region Result ops and copying everything
    // else through verbatim; remap all jump/match targets through the index map.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant { dst, fields, .. } if region && res[*dst] => {
                // Always-Ok constructor: payload = the single scalar field.
                let (_, field_reg) = &fields[0];
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *field_reg,
                });
            }
            RegInstr::Move { dst, src } if region && res[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchResult { src, ok_ip, err_ip } if region && res[*src] => {
                // Statically always-Ok ⇒ unconditional jump to the Ok arm. The Err arm
                // (`err_ip`) becomes unreachable.
                let _ = err_ip;
                fixups.push((new_code.len(), Fix::Target(*ok_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::TryResult { dst, src, .. } if region && res[*src] => {
                // Statically always-Ok `?` ⇒ payload projection. The short-circuit arm is
                // unreachable on the native path; any real Err was already replaced by a
                // Bail before constructing heap state, so the interpreter rerun performs
                // the normal cleanup/return behavior.
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if region && res[*src] => {
                if expected.as_str() == "Err" {
                    // Dead Err-arm unwrap: unreachable after the always-Ok rewrite. Emit
                    // a Bail sentinel so that, even if some path reached it, native would
                    // safely deopt rather than read a non-existent heap Err payload.
                    new_code.push(RegInstr::RuntimeError {
                        message: String::new(),
                    });
                } else {
                    // Ok-arm unwrap ⇒ the scalar payload.
                    new_code.push(RegInstr::Move {
                        dst: *dst,
                        src: payload_reg[*src],
                    });
                }
            }
            // Copy-through, remapping jump targets (in-region native branches and the
            // pre/post-loop control flow). A `MatchResult` outside the region (or on a
            // non-RES src) is copied with BOTH targets remapped; same for the other
            // match ops the body may carry.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}

/// Two-armed scalar `Result` dissolution (J0.1 #7 follow-up). Companion to
/// [`native_scalar_replace_results_in_region`]'s always-`Ok` path: handles a Result
/// constructed as EITHER `Ok(scalar)` or `Err(scalar)` in-region and consumed (matched)
/// in-region. Each RES register becomes a boolean `tag` (true = `Ok`) plus one shared
/// scalar `payload`; `MatchResult` routes on the tag and each arm's `UnwrapVariantValue`
/// reads the payload (which holds the constructed arm's value).
///
/// Live-after is supported: a RES register read after the region reconstructs `Ok` or
/// `Err` from the tag + payload at OSR-exit. This is sound WITHOUT a definite-assignment
/// analysis because the rewrite sets `tag` and `payload` at EVERY point the original
/// Result is assigned, so they are definitely-assigned wherever the Result is — hence in
/// the OSR-exit deopt live set whenever the Result is live there. `?`-short-circuit
/// (`TryResult`) and a RES register written AFTER the region (post-loop reassignment) or
/// read BEFORE it (live-in) are out of scope ⇒ bail. `res` is the move-closed RES set.
///
/// LIMITATION (the `Ok` and `Err` arms share ONE `payload` register): this only OSRs
/// when both arms carry the SAME native type (e.g. `Result<Int, Int>`). A Result whose
/// arms differ (e.g. `Result<Int, String>` — Int `Ok`, Handle `Err`) assigns the shared
/// payload conflicting types, so the native type inference rejects it and the loop
/// declines to OSR (SAFE — runs on the interpreter, never incorrect). Supporting
/// different-typed arms needs a per-arm payload register; the heap arm additionally
/// needs the extended deopt ABI (carrying Handle payloads) for the live-after case.
/// Verified by probe 2026-06-28: same-typed arms OSR; `Result<Int,String>` declines.
#[cfg(feature = "native-jit")]
fn native_scalar_replace_two_armed_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    res: &[bool],
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<ResultRecipe>)> {
    let in_region = |i: usize| i >= header && i < exit;

    // Validate every in-region def/use of a RES register: defs are `Ok`/`Err`
    // constructors with one scalar (non-RES) field, or a `Move` from another RES; uses
    // are `MatchResult`/`UnwrapVariantValue`. `?` (`TryResult`) and any other touch bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant { dst, layout, fields } if res[*dst] => {
                let name = layout.name.as_ref();
                if name != "Ok" && name != "Err" {
                    return None;
                }
                if fields.len() != 1 || fields.iter().any(|(_, r)| res[*r]) {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if res[*dst] => {
                if !res[*src] {
                    return None;
                }
            }
            RegInstr::MatchResult { src, .. } if res[*src] => {}
            RegInstr::UnwrapVariantValue { dst, src, .. } if res[*src] => {
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { src, .. } if res[*src] => return None,
            other => {
                match instr_read_regs(other) {
                    RegFootprint::Some(reads) => {
                        if reads.into_iter().any(|r| r < n_regs && res[r]) {
                            return None;
                        }
                    }
                    RegFootprint::All => return None,
                }
                if let RegInstr::UnwrapVariantValue { dst, .. }
                | RegInstr::MakeVariant { dst, .. } = other
                {
                    if res[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // Boundary: a RES register WRITTEN after the region (post-loop reassignment) or READ
    // before it (live-in) is out of scope ⇒ bail. A RES register READ after the region is
    // live-after ⇒ mark it for OSR-exit reconstruction.
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && res[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && res[r] {
                        if i < header {
                            return None; // live-in Result
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }

    // Allocate a `tag` + PER-ARM `ok_payload` + `err_payload` register per RES register
    // (consecutively). Separate per-arm payloads let arms of different native types
    // (e.g. `Result<Int, String>`: Int `Ok`, Handle `Err`) each carry their own typed
    // value instead of forcing one shared payload register into conflicting types.
    let mut tag_reg = vec![0usize; n_regs];
    let mut ok_payload_reg = vec![0usize; n_regs];
    let mut err_payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, &is_res) in res.iter().enumerate() {
        if is_res {
            tag_reg[reg] = next_reg;
            ok_payload_reg[reg] = next_reg + 1;
            err_payload_reg[reg] = next_reg + 2;
            next_reg += 3;
        }
    }

    // Live-after reconstruction recipes:
    // `(variant_reg, ok_payload, err_payload, Some(tag_reg))` ⇒ rebuild `Ok(ok_payload)`
    // / `Err(err_payload)` from the tag at OSR-exit. Sound without a definite-assignment
    // pass: `tag` + the taken arm's payload are written at every RES def, so the payload
    // matching the live tag is in the deopt live set wherever the Result is live (the
    // other arm's payload is never read).
    let recipes: Vec<ResultRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| (reg, ok_payload_reg[reg], err_payload_reg[reg], Some(tag_reg[reg])))
        .collect();

    // Rewrite, dissolving in-region Result ops; remap jump/match targets through the map.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len() + 8);
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant {
                dst, layout, fields, ..
            } if region && res[*dst] => {
                let is_ok = layout.name.as_ref() == "Ok";
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: is_ok,
                });
                // Write only the taken arm's payload register (the tag routes the
                // matching unwrap; the other arm's payload is never read for this value).
                let (_, field_reg) = &fields[0];
                new_code.push(RegInstr::Move {
                    dst: if is_ok {
                        ok_payload_reg[*dst]
                    } else {
                        err_payload_reg[*dst]
                    },
                    src: *field_reg,
                });
            }
            RegInstr::Move { dst, src } if region && res[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: ok_payload_reg[*dst],
                    src: ok_payload_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: err_payload_reg[*dst],
                    src: err_payload_reg[*src],
                });
            }
            RegInstr::MatchResult { src, ok_ip, err_ip } if region && res[*src] => {
                // tag true (Ok) ⇒ ok_ip; else fall through to an unconditional jump to err_ip.
                fixups.push((new_code.len(), Fix::Target(*ok_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*err_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapVariantValue {
                dst, src, expected, ..
            } if region && res[*src] => {
                // Read the payload register for the arm this unwrap belongs to: the `Err`
                // unwrap (reached only on the `Err` branch) reads `err_payload`; `Ok`
                // (the default) reads `ok_payload`.
                let src_payload = if expected.as_str() == "Err" {
                    err_payload_reg[*src]
                } else {
                    ok_payload_reg[*src]
                };
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: src_payload,
                });
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}

/// OSR × J3 for VARIANTS: scalar-replace non-escaping user `sum`/variant values that
/// live entirely inside the loop region `[header, exit)` of an otherwise native-
/// INELIGIBLE function. Mirrors [`native_scalar_replace_options_in_region`] but for
/// `MakeVariant`/`MatchVariant`/`UnwrapVariantValue`.
///
/// Scope (multiple scalar payload fields per arm; N>=0 per arm, possibly different N
/// per arm): a variant register `R` is replaced iff every definition is a `MakeVariant`
/// whose arm carries only **scalar fields** (each payload register must end up
/// Int/Float/Bool — never a heap/Option/variant value, and never itself a replaceable
/// variant register) or a `Move` from another replaceable variant register; every use
/// is `MatchVariant{src:R}`, `UnwrapVariantValue{src:R}`, a `GetField{base:R}` (the
/// payload-field read a struct-style arm pattern lowers to), or a `Move{src:R}` to
/// another replaceable variant register; and `R` never appears as a value operand of
/// anything else. Heap/sum-payload arms (incl. nested variants/structs) ⇒ bail. If any
/// in-region variant register is not replaceable, the whole region pass bails (no OSR),
/// exactly like the Option pass.
///
/// Rewrite: each `R` becomes a `tag` register (Int holding the arm index) PLUS one
/// fresh scalar **leaf register per `(arm, slot)`** — i.e. every payload field of every
/// arm in `R`'s alias class gets its own leaf register (so a 3-field arm dissolves to a
/// tag plus three leaf registers). A per-class arm-name→tag-index map is built by
/// scanning the class's `MakeVariant` arm names AND its `MatchVariant`/
/// `UnwrapVariantValue` `expected` names, assigning each distinct name a stable index.
/// Per arm, the slot order is the arm's `MakeVariant` `layout.field_names` (all defs of
/// one arm must agree on field names; otherwise bail).
/// - `MakeVariant{dst:R, layout, fields}` → `LoadInt tag = idx(layout.name)`; for each
///   slot, `Move leaf[(arm, slot)] = <that field reg>` (fieldless arms write only the
///   tag).
/// - `MatchVariant{src:R, expected, match_ip, else_ip}` → `LoadInt c = idx(expected)`;
///   `Equal eq = tag, c`; `JumpIfBool eq==true → match_ip`; `Jump → else_ip`.
/// - `GetField{dst, base:R, name}` → `Move dst = leaf[(arm, slot)]`, where the arm is
///   the unique class arm that declares field `name` (ambiguous/absent name ⇒ bail) and
///   the slot is `name`'s position in that arm's field list.
/// - `UnwrapVariantValue{dst, src:R, expected}` → `Move dst = leaf[(expected, 0)]` (the
///   single-field tuple-arm case; its arm is `expected`, slot 0).
/// - `Move` aliases copy the tag and every leaf register.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the Option region pass (each rewritten op's fragments map to
/// the original op's index; copy-through maps one-to-one).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_variants_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no variant op inside the region ⇒ nothing for THIS pass to do.
    if !(header..exit).any(|i| is_variant_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // VAR = registers carrying a (replaceable) variant value: seed from in-region
    // `MakeVariant` dsts, close under in-region `Move` aliasing.
    let mut var = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, .. } = &code[i] {
            var[*dst] = true;
        }
    }
    analysis.close_region_move_aliases(code, &mut var)?;

    // Every in-region instruction must be native-subset, a variant op, or a
    // `GetField` reading a VAR register (the payload-field read a struct-style arm
    // pattern lowers to — its `base` is the matched variant). `GetField` on a
    // non-VAR base is a heap struct read this pass can't lower ⇒ bail.
    let is_var_getfield =
        |i: usize| -> bool { matches!(&code[i], RegInstr::GetField { base, .. } if var[*base]) };
    for i in header..exit {
        if !native_subset_instruction(&code[i]) && !is_variant_op(&code[i]) && !is_var_getfield(i) {
            return None;
        }
    }

    // Validate in-region uses/defs of VAR registers. Each `MakeVariant` arm carries
    // only scalar fields (no field may itself be a VAR register — that would be a
    // nested variant, out of scope ⇒ bail). Anything else touching a VAR register that
    // is not a recognized consumer ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant { dst, fields, .. } if var[*dst] => {
                if fields.iter().any(|(_, field_reg)| var[*field_reg]) {
                    return None; // nested variant payload ⇒ non-scalar
                }
            }
            RegInstr::Move { dst, src } if var[*dst] => {
                if !var[*src] {
                    return None;
                }
            }
            RegInstr::MatchVariant { src, .. } if var[*src] => {}
            RegInstr::UnwrapVariantValue { dst, src, .. } if var[*src] => {
                if var[*dst] {
                    return None; // unwrapped payload aliased as a variant ⇒ non-scalar
                }
            }
            // A payload-field read of a struct-style arm (`Rect(w, h) => ... read w`).
            // Its `dst` must NOT be a VAR register (a struct/variant-typed field is a
            // nested aggregate ⇒ out of scope, bail).
            RegInstr::GetField { dst, base, .. } if var[*base] => {
                if var[*dst] {
                    return None;
                }
            }
            // `DeepCopy` of a VAR register (e.g. from a `read`/param-marshalling of a
            // heap variant): a no-op once the variant is scalar-replaced (tag/payload
            // are copied by value). Allowed here; dropped in the rewrite.
            RegInstr::DeepCopy { reg } if var[*reg] => {}
            RegInstr::Move { src, .. } if var[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| var[r]) {
                    return None;
                }
                if let RegInstr::UnwrapVariantValue { dst, .. }
                | RegInstr::MakeVariant { dst, .. } = other
                {
                    if var[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // Dead-at-boundary soundness (identical argument to the Option pass): every VAR
    // register must be neither read nor written by ANY instruction OUTSIDE the region,
    // so the original register slot is never observed by interpreter-run code across
    // either OSR boundary. A footprint we cannot model reports `All` ⇒ bail.
    if analysis
        .external_reads_or_writes_touch(code, &var)
        .unwrap_or(true)
    {
        return None;
    }

    // `Move`-aliased VAR registers share ONE tag/payload register pair (the alias
    // copies both halves), and therefore must agree on the arm-name→index map. Group
    // VAR registers into alias classes with a union-find over in-region `Move` edges.
    let mut parent: Vec<usize> = (0..n_regs).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != c {
            let n = parent[c];
            parent[c] = r;
            c = n;
        }
        r
    }
    for i in header..exit {
        if let RegInstr::Move { dst, src } = &code[i] {
            if var[*dst] && var[*src] {
                let (a, b) = (find(&mut parent, *dst), find(&mut parent, *src));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }

    // Build ONE canonical arm-name→tag-index map per alias class. Scan every in-region
    // `MakeVariant` arm name AND `MatchVariant` `expected` name, collecting them under
    // the register's class root; assign each distinct name a stable index in sorted-
    // name order (deterministic, and identical across all registers in the class).
    let mut class_names: HashMap<usize, BTreeSet<String>> = HashMap::new();
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant { dst, layout, .. } if var[*dst] => {
                let root = find(&mut parent, *dst);
                class_names
                    .entry(root)
                    .or_default()
                    .insert(layout.name.to_string());
            }
            RegInstr::MatchVariant { src, expected, .. } if var[*src] => {
                let root = find(&mut parent, *src);
                class_names
                    .entry(root)
                    .or_default()
                    .insert(expected.clone());
            }
            _ => {}
        }
    }
    let class_arm_index: HashMap<usize, HashMap<String, i64>> = class_names
        .into_iter()
        .map(|(root, names)| {
            let map: HashMap<String, i64> = names
                .into_iter()
                .enumerate()
                .map(|(k, name)| (name, k as i64))
                .collect();
            (root, map)
        })
        .collect();
    let arm_idx = |reg: usize, parent: &mut Vec<usize>, name: &str| -> i64 {
        let root = find(parent, reg);
        *class_arm_index
            .get(&root)
            .and_then(|m| m.get(name))
            .expect("arm name interned for its class")
    };

    // Per class, per arm-name, the slot-ordered payload field names (from each arm's
    // `MakeVariant` `layout.field_names`). All `MakeVariant` defs of one arm in a class
    // MUST agree on the field-name vector (they always do for a single `sum` type; a
    // disagreement means an unresolvable shape ⇒ bail). `(root, arm_name)` keys.
    let mut class_arm_fields: HashMap<(usize, String), Vec<Rc<str>>> = HashMap::new();
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, layout, .. } = &code[i] {
            if var[*dst] {
                let root = find(&mut parent, *dst);
                let key = (root, layout.name.to_string());
                let shape = layout.field_names.clone();
                match class_arm_fields.get(&key) {
                    Some(prev) if *prev != shape => return None, // shape contradiction
                    Some(_) => {}
                    None => {
                        class_arm_fields.insert(key, shape);
                    }
                }
            }
        }
    }

    // Per class, the field-name → owning arm-name map, used to resolve a
    // `GetField{base:R, name}` to its arm. A field name owned by MORE THAN ONE arm in a
    // class is ambiguous (we can't statically know which arm's leaf the read refers to)
    // ⇒ bail conservatively. Within one arm a field name is unique by construction.
    let mut class_field_owner: HashMap<(usize, String), String> = HashMap::new();
    for ((root, arm_name), fields) in &class_arm_fields {
        for fname in fields {
            let key = (*root, fname.to_string());
            match class_field_owner.get(&key) {
                Some(existing) if existing != arm_name => return None, // ambiguous field
                Some(_) => {}
                None => {
                    class_field_owner.insert(key, arm_name.clone());
                }
            }
        }
    }

    // Allocate fresh registers per alias class: ONE tag register, plus one leaf scalar
    // register per `(arm, slot)` across all arms of the class. `class_tag[root]` is the
    // tag register; `leaf_reg[(root, arm_name, slot)]` is the per-field leaf register.
    let mut tag_reg = vec![0usize; n_regs];
    let mut class_tag: HashMap<usize, usize> = HashMap::new();
    let mut leaf_reg: HashMap<(usize, String, usize), usize> = HashMap::new();
    let mut next_reg = n_regs;
    // Stable allocation order: roots ascending, then arms by tag-index, then slot.
    let mut roots: Vec<usize> = (0..n_regs)
        .filter(|&r| var[r])
        .map(|r| find(&mut parent, r))
        .collect();
    roots.sort_unstable();
    roots.dedup();
    for root in &roots {
        let t = next_reg;
        next_reg += 1;
        class_tag.insert(*root, t);
        // Arms in tag-index order for determinism.
        let mut arms: Vec<(String, Vec<Rc<str>>)> = class_arm_fields
            .iter()
            .filter(|((r, _), _)| r == root)
            .map(|((_, arm), fields)| (arm.clone(), fields.clone()))
            .collect();
        arms.sort_by_key(|(arm, _)| {
            class_arm_index
                .get(root)
                .and_then(|m| m.get(arm))
                .copied()
                .unwrap_or(i64::MAX)
        });
        for (arm, fields) in arms {
            for slot in 0..fields.len() {
                leaf_reg.insert((*root, arm.clone(), slot), next_reg);
                next_reg += 1;
            }
        }
    }
    for reg in 0..n_regs {
        if var[reg] {
            let root = find(&mut parent, reg);
            tag_reg[reg] = class_tag[&root];
        }
    }
    // Resolve a `(reg, arm, slot)` to its leaf register.
    let leaf_of = |reg: usize, parent: &mut Vec<usize>, arm: &str, slot: usize| -> Option<usize> {
        let root = find(parent, reg);
        leaf_reg.get(&(root, arm.to_string(), slot)).copied()
    };
    // Resolve a `GetField{base:R, name}` to its `(arm, slot)`: `name`'s owning arm in
    // R's class, and `name`'s position in that arm's field list.
    let getfield_arm_slot =
        |reg: usize, parent: &mut Vec<usize>, name: &str| -> Option<(String, usize)> {
            let root = find(parent, reg);
            let arm = class_field_owner.get(&(root, name.to_string()))?.clone();
            let slot = class_arm_fields
                .get(&(root, arm.clone()))?
                .iter()
                .position(|f| &**f == name)?;
            Some((arm, slot))
        };

    // PRE-FLIGHT (bail-rather-than-panic): every in-region payload read of a VAR
    // register must resolve to a concrete leaf register BEFORE we rewrite. A
    // `GetField` whose name has no owning arm / unresolvable slot, or an
    // `UnwrapVariantValue` on an arm with no in-region `MakeVariant` (hence no leaf),
    // is rejected here so the rewrite below can rely on the lookups succeeding.
    for i in header..exit {
        match &code[i] {
            RegInstr::GetField { base, name, .. } if var[*base] => {
                let (arm, slot) = getfield_arm_slot(*base, &mut parent, name)?;
                leaf_of(*base, &mut parent, &arm, slot)?;
            }
            RegInstr::UnwrapVariantValue { src, expected, .. } if var[*src] => {
                // Tuple-style single-field arm: payload is slot 0 of `expected`.
                leaf_of(*src, &mut parent, expected, 0)?;
            }
            _ => {}
        }
    }

    // Rewrite the whole code, scalar-replacing in-region variant ops and copying the
    // rest through, remapping all jump/match targets through the index map. Each
    // rewritten op may emit MULTIPLE instructions; jump fixups are resolved after.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    // Scratch registers for `MatchVariant` lowering (constant + equality result). One
    // dedicated pair, reused across all matches (each match fully consumes them before
    // branching, and they are never live across the branch).
    let cmp_const_reg = next_reg;
    let cmp_eq_reg = next_reg + 1;
    next_reg += 2;
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if region && var[*dst] => {
                let idx = arm_idx(*dst, &mut parent, &layout.name);
                new_code.push(RegInstr::LoadInt {
                    dst: tag_reg[*dst],
                    value: idx,
                });
                // Each scalar payload field → `Move` into its `(arm, slot)` leaf
                // register. `fields` is in canonical slot order (matching the arm's
                // `layout.field_names`, validated above), so slot = field index.
                for (slot, (_, field_reg)) in fields.iter().enumerate() {
                    let leaf = leaf_of(*dst, &mut parent, &layout.name, slot)
                        .expect("MakeVariant arm/slot leaf interned");
                    new_code.push(RegInstr::Move {
                        dst: leaf,
                        src: *field_reg,
                    });
                }
            }
            RegInstr::Move { dst, src } if region && var[*dst] => {
                // Alias copy: source and dst share a class ⇒ identical tag and leaf
                // registers ⇒ these would be self-copies. Emit nothing.
                let _ = (dst, src);
            }
            RegInstr::MatchVariant {
                src,
                expected,
                match_ip,
                else_ip,
            } if region && var[*src] => {
                let idx = arm_idx(*src, &mut parent, expected);
                new_code.push(RegInstr::LoadInt {
                    dst: cmp_const_reg,
                    value: idx,
                });
                new_code.push(RegInstr::Equal {
                    dst: cmp_eq_reg,
                    lhs: tag_reg[*src],
                    rhs: cmp_const_reg,
                });
                fixups.push((new_code.len(), Fix::Target(*match_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: cmp_eq_reg,
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*else_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            // Tuple-style single-field arm payload read: slot 0 of `expected`.
            RegInstr::UnwrapVariantValue { dst, src, expected } if region && var[*src] => {
                let leaf = leaf_of(*src, &mut parent, expected, 0)
                    .expect("UnwrapVariantValue arm leaf interned (pre-flight checked)");
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            // Struct-style arm payload-field read: `Move dst = leaf[(arm, slot)]`.
            RegInstr::GetField { dst, base, name } if region && var[*base] => {
                let (arm, slot) = getfield_arm_slot(*base, &mut parent, name)
                    .expect("GetField arm/slot resolvable (pre-flight checked)");
                let leaf = leaf_of(*base, &mut parent, &arm, slot)
                    .expect("GetField leaf interned (pre-flight checked)");
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            // `DeepCopy` of a scalar-replaced variant: drop it (scalars copy by value).
            RegInstr::DeepCopy { reg } if region && var[*reg] => {}
            // Copy-through, remapping jump/match targets.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × J3 for non-escaping FLAT user STRUCTS. Mirrors
/// Resolve the declared layout shape (`field_names`) of a struct-valued register by
/// walking its in-region definitions. A register defined by `MakeStruct` carries the
/// shape directly; a `Move` forwards its source's shape; a `GetFieldSlot{dst, base,
/// slot}` reading a struct-typed field has the shape of whatever `MakeStruct` wrote
/// that slot of `base` (i.e. the inner field's own struct shape). Returns `None` when
/// the shape is ambiguous or not statically resolvable (⇒ the caller bails OSR).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn struct_shape_of_reg(
    code: &[RegInstr],
    header: usize,
    exit: usize,
    reg: usize,
) -> Option<Vec<Rc<str>>> {
    fn go(
        code: &[RegInstr],
        header: usize,
        exit: usize,
        reg: usize,
        depth: usize,
    ) -> Option<Vec<Rc<str>>> {
        if depth > 64 {
            return None;
        }
        let mut found: Option<Vec<Rc<str>>> = None;
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct { dst, layout, .. } if *dst == reg => {
                    let shape = layout.field_names.clone();
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                RegInstr::Move { dst, src } if *dst == reg => {
                    let shape = go(code, header, exit, *src, depth + 1)?;
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if *dst == reg => {
                    // The shape of `reg` is the shape of the struct stored in `base`'s
                    // `slot`: resolve the field-source register that filled that slot of
                    // `base` (following `base` through Move aliases to its `MakeStruct`),
                    // then recurse on that source's shape.
                    let base_shape = go(code, header, exit, *base, depth + 1)?;
                    let slot_name = base_shape.get(*slot)?.clone();
                    let mut field_shape: Option<Vec<Rc<str>>> = None;
                    for &fsrc in &field_srcs_of(code, header, exit, *base, &slot_name, depth + 1) {
                        let s = go(code, header, exit, fsrc, depth + 1)?;
                        match &field_shape {
                            Some(prev) if *prev != s => return None,
                            _ => field_shape = Some(s),
                        }
                    }
                    let shape = field_shape?;
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                _ => {}
            }
        }
        found
    }
    // Every register that feeds field `name` across ALL of `base`'s `MakeStruct`
    // definitions, resolving `base` through `Move` aliases. (A class can have several
    // `MakeStruct` defs along different paths; each must agree on shape, validated by
    // the caller.)
    fn field_srcs_of(
        code: &[RegInstr],
        header: usize,
        exit: usize,
        base: usize,
        name: &Rc<str>,
        depth: usize,
    ) -> Vec<usize> {
        if depth > 64 {
            return vec![];
        }
        let mut out = Vec::new();
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct { dst, fields, .. } if *dst == base => {
                    if let Some((_, fsrc)) = fields.iter().find(|(n, _)| **n == **name) {
                        out.push(*fsrc);
                    }
                }
                // Resolve through a Move alias: `base = Move(src)` ⇒ inherit src's defs.
                RegInstr::Move { dst, src } if *dst == base => {
                    out.extend(field_srcs_of(code, header, exit, *src, name, depth + 1));
                }
                _ => {}
            }
        }
        out
    }
    go(code, header, exit, reg, 0)
}

/// [`native_scalar_replace_variants_in_region`] but for `MakeStruct`/`GetFieldSlot`,
/// with RECURSIVE (nested-struct) dissolution.
///
/// A struct register `R` is dissolvable iff it is non-escaping and every field is
/// EITHER a scalar OR itself a dissolvable struct register, recursively. The whole
/// nested struct dissolves to ONE fresh register per LEAF SCALAR field (innermost-
/// first): a struct-typed field slot owns no register of its own — it aliases the
/// inner struct register's leaf registers (union-find), so a chained read `a.b.c`
/// becomes plain register moves end-to-end and the nested struct never allocates.
///
/// Membership grows to a fixpoint over three relations: `Move` aliasing, a
/// `MakeStruct` field whose source is itself a struct register (⇒ that slot is
/// struct-typed), and a `GetFieldSlot` reading a struct-typed slot (⇒ its dst is a
/// struct register too). A struct-typed slot's per-slot anchor is unioned with the
/// inner struct register's class, so they literally share leaf registers.
///
/// Bails (conservative; when unsure REJECT) on: any escaping use of a struct register
/// (read as a non-field/non-alias value operand, returned, stored, captured, alive at
/// either OSR boundary), a field that is a heap value or a NON-dissolvable struct, a
/// shape contradiction (scalar in a struct slot or vice-versa), or an unresolvable
/// shape. The flat-struct case is the depth-1 instance of this recursion.
///
/// Rewrite:
/// - `MakeStruct{dst:R}`: scalar field → `Move leaf = src`; nested field → nothing
///   (aliased to the inner's already-written leaf registers).
/// - `GetFieldSlot{dst, base:R, slot}`: scalar slot → `Move dst = leaf`; struct slot →
///   nothing (`dst` aliases the inner's leaf registers).
/// - `Move` struct alias → nothing (shared leaf registers ⇒ self-copies).
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other two region passes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_structs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no `MakeStruct` inside the region ⇒ nothing for THIS pass to do.
    // (A bare `GetFieldSlot` whose base is a handle param stays a native heap read.)
    if !(header..exit).any(|i| is_make_struct_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // Every in-region instruction must be native-subset or a `MakeStruct`.
    for i in header..exit {
        if !native_subset_instruction(&code[i]) && !is_make_struct_op(&code[i]) {
            return None;
        }
    }

    // STR = registers carrying a (replaceable) struct value. Nested support: a struct
    // register is dissolvable when every field is EITHER a scalar OR itself a
    // dissolvable struct register, recursively. We seed STR from in-region
    // `MakeStruct` dsts, then close under THREE relations to a fixpoint:
    //   (1) `Move{dst,src}` with `src` STR ⇒ `dst` is STR (alias);
    //   (2) a `MakeStruct{dst:R}` field whose `src` is STR makes that field a NESTED
    //       (struct-typed) slot of R's shape;
    //   (3) `GetFieldSlot{dst, base:R, slot}` reading a NESTED slot of R ⇒ `dst` is a
    //       STR register (it aliases the inner struct), which can in turn expose more
    //       nested slots / Move-aliases.
    // The slot-kind map (per layout shape, by field name) records whether a slot is
    // scalar or struct-typed; we discover it incrementally as STR membership grows.
    let mut strv = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeStruct { dst, .. } = &code[i] {
            strv[*dst] = true;
        }
    }
    // `nested_slots[shape_field_names] = set of field-name indices that are struct-typed`.
    // Keyed by the layout's `field_names` (the canonical shape) so all structs of the
    // same declared type share one slot-kind classification.
    let mut nested_slots: HashMap<Vec<Rc<str>>, std::collections::HashSet<usize>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for i in header..exit {
            match &code[i] {
                RegInstr::Move { dst, src } => {
                    if strv[*src] && !strv[*dst] {
                        strv[*dst] = true;
                        changed = true;
                    }
                }
                RegInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } if strv[*dst] => {
                    let set = nested_slots.entry(layout.field_names.clone()).or_default();
                    for (name, src) in fields {
                        if strv[*src] {
                            if let Some(slot) =
                                layout.field_names.iter().position(|n| &**n == &**name)
                            {
                                if set.insert(slot) {
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                    // Is `slot` a nested (struct-typed) slot of `base`'s shape? Find
                    // `base`'s shape via any in-region `MakeStruct` defining its class;
                    // for now match against EVERY shape whose nested set contains `slot`
                    // AND that `base` could carry. We resolve `base`'s exact shape during
                    // class shaping; here we conservatively promote `dst` to STR when the
                    // slot is nested under any shape that `base` is built with. Since a
                    // register has a single shape, we look it up from its def.
                    if !strv[*dst] {
                        if let Some(shape) = struct_shape_of_reg(code, header, exit, *base) {
                            if nested_slots.get(&shape).is_some_and(|s| s.contains(slot)) {
                                strv[*dst] = true;
                                changed = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Validate in-region uses/defs of STR registers.
    // - `MakeStruct{dst:R}`: each field `src` is EITHER a scalar (non-STR) OR an STR
    //   register sitting in a nested slot (recorded above). A scalar reg in a nested
    //   slot, or an STR reg in a non-nested slot, is a shape contradiction ⇒ bail.
    // - `Move{dst,src}` writing an STR reg: `src` must be STR (alias copy).
    // - `GetFieldSlot{dst, base:R}`: `dst` is STR exactly when the slot is nested.
    // - Any OTHER read of an STR reg as a value operand ⇒ escape ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeStruct {
                dst,
                layout,
                fields,
            } if strv[*dst] => {
                let set = nested_slots.get(&layout.field_names);
                for (name, src) in fields {
                    let slot = layout.field_names.iter().position(|n| &**n == &**name)?;
                    let is_nested = set.is_some_and(|s| s.contains(&slot));
                    if strv[*src] != is_nested {
                        return None; // scalar-in-struct-slot or struct-in-scalar-slot
                    }
                }
            }
            RegInstr::Move { dst, src } if strv[*dst] => {
                if !strv[*src] {
                    return None;
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                let shape = struct_shape_of_reg(code, header, exit, *base)?;
                let is_nested = nested_slots.get(&shape).is_some_and(|s| s.contains(slot));
                if strv[*dst] != is_nested {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if strv[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| strv[r]) {
                    return None;
                }
                if let RegInstr::GetFieldSlot { dst, .. } | RegInstr::MakeStruct { dst, .. } = other
                {
                    if strv[*dst] {
                        return None;
                    }
                }
            }
        }
    }

    // Dead-at-boundary soundness (identical argument to the Option/variant passes):
    // every STR register must be neither read nor written by ANY instruction OUTSIDE
    // the region, so the original register slot is never observed across either OSR
    // boundary. A footprint we cannot model reports `All` ⇒ bail.
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    if analysis
        .external_reads_or_writes_touch(code, &strv)
        .unwrap_or(true)
    {
        return None;
    }

    // Alias union-find. STR registers that name the SAME logical struct value share
    // ONE leaf-register layout. We union over:
    //   (a) in-region `Move{dst,src}` where both are STR (plain alias);
    //   (b) `MakeStruct{dst:R, field src}` nested slot ⇒ union R's per-slot ANCHOR with
    //       `src`'s class (the slot IS the inner struct's registers);
    //   (c) `GetFieldSlot{dst, base:R, slot}` nested ⇒ union `dst` with R's slot anchor.
    // Per-slot anchors are virtual ids ≥ n_regs (one per (class-representative, slot)).
    // Because anchors are created lazily and keyed by a register's find-root, we resolve
    // them through a small interning map below.
    let mut parent: Vec<usize> = (0..n_regs).collect();
    fn find(parent: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != c {
            let n = parent[c];
            parent[c] = r;
            c = n;
        }
        r
    }
    fn union(parent: &mut Vec<usize>, a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    // Lazily intern a virtual anchor id for the (struct-value, slot) pair. Keyed by the
    // current find-root of `base` so all aliases of `base` share the anchor.
    fn anchor_of(
        parent: &mut Vec<usize>,
        anchors: &mut HashMap<(usize, usize), usize>,
        base: usize,
        slot: usize,
    ) -> usize {
        let root = find(parent, base);
        if let Some(&a) = anchors.get(&(root, slot)) {
            return a;
        }
        let id = parent.len();
        parent.push(id);
        anchors.insert((root, slot), id);
        id
    }
    let mut anchors: HashMap<(usize, usize), usize> = HashMap::new();
    // (a) plain Move aliases.
    for i in header..exit {
        if let RegInstr::Move { dst, src } = &code[i] {
            if strv[*dst] && strv[*src] {
                union(&mut parent, *dst, *src);
            }
        }
    }
    // (b)+(c): union nested-slot anchors with their inner struct registers. Iterate to a
    // fixpoint because anchors are keyed by find-roots that the unions themselves change.
    let mut changed = true;
    while changed {
        changed = false;
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } if strv[*dst] => {
                    for (name, src) in fields {
                        if strv[*src] {
                            let slot = layout.field_names.iter().position(|n| &**n == &**name)?;
                            let a = anchor_of(&mut parent, &mut anchors, *dst, slot);
                            let before = find(&mut parent, a);
                            union(&mut parent, a, *src);
                            if find(&mut parent, a) != before {
                                changed = true;
                            }
                        }
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] && strv[*dst] => {
                    let a = anchor_of(&mut parent, &mut anchors, *base, *slot);
                    let before = find(&mut parent, *dst);
                    union(&mut parent, a, *dst);
                    if find(&mut parent, *dst) != before {
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Determine ONE canonical layout shape (field_names) per alias class from its
    // `MakeStruct` defs. All defs in a class must agree on the shape.
    let mut class_shape: HashMap<usize, Vec<Rc<str>>> = HashMap::new();
    for i in header..exit {
        if let RegInstr::MakeStruct { dst, layout, .. } = &code[i] {
            if strv[*dst] {
                let root = find(&mut parent, *dst);
                let shape = layout.field_names.clone();
                match class_shape.get(&root) {
                    Some(existing) if *existing != shape => return None, // shape mismatch
                    Some(_) => {}
                    None => {
                        class_shape.insert(root, shape);
                    }
                }
            }
        }
    }

    // Allocate one fresh LEAF register per SCALAR slot, per alias class. A nested slot
    // owns no register here — it resolves through its anchor to the inner class's leaf
    // registers. `class_slot_reg[(root, slot)]` is the leaf reg for a scalar slot.
    let mut next_reg = parent.len();
    let mut class_slot_reg: HashMap<(usize, usize), usize> = HashMap::new();
    let roots: Vec<usize> = class_shape.keys().copied().collect();
    for root in roots {
        let shape = class_shape.get(&root).cloned().expect("root has shape");
        let nested = nested_slots.get(&shape).cloned().unwrap_or_default();
        for slot in 0..shape.len() {
            if !nested.contains(&slot) {
                let r = next_reg;
                next_reg += 1;
                class_slot_reg.insert((root, slot), r);
            }
        }
    }
    // The leaf register backing `reg`'s scalar field `slot` (resolving Move aliases).
    let scalar_slot_reg = |parent: &mut Vec<usize>,
                           class_slot_reg: &HashMap<(usize, usize), usize>,
                           reg: usize,
                           slot: usize|
     -> usize {
        let root = find(parent, reg);
        *class_slot_reg
            .get(&(root, slot))
            .expect("scalar slot has a leaf register")
    };
    let slot_of_name = |parent: &mut Vec<usize>,
                        class_shape: &HashMap<usize, Vec<Rc<str>>>,
                        reg: usize,
                        name: &str|
     -> usize {
        let root = find(parent, reg);
        class_shape
            .get(&root)
            .and_then(|shape| shape.iter().position(|n| &**n == name))
            .expect("field name in class shape")
    };

    // Pre-flight: every struct op the rewrite will touch must resolve (every STR class
    // has a shape; every SCALAR slot it reads/writes has an allocated leaf register).
    // Bail rather than panic on any unresolved case — conservative REJECT.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeStruct {
                dst,
                layout,
                fields,
            } if strv[*dst] => {
                let root = find(&mut parent, *dst);
                if class_shape.get(&root) != Some(&layout.field_names) {
                    return None;
                }
                for (name, src) in fields {
                    if strv[*src] {
                        continue;
                    }
                    let slot = layout.field_names.iter().position(|n| &**n == &**name)?;
                    if !class_slot_reg.contains_key(&(root, slot)) {
                        return None;
                    }
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                let root = find(&mut parent, *base);
                let shape = class_shape.get(&root)?;
                if *slot >= shape.len() {
                    return None;
                }
                if !strv[*dst] && !class_slot_reg.contains_key(&(root, *slot)) {
                    return None;
                }
            }
            _ => {}
        }
    }

    // Rewrite the whole code, scalar-replacing in-region struct ops and copying the
    // rest through, remapping jump/match targets through the index map.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeStruct { dst, fields, .. } if region && strv[*dst] => {
                // For each SCALAR field, move the source into its slot's leaf register.
                // A NESTED field is a struct-typed slot whose per-slot anchor is unioned
                // with the inner struct register's class, so the inner's leaf registers
                // ARE this slot's registers — they were already written by the inner's
                // own (earlier) `MakeStruct`. Emit nothing for it.
                for (name, src) in fields {
                    if strv[*src] {
                        continue; // nested struct field: aliased, no copy needed
                    }
                    let slot = slot_of_name(&mut parent, &class_shape, *dst, name);
                    let leaf = scalar_slot_reg(&mut parent, &class_slot_reg, *dst, slot);
                    new_code.push(RegInstr::Move {
                        dst: leaf,
                        src: *src,
                    });
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if region && strv[*base] => {
                if strv[*dst] {
                    // Reading a struct-typed slot: `dst` is unioned with `base`'s slot
                    // anchor, so `dst` already names the inner struct's leaf registers.
                    // A subsequent `GetFieldSlot{base:dst, inner_slot}` reads the inner's
                    // scalar leaf directly — the `a.b.c` chain collapses to register
                    // moves end-to-end. Emit nothing here.
                } else {
                    let leaf = scalar_slot_reg(&mut parent, &class_slot_reg, *base, *slot);
                    new_code.push(RegInstr::Move {
                        dst: *dst,
                        src: leaf,
                    });
                }
            }
            RegInstr::Move { dst, src } if region && strv[*dst] => {
                // Plain struct alias: `dst` and `src` share an alias class ⇒ identical
                // leaf registers ⇒ every per-slot copy is a self-Move. Emit nothing.
                let _ = (dst, src);
            }
            // Copy-through, remapping jump/match targets.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × J3 (loop-carried struct scalar replacement). Extends
/// [`native_scalar_replace_structs_in_region`] to a struct that is CREATED BEFORE the
/// loop, MUTATED IN PLACE across iterations (`SetFieldSlot`), and DEAD after the loop.
///
/// The shipped struct pass only dissolves a struct allocated AND dead INSIDE the loop
/// (the `MakeStruct` lives in the region; the dead-at-boundary gate forbids any
/// outside reference). A struct mutated loop-carried fails that pass two ways: its
/// `MakeStruct` sits in the PRE-HEADER (so the strict "every in-region instr is
/// native-subset or `MakeStruct`" check trips on the in-loop `SetFieldSlot`, which the
/// shipped pass does not even model), and the struct register is WRITTEN before the
/// region (so the dead-at-boundary gate bails). The in-loop `SetFieldSlot` is a heap
/// write the native tier cannot perform, so without this pass the loop never OSRs.
///
/// This pass dissolves such a struct into one LOOP-CARRIED scalar register per field:
/// - the pre-header `MakeStruct{m}` + `Move{p, m}` becomes nothing — each field's
///   INIT SOURCE register is REUSED as that field's loop-carried leaf register, so the
///   interpreter has already written it (definite assignment) before the header and it
///   marshals live-in through the normal `try_osr` window with no new channel;
/// - in-loop `GetFieldSlot{dst, base:p, slot}` → `Move dst := leaf_slot` (register read);
/// - in-loop `SetFieldSlot{dst, base:p, slot, value}` → `Move leaf_slot := value`
///   (the heap write becomes a register write) plus `LoadUnit dst` (its old `dst`,
///   which the interpreter set to `Unit`, is preserved in case it is read);
/// - in-loop self-`Move{p, p}` (the lowerer's redundant copy after each `SetFieldSlot`)
///   → nothing.
/// The leaf registers are loop-carried (live-in at the header, carried across the
/// backedge, in the OSR window). Because `p` is DEAD after the loop, no heap struct is
/// reconstructed at the OSR-exit; the scalar leaves simply flow out (each leaf is a
/// real register `< n_regs`, restored by the precise deopt — harmless, since dead).
///
/// Conservative bails (when unsure REJECT — never unsound): no in-region
/// `SetFieldSlot` (nothing for THIS pass — return unchanged); the struct ESCAPES (read
/// as a plain value / stored / returned / captured / any non-(Get/Set)FieldSlot,
/// non-alias use); the struct is LIVE after the loop (read at/after `exit` ⇒ would need
/// heap reconstruction ⇒ out of scope); the pre-header def is not a clean
/// `MakeStruct` (single def reachable through `Move` aliases); a field INIT SOURCE
/// register is not REUSABLE as a leaf (shared between two fields, or read/written
/// anywhere other than the defining `MakeStruct` — reuse would corrupt a live value);
/// two struct handles that might alias the same heap object; a footprint we cannot
/// model (`RegFootprint::All`). Every existing loop-LOCAL / nested struct behavior is
/// untouched — this pass only fires when the shipped struct pass could not (an
/// in-region `SetFieldSlot` on a pre-header struct).
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other region passes; `new_n_regs == n_regs` (no fresh
/// regs — leaves reuse the init sources).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_loop_carried_struct_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;
    let identity = || -> (Vec<RegInstr>, usize, Vec<usize>) {
        (code.to_vec(), n_regs, (0..code.len()).collect())
    };

    // Fast path: no in-region `SetFieldSlot` ⇒ nothing for THIS pass. (A loop-LOCAL
    // struct, or a bare native-subset body, is handled by the earlier passes.)
    if !(header..exit).any(|i| matches!(&code[i], RegInstr::SetFieldSlot { .. })) {
        return Some(identity());
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // Candidate struct handles: every register that is the `base` of an in-region
    // `GetFieldSlot`/`SetFieldSlot`. We require EXACTLY ONE such base register (a
    // single loop-carried struct); multiple distinct bases (possible aliasing of two
    // heap structs) ⇒ bail. A `base` that is a Move-alias of the same pre-header
    // struct still appears as one register here because the lowering threads the
    // in-place mutation back through that one register (`SetFieldSlot` rewrites its
    // `base` slot, and the self-`Move{p,p}` keeps it that register).
    let mut base_regs: Vec<usize> = Vec::new();
    for i in header..exit {
        match &code[i] {
            RegInstr::GetFieldSlot { base, .. } | RegInstr::SetFieldSlot { base, .. } => {
                if !base_regs.contains(base) {
                    base_regs.push(*base);
                }
            }
            _ => {}
        }
    }
    if base_regs.len() != 1 {
        return None;
    }
    let p = base_regs[0];
    if p >= n_regs {
        return None;
    }

    // The pre-header definition of `p`: follow `Move{p, src}` aliases backward to a
    // single `MakeStruct`. `p` must have EXACTLY ONE writer outside the region, that
    // writer must be either a `MakeStruct{p,..}` directly or `Move{p, m}` where `m`
    // has exactly one writer, a `MakeStruct{m,..}`, and `m` is used ONLY by that Move.
    // All such defs must precede the region (a pre-header def).
    //
    // `instr_written_reg` reports the literal `dst` field. A struct handle mutated in
    // place by `SetFieldSlot{base:p}` writes `p`'s slot at RUNTIME but its modelled
    // `dst` is the (unused) result reg — so a SetFieldSlot does NOT appear as a writer
    // of `p` here. What DOES is the lowerer's redundant self-`Move{p,p}` emitted after
    // each in-region `SetFieldSlot` (a semantic no-op). Exclude those self-moves so the
    // sole REAL writer of `p` is its pre-header definition.
    let is_self_move = |i: usize| matches!(&code[i], RegInstr::Move { dst, src } if dst == src);
    let p_writers: Vec<usize> = analysis
        .writer_ips_of(code, p)?
        .into_iter()
        .filter(|&i| !(in_region(i) && is_self_move(i)))
        .collect();
    if p_writers.len() != 1 {
        return None;
    }
    let p_def = p_writers[0];
    if in_region(p_def) || p_def >= header {
        return None;
    }
    // Resolve the `MakeStruct` providing `p`'s fields (directly, or through one Move).
    let make_idx = match &code[p_def] {
        RegInstr::MakeStruct { dst, .. } if *dst == p => p_def,
        RegInstr::Move { dst, src } if *dst == p => {
            let m = *src;
            let m_writers = analysis.writer_ips_of(code, m)?;
            if m_writers.len() != 1 {
                return None;
            }
            let mi = m_writers[0];
            if !matches!(&code[mi], RegInstr::MakeStruct { dst, .. } if *dst == m) {
                return None;
            }
            // `m` must be used ONLY by this Move (otherwise the struct also flows
            // elsewhere — an escape we are not modelling).
            for (i, instr) in code.iter().enumerate() {
                if i == mi {
                    continue;
                }
                let reads = match instr_read_regs(instr) {
                    RegFootprint::Some(rs) => rs,
                    RegFootprint::All => return None,
                };
                if reads.contains(&m) && i != p_def {
                    return None;
                }
            }
            mi
        }
        _ => return None,
    };
    if make_idx >= header {
        return None;
    }
    let RegInstr::MakeStruct { layout, fields, .. } = &code[make_idx] else {
        return None;
    };

    // Non-escaping + dead-after-loop check on `p`. EVERY read of `p` in the whole
    // function must be: an in-region `GetFieldSlot`/`SetFieldSlot` base, an in-region
    // self-`Move{p,p}`, or the pre-header `Move{p_def}` source-side... `p` is never a
    // Move SOURCE in the pre-header (it is the dst there). So: every read of `p` must
    // be a (Get/Set)FieldSlot base or a self-Move — both IN-REGION. Any read of `p`
    // OUTSIDE the region (incl. at/after `exit`) ⇒ live-after-loop or escape ⇒ bail.
    for (i, instr) in code.iter().enumerate() {
        let reads = match instr_read_regs(instr) {
            RegFootprint::Some(rs) => rs,
            RegFootprint::All => return None,
        };
        if !reads.contains(&p) {
            continue;
        }
        match instr {
            RegInstr::GetFieldSlot { base, .. } | RegInstr::SetFieldSlot { base, .. }
                if *base == p && in_region(i) => {}
            RegInstr::Move { dst, src } if *dst == p && *src == p && in_region(i) => {}
            _ => return None, // escapes or live after the loop
        }
    }
    // `p` must not be WRITTEN anywhere except its single pre-header def and the
    // in-region redundant self-`Move{p,p}` (deleted by the rewrite). `SetFieldSlot`
    // does not model `p` as a written reg (its `dst` is the unused result). Any OTHER
    // writer of `p` (e.g. a second struct construction on another path) ⇒
    // aliasing/polymorphism ⇒ bail.
    for i in 0..code.len() {
        if i == p_def {
            continue;
        }
        if in_region(i) && is_self_move(i) {
            continue; // redundant self-Move, deleted below
        }
        let writes = match instr_written_reg(&code[i]) {
            RegFootprint::Some(ws) => ws,
            RegFootprint::All => return None,
        };
        if writes.contains(&p) {
            return None;
        }
    }

    // Allocate one loop-carried leaf register per field slot by REUSING the field's
    // init-source register. Reuse is sound iff the source register is used ONLY as
    // that single `MakeStruct` field source (so the native loop owning/overwriting it
    // corrupts nothing) and the sources are pairwise DISTINCT (each leaf is its own
    // register). Build `slot -> leaf` from the layout's declaration order.
    let n_slots = layout.field_names.len();
    let mut slot_leaf: Vec<Option<usize>> = vec![None; n_slots];
    let mut seen_leaves: Vec<usize> = Vec::new();
    for (name, src) in fields.iter() {
        let slot = layout.field_names.iter().position(|n| &**n == &**name)?;
        if slot >= n_slots {
            return None;
        }
        if slot_leaf[slot].is_some() {
            return None; // duplicate field
        }
        // Distinct across fields.
        if seen_leaves.contains(src) {
            return None;
        }
        // The source must be a real interpreter register that the OSR window can
        // marshal (`< n_regs`).
        if *src >= n_regs {
            return None;
        }
        // The source must be used ONLY by this `MakeStruct` (any other read would be
        // clobbered when the native loop overwrites the leaf). Reads elsewhere ⇒ bail.
        for (i, instr) in code.iter().enumerate() {
            if i == make_idx {
                continue;
            }
            let reads = match instr_read_regs(instr) {
                RegFootprint::Some(rs) => rs,
                RegFootprint::All => return None,
            };
            if reads.contains(src) {
                return None;
            }
        }
        // The source must have a single writer (its pre-header init), and that writer
        // must precede the region. (It feeds the live-in marshalling.)
        let src_writers = analysis.writer_ips_of(code, *src)?;
        if src_writers.len() != 1 || src_writers[0] >= header {
            return None;
        }
        slot_leaf[slot] = Some(*src);
        seen_leaves.push(*src);
    }
    if slot_leaf.iter().any(|s| s.is_none()) {
        return None; // a slot had no field source
    }
    let slot_leaf: Vec<usize> = slot_leaf.into_iter().map(|s| s.expect("checked")).collect();

    // Each in-region `SetFieldSlot{dst}` writes `dst := Unit` at runtime (an unused
    // result of the in-place mutation). The rewrite turns the SetFieldSlot into a plain
    // `Move leaf := value` and emits NOTHING for the (dead) `dst`. Require every such
    // `dst` to be DEAD (never read anywhere in the function) so dropping its `Unit`
    // write is observationally invisible; a read of it would need a `LoadUnit` (not in
    // the native subset) ⇒ bail instead.
    for i in header..exit {
        if let RegInstr::SetFieldSlot { dst, base, .. } = &code[i] {
            if *base != p {
                continue;
            }
            for (j, instr) in code.iter().enumerate() {
                let reads = match instr_read_regs(instr) {
                    RegFootprint::Some(rs) => rs,
                    RegFootprint::All => return None,
                };
                if j != i && reads.contains(dst) {
                    return None;
                }
            }
        }
    }

    // Rewrite. The pre-header `MakeStruct` (and its forwarding `Move{p, m}`) are
    // DELETED — the field sources already hold the init values as the leaf registers.
    // In-region field ops become register Moves. Everything else copies through with
    // jump/match targets remapped via the index map.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            // Pre-header struct construction + its forwarding Move: delete (leaves are
            // the init-source registers, already written by the interpreter).
            RegInstr::MakeStruct { dst, .. } if i == make_idx && *dst == p => {}
            RegInstr::MakeStruct { dst, .. } if i == make_idx => {
                // The `MakeStruct{m}` whose result a `Move{p,m}` forwards: delete it;
                // its dst `m` is otherwise unused (validated above).
                let _ = dst;
            }
            RegInstr::Move { dst, src } if i == p_def && *dst == p && *src != p => {
                // The forwarding `Move{p, m}`: delete (m is gone).
            }
            // In-region field reads/writes on `p`.
            RegInstr::GetFieldSlot { dst, base, slot } if in_region(i) && *base == p => {
                let leaf = *slot_leaf.get(*slot)?;
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            RegInstr::SetFieldSlot {
                base, slot, value, ..
            } if in_region(i) && *base == p => {
                let leaf = *slot_leaf.get(*slot)?;
                // The heap write becomes a register write into the loop-carried leaf.
                // The SetFieldSlot's `dst` (the unused `Unit` result) is validated dead
                // above, so emit nothing for it.
                new_code.push(RegInstr::Move {
                    dst: leaf,
                    src: *value,
                });
            }
            // The lowerer's redundant self-`Move{p,p}` after each SetFieldSlot.
            RegInstr::Move { dst, src } if in_region(i) && *dst == p && *src == p => {}
            // Copy-through, remapping jump/match targets.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`). A deleted pre-header
    // instruction maps its (empty) range to the next emitted index; the OSR boundary
    // (header/exit) is in-region/post-region control flow, never a deleted pre-header
    // index, so the boundary mapping stays unambiguous.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, n_regs, ip_map))
}

/// Registers an instruction READS (value operands). Returns [`RegFootprint::All`]
/// for any variant whose read set we do not exhaustively model. Used by OSR × J3 to
/// prove a scalar-replaced Option register is dead at the loop boundary.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn instr_read_regs(instr: &RegInstr) -> RegFootprint {
    native_instr_semantics(instr).reads
}

/// The register an instruction WRITES (its `dst`), or [`RegFootprint::All`] for a
/// variant we do not model (treated as writing every register — sound).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn instr_written_reg(instr: &RegInstr) -> RegFootprint {
    native_instr_semantics(instr).writes
}

/// Fuse a heap field read that is used only as the closure operand for native
/// closure metadata reads:
///
/// ```text
/// f  = GetFieldSlot(op, slot)      // lowers to FieldHandle
/// id = NativeClosureId(f)
/// c0 = NativeClosureCapture(f, 0)
/// ```
///
/// becomes:
///
/// ```text
/// f  = Move(op)                    // dead handle-shaped placeholder
/// id = NativeFieldClosureId(op, slot)
/// c0 = NativeFieldClosureCapture(op, slot, 0)
/// ```
///
/// Keeping the instruction count stable preserves all branch/ip maps. The dummy
/// `Move` writes the old destination register without performing a host helper call;
/// the pass only fires when every read of that register is rewritten.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_fuse_field_closure_metadata_reads(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    let mut field_defs: Vec<Option<(usize, usize, usize)>> = vec![None; n_regs];
    let mut invalid = vec![false; n_regs];

    for (ip, instr) in code.iter().enumerate() {
        // A field-closure def captures its base register's value at def time, so a
        // later read fused into `GetFieldSlot(base, slot)` is only sound while `base`
        // still holds that value. If `base` is clobbered between the def and the read,
        // invalidate every def that captured it — otherwise the fused read resolves
        // against the new base and dispatches the wrong closure.
        match instr_written_reg(instr) {
            RegFootprint::Some(writes) => {
                for w in writes {
                    if w < n_regs {
                        for def in field_defs.iter_mut() {
                            if def.is_some_and(|(base, _, _)| base == w) {
                                *def = None;
                            }
                        }
                    }
                }
            }
            RegFootprint::All => {
                field_defs.fill(None);
                invalid.fill(true);
            }
        }
        match instr {
            RegInstr::GetFieldSlot { dst, base, slot } if *dst < n_regs && !invalid[*dst] => {
                if field_defs[*dst].is_none() {
                    field_defs[*dst] = Some((*base, *slot, ip));
                } else {
                    field_defs[*dst] = None;
                    invalid[*dst] = true;
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*src].is_some()
                    && !invalid[*dst] =>
            {
                if field_defs[*dst].is_none() {
                    field_defs[*dst] = field_defs[*src];
                } else {
                    field_defs[*dst] = None;
                    invalid[*dst] = true;
                }
            }
            _ => match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    for dst in writes {
                        if dst < n_regs {
                            field_defs[dst] = None;
                            invalid[dst] = true;
                        }
                    }
                }
                RegFootprint::All => {
                    field_defs.fill(None);
                    invalid.fill(true);
                }
            },
        }
    }

    let mut reads = vec![0usize; n_regs];
    let mut rewritable_reads = vec![0usize; n_regs];
    for (ip, instr) in code.iter().enumerate() {
        match instr {
            RegInstr::NativeClosureId { closure, .. }
            | RegInstr::NativeClosureCapture { closure, .. }
                if *closure < n_regs && field_defs[*closure].is_some() =>
            {
                reads[*closure] += 1;
                if field_defs[*closure].is_some_and(|(_, _, def_ip)| def_ip < ip) {
                    rewritable_reads[*closure] += 1;
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*src].is_some()
                    && field_defs[*dst] == field_defs[*src] =>
            {
                reads[*src] += 1;
                if field_defs[*src].is_some_and(|(_, _, def_ip)| def_ip < ip) {
                    rewritable_reads[*src] += 1;
                }
            }
            _ => match instr_read_regs(instr) {
                RegFootprint::Some(regs) => {
                    for reg in regs {
                        if reg < n_regs {
                            reads[reg] += 1;
                        }
                    }
                }
                RegFootprint::All => {
                    for (reg, def) in field_defs.iter().enumerate() {
                        if def.is_some() {
                            reads[reg] += 1;
                        }
                    }
                }
            },
        }
    }

    let can_rewrite = |reg: usize| {
        let Some(def) = (reg < n_regs).then(|| field_defs[reg]).flatten() else {
            return false;
        };
        let mut group_has_read = false;
        for (alias, alias_def) in field_defs.iter().enumerate() {
            if *alias_def == Some(def) {
                group_has_read |= reads[alias] > 0;
                if reads[alias] != rewritable_reads[alias] {
                    return false;
                }
            }
        }
        reg < n_regs && group_has_read && reads[reg] == rewritable_reads[reg]
    };

    let mut changed = false;
    let mut out = Vec::with_capacity(code.len());
    for instr in code {
        let rewritten = match instr {
            RegInstr::GetFieldSlot { dst, base, .. } if can_rewrite(*dst) => {
                changed = true;
                RegInstr::Move {
                    dst: *dst,
                    src: *base,
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*dst] == field_defs[*src]
                    && can_rewrite(*src) =>
            {
                let (base, _, _) = field_defs[*src]?;
                changed = true;
                RegInstr::Move {
                    dst: *dst,
                    src: base,
                }
            }
            RegInstr::NativeClosureId { dst, closure } if can_rewrite(*closure) => {
                let (base, slot, _) = field_defs[*closure]?;
                changed = true;
                RegInstr::NativeFieldClosureId {
                    dst: *dst,
                    base,
                    slot,
                }
            }
            RegInstr::NativeClosureCapture {
                dst,
                closure,
                index,
            } if can_rewrite(*closure) => {
                let (base, slot, _) = field_defs[*closure]?;
                changed = true;
                RegInstr::NativeFieldClosureCapture {
                    dst: *dst,
                    base,
                    slot,
                    index: *index,
                }
            }
            other => other.clone(),
        };
        out.push(rewritten);
    }

    Some(if changed {
        (out, n_regs, (0..code.len()).collect())
    } else {
        (code.to_vec(), n_regs, (0..code.len()).collect())
    })
}

/// The result of [`loop_local_sinkable_closures`].
#[cfg(feature = "native-jit")]
#[derive(Default)]
pub(in crate::reg_vm) struct SinkableClosures {
    /// `call_operand_reg -> callee_id`: a `CallClosure{closure: reg}` whose `reg` is a
    /// sunk closure-value register dispatches statically to `callee_id`; its body is
    /// inlined at the call site.
    sink_calls: std::collections::HashMap<usize, usize>,
    /// `MakeClosure`/copy-`Move` instruction indices to DELETE: the heap alloc and the
    /// pure copies that only forward the (now non-existent) closure value.
    dead_defs: std::collections::HashSet<usize>,
    /// `call_operand_reg -> capture_regs`: the capture registers from the unique
    /// sunk `MakeClosure` definition behind that operand.
    capture_regs: std::collections::HashMap<usize, Vec<usize>>,
}

/// OSR × closure-allocation sinking. A `MakeClosure{dst, function:k, captures}` whose
/// closure value is **loop-local and non-escaping** — flowing (possibly through pure
/// copy `Move`s) ONLY into the `closure` operand of `CallClosure` sites, never stored,
/// returned, captured, or read as a plain value — can have its heap allocation
/// dissolved: the callee `k` is known STATICALLY from the `MakeClosure` (no profile
/// needed), so its body is inlined at every call site and the `MakeClosure` (plus its
/// dead copy `Move`s) is deleted. The loop then becomes pure-scalar and OSRs.
///
/// The analysis builds, for each in-region `MakeClosure`, the **closure-value set**
/// `S` = its dst plus every register that is a pure `Move`-copy of a member of `S`.
/// It is sinkable iff ALL of the following hold (any failure ⇒ skip it; the alloc
/// stays on its normal heap path — behavior unchanged):
/// - every register in `S` has a SINGLE definition (the `MakeClosure`, or a `Move`
///   from another `S` member) — a second definition of any `S` register (e.g. a
///   `MakeClosure` of a different callee on another path) is polymorphic ⇒ bail;
/// - EVERY read of any `S` register is either a copy-`Move` into another `S` member
///   or the `closure` operand of a `CallClosure` (never an `arg`, never any other
///   instruction — i.e. the closure value never escapes as a value); every such
///   `CallClosure` is in the region with no `mut` args;
/// - the callee `k` is native-inlinable at the call arity
///   ([`native_callee_inlinable`] when captureless, else
///   [`native_capturing_callee_inlinable`]) and `captures.len() == callee.captures`.
///   Each scalar capture is materialized at the inline site by a plain `Move` of the
///   (already-live) capture register; a non-scalar (heap) capture is caught by the
///   downstream OSR type inference (Int/Bool/Float only — the same safety net the
///   other region passes rely on), so such a body simply fails to compile.
/// An instruction that produces/consumes only scalar (Int/Bool/Float) values — no
/// heap operand. A heap value can only be consumed by a NON-scalar instruction, so
/// a register read exclusively by scalar-only instructions cannot hold a heap value.
#[cfg(feature = "native-jit")]
fn native_instr_scalar_only(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::LoadInt { .. }
            | RegInstr::LoadFloat { .. }
            | RegInstr::LoadBool { .. }
            | RegInstr::LoadUnit { .. }
            | RegInstr::Move { .. }
            | RegInstr::AddInt { .. }
            | RegInstr::SubInt { .. }
            | RegInstr::MulInt { .. }
            | RegInstr::DivInt { .. }
            | RegInstr::ModInt { .. }
            | RegInstr::BitAndInt { .. }
            | RegInstr::BitOrInt { .. }
            | RegInstr::BitXorInt { .. }
            | RegInstr::ShiftLeftInt { .. }
            | RegInstr::ShiftRightInt { .. }
            | RegInstr::LessInt { .. }
            | RegInstr::LessEqualInt { .. }
            | RegInstr::GreaterInt { .. }
            | RegInstr::GreaterEqualInt { .. }
            | RegInstr::Equal { .. }
            | RegInstr::NotEqual { .. }
            | RegInstr::Jump { .. }
            | RegInstr::JumpIfBool { .. }
            | RegInstr::JumpIfIntCompare { .. }
            | RegInstr::Return { .. }
            | RegInstr::RuntimeError { .. }
    )
}

/// Prove every capture of `callee` is a SCALAR value. Capture slots are callee regs
/// `0..callee.captures`; follow pure copy-`Move`s so an aliased capture is tracked.
/// If every read of a capture-derived register is [`native_instr_scalar_only`], the
/// captures are all scalar (a heap capture would necessarily be consumed by a
/// non-scalar op). Conservative: any non-scalar (or `All`-reading) use fails the
/// proof. This is the static analog of the runtime `captures_all_scalar` bit the
/// monomorphic inline path uses — needed because OSR type inference now admits
/// `Handle`, so it no longer rejects a sunk heap capture on its own.
#[cfg(feature = "native-jit")]
fn native_callee_captures_all_scalar(callee: &RegFunction) -> bool {
    use std::collections::HashSet;
    if callee.captures == 0 {
        return true;
    }
    let mut derived: HashSet<usize> = (0..callee.captures).collect();
    loop {
        let mut grew = false;
        for instr in &callee.code {
            if let RegInstr::Move { dst, src } = instr {
                if derived.contains(src) && !derived.contains(dst) {
                    derived.insert(*dst);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    callee.code.iter().all(|instr| {
        let reads_capture = match instr_read_regs(instr) {
            RegFootprint::Some(regs) => regs.iter().any(|r| derived.contains(r)),
            RegFootprint::All => true,
        };
        !reads_capture || native_instr_scalar_only(instr)
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn loop_local_sinkable_closures(
    unit: &RegUnit,
    func: &RegFunction,
    header: usize,
    exit: usize,
    j3: bool,
) -> SinkableClosures {
    use std::collections::HashSet;
    let in_region = |i: usize| i >= header && i < exit;
    let mut out = SinkableClosures::default();

    'candidates: for (mi, instr) in func.code.iter().enumerate() {
        let RegInstr::MakeClosure {
            dst: c,
            function: k,
            captures,
        } = instr
        else {
            continue;
        };
        if !in_region(mi) {
            continue;
        }
        let k = *k;
        let callee = match unit.functions.get(k) {
            Some(callee) => callee,
            None => continue,
        };

        // Grow the closure-value set `S` by following pure copy-`Move`s: a
        // `Move{dst:d, src:s}` with `s in S` adds `d` to `S`. Record the defining
        // instruction index of each member (the MakeClosure or the copy Move).
        let mut value_regs: HashSet<usize> = HashSet::new();
        value_regs.insert(*c);
        let mut def_indices: HashSet<usize> = HashSet::new();
        def_indices.insert(mi);
        // Fixpoint over the copy graph (bounded by code length).
        loop {
            let mut grew = false;
            for (di, dinstr) in func.code.iter().enumerate() {
                if let RegInstr::Move { dst, src } = dinstr {
                    if value_regs.contains(src) && !value_regs.contains(dst) {
                        value_regs.insert(*dst);
                        def_indices.insert(di);
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }

        // Single-definition guard: every `S` register must be written ONLY by a
        // recorded def (the MakeClosure or an `S`-copy Move). Any other writer means
        // the value is redefined on some path (polymorphic / clobbered) ⇒ bail.
        for (wi, winstr) in func.code.iter().enumerate() {
            if def_indices.contains(&wi) {
                continue;
            }
            let writes_value = match instr_written_reg(winstr) {
                RegFootprint::Some(regs) => regs.iter().any(|r| value_regs.contains(r)),
                RegFootprint::All => true,
            };
            if writes_value {
                continue 'candidates;
            }
        }

        // Non-escape: every read of an `S` register must be a copy-`Move` (already a
        // recorded def) or the `closure` operand of an in-region `CallClosure` with no
        // `mut` args, where the value is NOT also an arg.
        let mut call_operands: Vec<usize> = Vec::new();
        for (ri, rinstr) in func.code.iter().enumerate() {
            if def_indices.contains(&ri) {
                // A recorded copy-Move: it reads an `S` reg by construction; fine.
                continue;
            }
            match rinstr {
                RegInstr::CallClosure {
                    closure,
                    args,
                    mut_args,
                    ..
                } if value_regs.contains(closure) => {
                    if args.iter().any(|a| value_regs.contains(a))
                        || !mut_args.is_empty()
                        || !in_region(ri)
                    {
                        continue 'candidates;
                    }
                    // Under j3 (OSR×inline) a mapper that builds/destructures a
                    // non-escaping Option/Result/variant/struct also qualifies — it
                    // dissolves post-inline via the region SR passes (e.g. an
                    // `and_then` mapper `|v| Some(v*2)` / `|v| Ok(v*2)`).
                    let inlinable = if callee.captures == 0 {
                        if j3 {
                            native_callee_inlinable_j3(callee, args.len())
                        } else {
                            native_callee_inlinable(callee, args.len())
                        }
                    } else {
                        native_capturing_callee_inlinable(callee, args.len())
                    };
                    if callee.captures != captures.len()
                        || !inlinable
                        || !native_callee_captures_all_scalar(callee)
                    {
                        continue 'candidates;
                    }
                    call_operands.push(*closure);
                }
                _ => {
                    let reads_value = match instr_read_regs(rinstr) {
                        RegFootprint::Some(regs) => regs.iter().any(|r| value_regs.contains(r)),
                        RegFootprint::All => true,
                    };
                    if reads_value {
                        continue 'candidates;
                    }
                }
            }
        }
        if call_operands.is_empty() {
            continue;
        }
        for op in call_operands {
            out.sink_calls.insert(op, k);
            out.capture_regs.insert(op, captures.clone());
        }
        out.dead_defs.extend(def_indices);
    }
    out
}

#[cfg(feature = "native-jit")]
/// Inline straight-line leaf `CallKnown`/closure calls, returning the rewritten
/// code, the new register count, AND a transformed→original ip-map
/// (`ip_map[transformed_ip] = original_ip`).
///
/// For a copy-through (non-inlined) instruction the map is its original index. For
/// an instruction spliced in from an inlined callee — including the arg-marshalling
/// `Move`s and dispatch scaffolding — the map is the original index of the
/// `CallKnown`/`CallClosure` it was inlined from: if a deopt lands inside the
/// inlined region, the interpreter resumes by re-executing that original call.
///
/// `loop_region` is the `[header, exit)` index range (in ORIGINAL `func.code`) of
/// the OSR loop the caller intends to compile. ONLY calls whose original index lies
/// inside that range are subject to the inline-or-bail rule: an in-region call must
/// be inlinable (it must dissolve to reach the native subset) or the whole pass
/// bails (`None`). A call OUTSIDE the region (a pre-/post-loop helper such as
/// `bench_size`) is copied through verbatim — it never runs natively (OSR entry is
/// the header, the only native exit is `OsrExit`), so its inlinability is irrelevant
/// and must not veto OSR for the hot loop. Passing `None` makes EVERY call in-scope
/// (the conservative whole-function behavior), preserved for callers that do not
/// pre-detect a region.
pub(in crate::reg_vm) fn native_inline_leaf_calls(
    unit: &RegUnit,
    func: &RegFunction,
    j3: bool,
    loop_region: Option<(usize, usize)>,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    native_inline_leaf_calls_inner(unit, func, j3, loop_region, &|_| false)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_inline_leaf_calls_preserving_known_calls(
    unit: &RegUnit,
    func: &RegFunction,
    j3: bool,
    loop_region: Option<(usize, usize)>,
    preserve_call_known: &std::collections::HashSet<usize>,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    native_inline_leaf_calls_inner(unit, func, j3, loop_region, &|i| {
        preserve_call_known.contains(&i)
    })
}

#[cfg(feature = "native-jit")]
fn native_inline_leaf_calls_inner(
    unit: &RegUnit,
    func: &RegFunction,
    j3: bool,
    loop_region: Option<(usize, usize)>,
    preserve_call_known: &dyn Fn(usize) -> bool,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    // A call at original index `i` is subject to inline-or-bail only if it lies in
    // the loop region (or no region was supplied ⇒ whole function in-scope).
    let in_region = |i: usize| match loop_region {
        Some((h, e)) => i >= h && i < e,
        None => true,
    };
    // Closure-allocation sinking: a `MakeClosure` whose dst is local to the compiled
    // region/function + non-escaping + called only via `CallClosure{closure:dst}` is
    // sunk — its alloc is dissolved and the statically-known callee body is inlined
    // at each call. With `loop_region=None`, run the same analysis over the whole
    // function so normal native translation can remove local closure churn too.
    let sinkable = match loop_region {
        Some((h, e)) => loop_local_sinkable_closures(unit, func, h, e, j3),
        None => loop_local_sinkable_closures(unit, func, 0, func.code.len(), j3),
    };
    let has_inlinable_call = func.code.iter().enumerate().any(|(i, instr)| match instr {
        RegInstr::CallKnown { .. } => in_region(i) && !preserve_call_known(i),
        RegInstr::SpawnTask { .. } => in_region(i),
        RegInstr::CallClosure { closure, .. } => {
            in_region(i)
                && (sinkable.sink_calls.contains_key(closure)
                    || monomorphic_closure_inline_target(unit, func, i).is_some()
                    || polymorphic_closure_inline_targets(unit, func, i).is_some())
        }
        _ => false,
    });
    if !has_inlinable_call {
        let ip_map: Vec<usize> = (0..func.code.len()).collect();
        return Some((func.code.clone(), func.regs, ip_map));
    }

    let direct_call_results: Vec<usize> = func
        .code
        .iter()
        .enumerate()
        .filter_map(|(i, instr)| match instr {
            RegInstr::CallKnown { dst, .. } if in_region(i) && !preserve_call_known(i) => {
                Some(*dst)
            }
            RegInstr::SpawnTask { dst, .. } if in_region(i) => Some(*dst),
            _ => None,
        })
        .collect();

    /// A jump target to be resolved once all positions are known.
    enum Fix {
        /// Target is a caller instruction index (use `index_map`).
        Caller(usize),
        /// Like `Caller` but for a branch-shaped MATCH op in the OUTER caller body
        /// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`): the caller's
        /// own indices shift when a leaf is spliced in, so both of its ip targets must
        /// be remapped through `index_map`. `second` selects the first/second target.
        CallerMatch { target: usize, second: bool },
        /// Target is a callee instruction index within splice `id` (use its `cmap`).
        Callee { id: usize, callee_target: usize },
        /// Like `Callee` but for a branch-shaped MATCH op spliced from a callee
        /// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`): these carry TWO
        /// callee ip targets, so `which` selects the first (`false`) or second
        /// (`true`) target field to patch.
        CalleeMatch {
            id: usize,
            callee_target: usize,
            second: bool,
        },
        /// A `Return` jump to the shared join slot `slot` (use `joins[slot]`). A
        /// `CallKnown`/monomorphic-`CallClosure` splice owns a private join slot; a
        /// polymorphic dispatch shares ONE join slot across all of its arms.
        Join(usize),
    }
    struct Splice {
        cmap: Vec<usize>,
    }

    let mut new_code: Vec<RegInstr> = Vec::new();
    // `ip_map[transformed_ip] = original_ip`, grown in lockstep with `new_code`.
    let mut ip_map: Vec<usize> = Vec::new();
    let mut index_map = vec![0usize; func.code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    let mut splices: Vec<Splice> = Vec::new();
    // Join positions, patched once known. One slot per `CallKnown`/mono splice; one
    // shared slot per polymorphic dispatch (all its arms target the same slot).
    let mut joins: Vec<usize> = Vec::new();
    let mut next_reg = func.regs;

    /// Splice one callee's reachable body into `new_code`, remapping its internal
    /// jumps and rewriting each `Return` into `Move dst <- result` + a `Jump` to the
    /// shared `join_slot`. Returns the splice id (registered in `splices` for
    /// `Fix::Callee` resolution). Args must already be moved into the `base` window.
    /// `None` if any callee instruction can't be offset into the native subset.
    #[allow(clippy::too_many_arguments)]
    fn splice_callee(
        unit: &RegUnit,
        callee: &RegFunction,
        dst: usize,
        base: usize,
        join_slot: usize,
        origin: usize,
        j3: bool,
        new_code: &mut Vec<RegInstr>,
        ip_map: &mut Vec<usize>,
        fixups: &mut Vec<(usize, Fix)>,
        splices: &mut Vec<Splice>,
        joins: &mut Vec<usize>,
        next_reg: &mut usize,
    ) -> Option<()> {
        let id = splices.len();
        let reachable = native_reachable_instructions(&callee.code);
        // Deopt-before-heap: a COLD arm that builds a heap value (`cold[ci]`) is
        // replaced by a single native `Bail` (a `RuntimeError` sentinel) at the arm's
        // start, with the rest of the arm emitting nothing. Because every op in the arm
        // is side-effect-free, native does nothing observable before bailing, so the
        // abandon-and-reinterpret-the-loop fallback re-runs the loop on the interpreter
        // (which rebuilds the heap value itself) — Execution Spec §7.2 holds unchanged.
        let (cold, arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
        let mut cmap = vec![0usize; callee.code.len()];
        let mut direct_spawn_results = vec![false; callee.regs];
        let mut direct_await_results = vec![false; callee.regs];
        for (ci, cinstr) in callee.code.iter().enumerate() {
            if !reachable[ci] {
                continue;
            }
            // Cold-arm interior (everything after the arm's start): emit nothing. The
            // interior is provably never a jump target from outside the arm (the
            // classifier enforces it), so no `cmap` entry is ever consulted; point it
            // at the arm's Bail for total safety.
            if cold[ci] && !arm_start[ci] {
                cmap[ci] = new_code.len();
                continue;
            }
            cmap[ci] = new_code.len();
            // Cold-arm start: a single `RuntimeError` ⇒ `JitInstr::Bail` at OSR-loop
            // translation. The arm's heap value is never built natively.
            if arm_start[ci] {
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                ip_map.push(origin);
                continue;
            }
            match cinstr {
                RegInstr::SpawnTask {
                    dst: spawn_dst,
                    function,
                    args,
                } => {
                    let spawned = unit.functions.get(*function)?;
                    if !native_callee_inlinable_j3_with_spawns(unit, spawned, args.len()) {
                        return None;
                    }
                    let spawn_base = *next_reg;
                    *next_reg += spawned.regs;
                    for (param, arg) in args.iter().enumerate() {
                        new_code.push(RegInstr::Move {
                            dst: spawn_base + param,
                            src: base + arg,
                        });
                        ip_map.push(origin);
                    }
                    let spawned_join_slot = joins.len();
                    joins.push(0);
                    splice_callee(
                        unit,
                        spawned,
                        base + spawn_dst,
                        spawn_base,
                        spawned_join_slot,
                        origin,
                        j3,
                        new_code,
                        ip_map,
                        fixups,
                        splices,
                        joins,
                        next_reg,
                    )?;
                    joins[spawned_join_slot] = new_code.len();
                    if *spawn_dst < direct_spawn_results.len() {
                        direct_spawn_results[*spawn_dst] = true;
                    }
                }
                RegInstr::AwaitJoin { dst, src }
                    if *src < direct_spawn_results.len() && direct_spawn_results[*src] =>
                {
                    new_code.push(RegInstr::Move {
                        dst: base + dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    if *dst < direct_await_results.len() {
                        direct_await_results[*dst] = true;
                    }
                }
                RegInstr::Move { dst, src }
                    if *src < direct_spawn_results.len() && direct_spawn_results[*src] =>
                {
                    new_code.push(RegInstr::Move {
                        dst: base + dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    if *dst < direct_spawn_results.len() {
                        direct_spawn_results[*dst] = true;
                    }
                }
                RegInstr::TryResult { dst, src, cleanup }
                    if cleanup.is_empty()
                        && *src < direct_await_results.len()
                        && direct_await_results[*src] =>
                {
                    new_code.push(RegInstr::TryResult {
                        dst: base + dst,
                        src: base + src,
                        cleanup: Vec::new(),
                    });
                    ip_map.push(origin);
                }
                RegInstr::Return { src } => {
                    new_code.push(RegInstr::Move {
                        dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    fixups.push((new_code.len(), Fix::Join(join_slot)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    ip_map.push(origin);
                }
                RegInstr::Jump { target } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::Jump { target: 0 });
                    ip_map.push(origin);
                }
                RegInstr::JumpIfBool {
                    cond,
                    expected,
                    target,
                } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::JumpIfBool {
                        cond: cond + base,
                        expected: *expected,
                        target: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::JumpIfIntCompare {
                        lhs: lhs + base,
                        rhs: rhs + base,
                        op: *op,
                        expected: *expected,
                        target: 0,
                    });
                    ip_map.push(origin);
                }
                // J3 branch-shaped match ops (OSR×inline only): two callee ip targets,
                // each remapped via the callee's `cmap`. The J3 region passes dissolve
                // the matched variant/Option/struct afterward.
                RegInstr::MatchOption {
                    src,
                    some_ip,
                    none_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *some_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *none_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchOption {
                        src: src + base,
                        some_ip: 0,
                        none_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchResult { src, ok_ip, err_ip } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *ok_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *err_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchResult {
                        src: src + base,
                        ok_ip: 0,
                        err_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchVariant {
                    src,
                    expected,
                    match_ip,
                    else_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *match_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *else_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchVariant {
                        src: src + base,
                        expected: expected.clone(),
                        match_ip: 0,
                        else_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchMapGet {
                    map,
                    key,
                    value_dst,
                    some_ip,
                    none_ip,
                }
                | RegInstr::MatchSortedMapGet {
                    map,
                    key,
                    value_dst,
                    some_ip,
                    none_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *some_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *none_ip,
                            second: true,
                        },
                    ));
                    let remapped = match cinstr {
                        RegInstr::MatchMapGet { .. } => RegInstr::MatchMapGet {
                            map: map + base,
                            key: key + base,
                            value_dst: value_dst + base,
                            some_ip: 0,
                            none_ip: 0,
                        },
                        RegInstr::MatchSortedMapGet { .. } => RegInstr::MatchSortedMapGet {
                            map: map + base,
                            key: key + base,
                            value_dst: value_dst + base,
                            some_ip: 0,
                            none_ip: 0,
                        },
                        _ => unreachable!(),
                    };
                    new_code.push(remapped);
                    ip_map.push(origin);
                }
                // A callee `RuntimeError` (e.g. the match-exhaustiveness fallback) is a
                // terminator with no registers: copy it through. In the native subset
                // it lowers to a bail, which re-runs the call from the interpreter —
                // sound because the inlined body is side-effect-free.
                RegInstr::RuntimeError { message } => {
                    new_code.push(RegInstr::RuntimeError {
                        message: message.clone(),
                    });
                    ip_map.push(origin);
                }
                pure => {
                    let offset = if j3 {
                        native_offset_regs_j3(pure, base)?
                    } else {
                        native_offset_regs(pure, base)?
                    };
                    new_code.push(offset);
                    ip_map.push(origin);
                }
            }
        }
        splices.push(Splice { cmap });
        Some(())
    }

    for (i, instr) in func.code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            // OSR × closure-allocation sinking: DELETE the `MakeClosure` and the dead
            // copy `Move`s whose value is being sunk — the heap alloc is dissolved and
            // every call to it is inlined below. The captured registers stay live (the
            // inlined body materializes each via a `Move` at the call site). Emit
            // nothing; `index_map[i]` points at the next instruction so any branch to
            // this ip lands correctly.
            _ if sinkable.dead_defs.contains(&i) => {}
            // OSR × closure-allocation sinking: a `CallClosure` whose closure operand
            // is a loop-local non-escaping closure value (a `MakeClosure` dst, possibly
            // forwarded through copy `Move`s). The callee `k` is known STATICALLY (no
            // profile, no identity guard — the closure value never exists at runtime),
            // so we inline its body directly: materialize each scalar capture from the
            // `MakeClosure`'s (still-live) capture registers, bind the call args, and
            // splice the body. This is the sibling of the J2 monomorphic path with the
            // guard removed and the captures sourced from the alloc site instead of a
            // heap closure handle.
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i) && sinkable.sink_calls.contains_key(closure) => {
                debug_assert!(mut_args.is_empty());
                let k = sinkable.sink_calls[closure];
                let callee = unit.functions.get(k)?;
                // Read the unique sunk `MakeClosure`'s capture registers for THIS
                // closure operand. Keying by operand avoids confusing two sinkable
                // closures that share the same callee but capture different values.
                let captures = sinkable.capture_regs.get(closure)?.clone();
                if captures.len() != callee.captures {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                // Capture layout matches the J2 inline path: capture regs `0..captures`
                // live BELOW the params. Materialize each capture by MOVING the alloc
                // site's (already-live) capture register into `base + k_cap` — no heap
                // closure ever exists, so there is no `NativeClosureCapture` read. A
                // non-scalar capture is caught by the downstream OSR type inference
                // (Int/Bool/Float only), so the body simply fails to compile.
                for (k_cap, &cap_reg) in captures.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + k_cap,
                        src: cap_reg,
                    });
                    ip_map.push(i);
                }
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + callee.captures + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    unit,
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                    j3,
                    &mut new_code,
                    &mut ip_map,
                    &mut fixups,
                    &mut splices,
                    &mut joins,
                    &mut next_reg,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::CallKnown {
                dst,
                function,
                args,
                mut_args,
            } if in_region(i) && !preserve_call_known(i) => {
                let callee0 = unit.functions.get(*function)?;
                // #7 foldable cold-arm sub-case: fold the callee's whole body first (a
                // measured-throwaway-string arm dissolves to scalar arithmetic), so a leaf
                // that was non-inlinable only because of that heap arm becomes inlinable.
                // The fold is semantics-preserving and a no-op for ordinary bodies. Use the
                // folded body CONSISTENTLY for both the inlinability verdict and the splice
                // below (candidacy applies the same fold) — an inconsistency only ever
                // declines OSR, never miscompiles.
                let folded_callee = if j3 {
                    native_string_folded_callee(callee0)
                } else {
                    None
                };
                let callee = folded_callee.as_ref().unwrap_or(callee0);
                // `mut` args need write-back at the callee join point, just like the
                // interpreter frame completion path. Heap copy-updated params are
                // now ordinary handle registers by that point, so a `Move` back to
                // the caller arg slot preserves the same semantics without a frame.
                // Under `j3` (OSR×inline) a callee that builds/destructures a
                // non-escaping Option/variant/struct also qualifies — it dissolves
                // post-inline.
                let inlinable = if j3 {
                    native_callee_inlinable_j3_with_spawns(unit, callee, args.len())
                } else {
                    native_callee_inlinable(callee, args.len())
                };
                if !inlinable {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    unit,
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                    j3,
                    &mut new_code,
                    &mut ip_map,
                    &mut fixups,
                    &mut splices,
                    &mut joins,
                    &mut next_reg,
                )?;
                joins[join_slot] = new_code.len();
                for &pos in mut_args {
                    new_code.push(RegInstr::Move {
                        dst: args[pos],
                        src: base + pos,
                    });
                    ip_map.push(i);
                }
            }
            RegInstr::SpawnTask {
                dst,
                function,
                args,
            } if in_region(i) => {
                let callee = unit.functions.get(*function)?;
                if !j3 || !native_callee_inlinable_j3_with_spawns(unit, callee, args.len()) {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    unit,
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                    j3,
                    &mut new_code,
                    &mut ip_map,
                    &mut fixups,
                    &mut splices,
                    &mut joins,
                    &mut next_reg,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::AwaitJoin { dst, src }
                if in_region(i) && direct_call_results.contains(src) =>
            {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: *src,
                });
                ip_map.push(i);
            }
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i) && monomorphic_closure_inline_target(unit, func, i).is_some() => {
                // J2.1 profile-guided monomorphic inlining: J1 profiled this site as
                // calling exactly one callee `k` (non-capturing, native-inlinable).
                // Guard the closure's identity, then inline `k`'s body. On a callee
                // mismatch the guard bails (re-run-from-top fallback), so the
                // interpreter handles the unexpected closure — sound because the
                // inlined subset is side-effect-free. `mut_args` is always empty for
                // an inlinable (side-effect-free) callee; the target check enforces it.
                debug_assert!(mut_args.is_empty());
                let k = monomorphic_closure_inline_target(unit, func, i)?;
                let callee = unit.functions.get(k)?;
                // Identity guard up front: bail before any inlined work if the
                // observed closure isn't the speculated callee `k`.
                new_code.push(RegInstr::NativeGuardClosureId {
                    closure: *closure,
                    expected: k,
                });
                ip_map.push(i);
                let base = next_reg;
                next_reg += callee.regs;
                // Capturing-closure inlining (OSR × J2): a closure callee lays out
                // its capture registers `0..captures` BELOW its params, so the
                // splice window is `[captures.. params.. locals]`. Materialize each
                // scalar capture into `base + k` via the host helper, then bind the
                // call args ABOVE the captures at `base + captures + param`. For a
                // non-capturing callee (`captures == 0`) this is exactly the shipped
                // path: no `NativeClosureCapture`, args at `base + param`.
                for k_cap in 0..callee.captures {
                    new_code.push(RegInstr::NativeClosureCapture {
                        dst: base + k_cap,
                        closure: *closure,
                        index: k_cap,
                    });
                    ip_map.push(i);
                }
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + callee.captures + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    unit,
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                    j3,
                    &mut new_code,
                    &mut ip_map,
                    &mut fixups,
                    &mut splices,
                    &mut joins,
                    &mut next_reg,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i) && polymorphic_closure_inline_targets(unit, func, i).is_some() => {
                // J2.2 polymorphic inline cache: J1 profiled this site as calling 2–3
                // distinct callees, EVERY one non-capturing and native-inlinable. Read
                // the closure's function id ONCE, then dispatch: `if id == Kj { inline
                // body of Kj; jump join }` for each speculated callee; if NONE match,
                // bail via the existing re-run-from-top fallback (a `RuntimeError`,
                // which lowers to `JitInstr::Bail`). Sound: every inlined body is
                // side-effect-free, so re-running from the top on a miss is safe —
                // identical discipline to J2.1's single-guard bail.
                debug_assert!(mut_args.is_empty());
                let targets = polymorphic_closure_inline_targets(unit, func, i)?;
                // Scratch registers for the dispatch: the id (read once), a per-arm
                // key constant, and the equality result. They live above every
                // inlined callee's window, so no arm can clobber them.
                let id_reg = next_reg;
                let key_reg = next_reg + 1;
                let eq_reg = next_reg + 2;
                next_reg += 3;
                // Read the closure's function id exactly once.
                new_code.push(RegInstr::NativeClosureId {
                    dst: id_reg,
                    closure: *closure,
                });
                ip_map.push(i);
                // One shared join past the last arm: every arm's `Return` jumps here.
                let join_slot = joins.len();
                joins.push(0);
                // Dispatch prologue: for each callee `Kj`, compare `id == Kj` and, on
                // match, jump to that arm (target patched once the arm is emitted).
                // Record one fixup slot per arm to backpatch its branch target.
                let mut arm_branch_pos: Vec<usize> = Vec::with_capacity(targets.len());
                for &k in &targets {
                    new_code.push(RegInstr::LoadInt {
                        dst: key_reg,
                        value: k as i64,
                    });
                    ip_map.push(i);
                    new_code.push(RegInstr::Equal {
                        dst: eq_reg,
                        lhs: id_reg,
                        rhs: key_reg,
                    });
                    ip_map.push(i);
                    arm_branch_pos.push(new_code.len());
                    new_code.push(RegInstr::JumpIfBool {
                        cond: eq_reg,
                        expected: true,
                        target: 0, // patched to the arm start below
                    });
                    ip_map.push(i);
                }
                // No-match: bail to the interpreter (re-run from the top). Sound
                // because every candidate body is side-effect-free.
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                ip_map.push(i);
                // Emit each arm's inlined body; record its start to patch the branch.
                for (arm, &k) in targets.iter().enumerate() {
                    let callee = unit.functions.get(k)?;
                    let arm_start = new_code.len();
                    // Backpatch this arm's dispatch branch to its body start.
                    if let RegInstr::JumpIfBool { target, .. } = &mut new_code[arm_branch_pos[arm]]
                    {
                        *target = arm_start;
                    }
                    let base = next_reg;
                    next_reg += callee.regs;
                    // Capturing-closure inline (OSR × J2.2): like the monomorphic
                    // path, a closure callee lays out its capture registers `0..
                    // captures` BELOW its params. Materialize each scalar capture
                    // from THIS arm's matched closure handle (`*closure`) into
                    // `base + k_cap`, then bind the call args ABOVE the captures at
                    // `base + captures + param`. For a non-capturing callee
                    // (`captures == 0`) this is exactly the shipped path (args at
                    // `base + param`).
                    for k_cap in 0..callee.captures {
                        new_code.push(RegInstr::NativeClosureCapture {
                            dst: base + k_cap,
                            closure: *closure,
                            index: k_cap,
                        });
                        ip_map.push(i);
                    }
                    for (param, arg) in args.iter().enumerate() {
                        new_code.push(RegInstr::Move {
                            dst: base + callee.captures + param,
                            src: *arg,
                        });
                        ip_map.push(i);
                    }
                    splice_callee(
                        unit,
                        callee,
                        *dst,
                        base,
                        join_slot,
                        i,
                        j3,
                        &mut new_code,
                        &mut ip_map,
                        &mut fixups,
                        &mut splices,
                        &mut joins,
                        &mut next_reg,
                    )?;
                }
                // Shared join lands just past the final arm.
                joins[join_slot] = new_code.len();
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Caller(*target)));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            // OUTER-caller branch-shaped match ops: when a leaf is inlined, the
            // caller's instruction indices shift, so the two ip targets of an outer
            // `MatchResult`/`MatchOption`/`MatchVariant`/`MatchMapGet` must be remapped
            // through `index_map` (the J3 region passes later dissolve these). Without
            // a region leaf inline these copy-through unchanged (identity index_map).
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *some_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *none_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *ok_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *err_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *match_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *else_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            other => {
                new_code.push(other.clone());
                ip_map.push(i);
            }
        }
    }

    for (pos, fix) in fixups {
        // For a `*Match`, remember which of the two match targets to patch.
        let second = matches!(
            fix,
            Fix::CalleeMatch { second: true, .. } | Fix::CallerMatch { second: true, .. }
        );
        let is_match = matches!(fix, Fix::CalleeMatch { .. } | Fix::CallerMatch { .. });
        let target = match fix {
            Fix::Caller(t) | Fix::CallerMatch { target: t, .. } => index_map[t],
            Fix::Callee { id, callee_target }
            | Fix::CalleeMatch {
                id, callee_target, ..
            } => splices[id].cmap[callee_target],
            Fix::Join(slot) => joins[slot],
        };
        match &mut new_code[pos] {
            // Single-target branches.
            RegInstr::Jump { target: t }
            | RegInstr::JumpIfBool { target: t, .. }
            | RegInstr::JumpIfIntCompare { target: t, .. }
                if !is_match =>
            {
                *t = target
            }
            // Two-target match ops: patch the first/second ip per `second`.
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                if second {
                    *none_ip = target;
                } else {
                    *some_ip = target;
                }
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                if second {
                    *err_ip = target;
                } else {
                    *ok_ip = target;
                }
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                if second {
                    *else_ip = target;
                } else {
                    *match_ip = target;
                }
            }
            _ => {}
        }
    }
    debug_assert_eq!(ip_map.len(), new_code.len());
    Some((new_code, next_reg, ip_map))
}

/// `RegFunction::native_status` value: the function is known not native-eligible.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_STATUS_NOT_ELIGIBLE: u8 = 1;

/// Consecutive runtime-bail count at which the native tier gives up on a
/// structurally-eligible function (predict-and-skip, like a JSC/V8 deopt count).
/// A function that passes the structural predictor but bails on *every* call
/// (arg-type mismatch or a runtime guard) otherwise re-compiles/marshals/bails
/// forever; after this many consecutive bails we mark it `NOT_ELIGIBLE` so the
/// cheap-negative early-return in `try_native` short-circuits all future calls.
/// Counter resets on any successful native completion, so a hot function that
/// bails only on a rare data edge keeps its fast path. Candidate for the
/// data-driven tuning of the native-tier give-up heuristic.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_BAIL_GIVEUP_THRESHOLD: u32 = 3;

/// Native-dispatch count at which the native tier gives up on a *back-edge-free*
/// (loop-free) function and demotes it to `NOT_ELIGIBLE` (a profitability
/// governor, mirroring the `NATIVE_BAIL_GIVEUP_THRESHOLD` predict-and-skip).
///
/// Invariant: a native body with no internal back-edge does bounded O(1) work
/// per dispatch, so calling it once per interpreter loop iteration (a tiny leaf
/// or closure body — `closure_alloc`, `option_result_chain`) pays FFI +
/// marshalling cost it can never amortize across the body's own iterations. A
/// *loop-bearing* body (the whole hot loop compiled into one native call —
/// `native_scalar_loop`, every `osr_*` kernel) does O(n) work per dispatch and
/// is dispatched `calls=1`, so it amortizes the FFI cost and is EXEMPT here
/// regardless of call count. The combination (back-edge-free AND dispatched many
/// times) is exactly the diagnosed per-iteration-dispatch loss pattern.
///
/// `K = 64` is large enough that a `calls=1` whole-loop win NEVER trips it and
/// small enough to cap the wasted FFI churn at a fixed bounded prefix before the
/// function falls back to the cheap interpreter path for the rest of the loop.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_NOAMORTIZE_GIVEUP: u32 = 64;

/// Backedge count at which a counting OSR candidate fires `try_osr`. Chosen so
/// tiny loops (which run < this many iterations) never pay OSR compile/setup, but
/// genuinely hot loops cross it quickly and then run native for the rest of their
/// (much longer) life. The eager path (`RSS_JIT_OSR`) uses threshold 0.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const OSR_BACKEDGE_THRESHOLD: u32 = 1000;

/// Effective OSR backedge threshold. Defaults to [`OSR_BACKEDGE_THRESHOLD`]
/// (1000); a bench/test-only `RSS_JIT_OSR_THRESHOLD` env var, when set to a
/// parseable `u32`, overrides it (mirrors `RSS_JIT_OSR`). Unset or unparseable
/// ⇒ exactly the default constant, so production behavior is unchanged. Read
/// per-fire (cheap relative to the OSR compile it gates); the override exists so
/// the auto-trigger threshold can be swept without recompiling per value.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn osr_backedge_threshold() -> u32 {
    match std::env::var("RSS_JIT_OSR_THRESHOLD") {
        Ok(s) => s.trim().parse::<u32>().unwrap_or(OSR_BACKEDGE_THRESHOLD),
        Err(_) => OSR_BACKEDGE_THRESHOLD,
    }
}
