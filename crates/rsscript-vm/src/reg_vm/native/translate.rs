//! Native-JIT IR producers and OSR-loop detection.

use super::super::*;
use super::passes::*;
use super::*;
use crate::text_util::strip_fresh_type;
use std::collections::{HashMap, HashSet};

mod jit_post;
mod loop_regions;
mod osr_loop;
use osr_loop::*;
mod type_infer;
use type_infer::*;

use jit_post::*;
pub(in crate::reg_vm) use loop_regions::*;

#[cfg(all(test, feature = "native-jit"))]
mod architecture_tests;

#[cfg(feature = "native-jit")]
fn native_call_mut_args_supported(mut_args: &[usize], param_tys: &[NativeTy]) -> bool {
    mut_args.iter().all(|&pos| {
        param_tys
            .get(pos)
            .is_some_and(|ty| matches!(ty, NativeTy::FlatIntMut | NativeTy::FlatFloatMut))
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_whole_function_region_exit(code: &[RegInstr]) -> usize {
    let reachable = native_reachable_instructions(code);
    let mut exit = code.len();
    while exit > 0 && !reachable[exit - 1] {
        exit -= 1;
    }
    exit
}

#[cfg(feature = "native-jit")]
fn native_profile_guidance_with_analysis(
    _profile: Option<&FunctionProfile>,
    _code: &[RegInstr],
    _ip_map: &[usize],
    _analysis: &NativeRegionAnalysis,
) -> NativeProfileGuidance {
    NativeProfileGuidance::default()
}

#[cfg(feature = "native-jit")]
fn native_osr_profile_guidance(
    profile: Option<&FunctionProfile>,
    code: &[RegInstr],
    n_regs: usize,
    lp: OsrLoop,
    ip_map: &[usize],
) -> NativeProfileGuidance {
    let analysis_exit = lp
        .exit
        .checked_add(1)
        .filter(|&exit| exit <= code.len())
        .unwrap_or(lp.exit);
    let Some(analysis) =
        NativeRegionAnalysis::compute_region(code, n_regs, lp.header, analysis_exit)
    else {
        return NativeProfileGuidance::default();
    };
    let mut guidance = native_profile_guidance_with_analysis(profile, code, ip_map, &analysis);
    guidance.hot_branch_edges.retain(|ip, hot_target| {
        let cold_ip = match code.get(*ip) {
            Some(RegInstr::JumpIfBool { target, .. })
            | Some(RegInstr::JumpIfIntCompare { target, .. }) => {
                if *hot_target {
                    *ip + 1
                } else {
                    *target
                }
            }
            _ => return false,
        };
        cold_ip != lp.exit
    });
    guidance
}

#[cfg(feature = "native-jit")]
fn native_declared_type_name_to_ty(type_name: &str) -> Option<NativeTy> {
    let root = type_root_name(strip_fresh_type(type_name));
    match root {
        "Int" => Some(NativeTy::Int),
        "Bool" => Some(NativeTy::Bool),
        "Float" => Some(NativeTy::Float),
        "Unit" => None,
        name if name.len() == 1 && name.chars().all(|ch| ch.is_ascii_uppercase()) => None,
        _ => Some(NativeTy::Handle),
    }
}

#[cfg(feature = "native-jit")]
fn native_set_compiled_call_arg_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    expected: NativeTy,
    n_params: usize,
    changed: &mut bool,
) -> bool {
    match (ty[reg], expected) {
        (
            Some(NativeTy::Handle),
            NativeTy::FlatInt | NativeTy::FlatIntMut | NativeTy::FlatFloat | NativeTy::FlatFloatMut,
        ) if reg < n_params => {
            ty[reg] = Some(expected);
            *changed = true;
            true
        }
        _ => native_set_ty(ty, reg, expected, changed),
    }
}

#[cfg(feature = "native-jit")]
fn native_set_list_read_base_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    changed: &mut bool,
) -> bool {
    match ty[reg] {
        Some(
            NativeTy::FlatInt | NativeTy::FlatIntMut | NativeTy::FlatFloat | NativeTy::FlatFloatMut,
        ) => true,
        _ => native_set_ty(ty, reg, NativeTy::Handle, changed),
    }
}

/// Translate a `RegFunction` into the native-JIT IR, or `None` if it is not in the
/// native subset (anything that isn't integer/boolean/control core, has captures,
/// or does not return an `Int`).
///
/// Callers invoke the compiled code only when **every argument is an `Int`**, so
/// all parameters are statically `i64`; type inference (a small fixpoint, to
/// handle loop back-edges) then proves every register is consistently `Int` or
/// `Bool`, every operand is well-typed, and the result is an `Int`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_to_native_jit(
    unit: &RegUnit,
    func: &RegFunction,
    facts: &VerifiedFunctionFacts,
    profile: Option<&FunctionProfile>,
    call_count: u32,
) -> Option<NativeTranslation> {
    translate_to_native_jit_with_compiled_callees(
        unit,
        func,
        facts,
        profile,
        call_count,
        &HashMap::new(),
    )
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_to_native_jit_with_compiled_callees(
    unit: &RegUnit,
    func: &RegFunction,
    facts: &VerifiedFunctionFacts,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    compiled_callees: &HashMap<usize, NativeCompiledCallee>,
) -> Option<NativeTranslation> {
    translate_to_native_jit_with_calls(
        unit,
        func,
        facts,
        NativeCallTranslationContext {
            profile,
            call_count,
            compiled_callees,
            self_call_sites: &HashSet::new(),
            group_call_sites: &HashMap::new(),
        },
    )
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct NativeCallTranslationContext<'a> {
    pub(in crate::reg_vm) profile: Option<&'a FunctionProfile>,
    pub(in crate::reg_vm) call_count: u32,
    pub(in crate::reg_vm) compiled_callees: &'a HashMap<usize, NativeCompiledCallee>,
    pub(in crate::reg_vm) self_call_sites: &'a HashSet<usize>,
    pub(in crate::reg_vm) group_call_sites: &'a HashMap<usize, u32>,
}

/// Like [`translate_to_native_jit_with_compiled_callees`], but `self_call_sites`
/// names the original ips of `CallKnown` instructions that call `func` itself —
/// emitted as `JitInstr::CallSelf` for native self-recursion (native-call-ABI
/// slice 3) — and `group_call_sites` maps the original ip of a `CallKnown` to a
/// *mutually-recursive group member* to that member's group index, emitted as
/// `JitInstr::CallGroup` (slice 4). Such functions use re-run-from-top deopt
/// (`precise_resume_safe` forced off), so a bail anywhere in the recursion unwinds
/// to the interpreter.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_to_native_jit_with_calls(
    unit: &RegUnit,
    func: &RegFunction,
    facts: &VerifiedFunctionFacts,
    context: NativeCallTranslationContext<'_>,
) -> Option<NativeTranslation> {
    use vm_jit::{JitCompare, JitInstr};

    let NativeCallTranslationContext {
        profile,
        call_count,
        compiled_callees,
        self_call_sites,
        group_call_sites,
    } = context;

    if func.captures != 0 {
        return None;
    }
    // Inline straight-line leaf calls first, so a function that only leaves the
    // native subset via small helper calls still qualifies (the calls vanish).
    // Whole-function native translation: the ENTIRE body runs natively, so every
    // call must be inlinable (`None` region ⇒ whole function in-scope).
    let preserve_call_known: HashSet<usize> = compiled_callees
        .keys()
        .copied()
        .chain(self_call_sites.iter().copied())
        .chain(group_call_sites.keys().copied())
        .collect();
    let (code, n_regs, ip_map) = native_inline_leaf_calls_preserving_known_calls(
        unit,
        func,
        profile,
        call_count,
        false,
        None,
        &preserve_call_known,
    )?;
    let mut pipeline = NativePipelineState::new(code, n_regs, ip_map)?;
    let region_exit = native_whole_function_region_exit(&pipeline.code);
    let (code, n_regs, next_ip_map) = native_elide_readonly_full_list_slices_in_region(
        &pipeline.code,
        pipeline.n_regs,
        0,
        region_exit,
    )?;
    pipeline.apply_rewrite(code, n_regs, next_ip_map)?;
    let region_exit = native_whole_function_region_exit(&pipeline.code);
    let (code, n_regs, next_ip_map) = native_lower_checked_payload_intrinsics_in_region(
        &pipeline.code,
        pipeline.n_regs,
        0,
        region_exit,
    )?;
    pipeline.apply_rewrite(code, n_regs, next_ip_map)?;
    // Option/Result/Variant/Struct now share one bounded virtual-object
    // orchestration boundary. The proven specialized rewrites remain the
    // mechanics, while work accounting, origin composition and elimination
    // telemetry are centralized so they can be retired independently.
    let virtualized = native_eliminate_virtual_objects_whole(&pipeline.code, pipeline.n_regs)?;
    let scalar_payload_regs = virtualized.zero_init_regs;
    debug_assert!(
        virtualized.summary.constructors_eliminated
            <= virtualized
                .summary
                .option_candidates
                .saturating_add(virtualized.summary.result_candidates)
                .saturating_add(virtualized.summary.variant_candidates)
                .saturating_add(virtualized.summary.struct_candidates)
    );
    pipeline.apply_rewrite(virtualized.code, virtualized.n_regs, virtualized.ip_map)?;
    // Dissolve non-escaping `Bytes.slice(...); Bytes.len(...)` values before native
    // type inference. Dynamic Bytes inputs retain a validating `Bytes.len` helper at
    // the slice site, while the allocation and output handle disappear. The regular
    // loop memoizer below can then cache that scalar helper when its operands are
    // invariant.
    let region_exit = native_whole_function_region_exit(&pipeline.code);
    let (code, n_regs, next_ip_map) =
        native_bytes_length_fold_in_region(&pipeline.code, pipeline.n_regs, 0, region_exit)?;
    pipeline.apply_rewrite(code, n_regs, next_ip_map)?;
    let (code, n_regs, origins) = pipeline.into_parts();
    let ip_map: Vec<usize> = origins.iter().map(|origin| origin.source_ip).collect();
    debug_assert!(
        origins
            .iter()
            .all(|origin| origin.resume_ip <= func.code.len())
    );
    if func.params > n_regs {
        return None;
    }
    // Reachability from `ip == 0` over the control-flow graph. The lowerer appends
    // a defensive `LoadUnit; Return(unit)` to every function body even when the
    // body always returns earlier; that tail is unreachable. Restricting analysis
    // (and codegen) to reachable instructions lets such functions still qualify —
    // dead instructions become `Nop`.
    let analysis = NativeRegionAnalysis::compute_prefix(&code, n_regs, 0, code.len())?;
    let reachable = analysis.reachable_mask();
    let profile_guidance =
        native_profile_guidance_with_analysis(profile, &code, &ip_map, &analysis);

    // Every *reachable* instruction must be in the native subset.
    for (i, instr) in code.iter().enumerate() {
        let compiled_call = matches!(instr, RegInstr::CallKnown { .. })
            && (compiled_callees.contains_key(&ip_map[i])
                || self_call_sites.contains(&ip_map[i])
                || group_call_sites.contains_key(&ip_map[i]));
        if reachable[i] && !compiled_call && !native_subset_instruction(instr) {
            return None;
        }
    }

    // Type inference by unification (fixpoint, to handle loop back-edges).
    // Parameters start untyped and acquire their type from the operands they are
    // combined with — so a float-parameter function is inferred correctly rather
    // than forced to `Int`.
    // Seed the storage lattice once from the bounded facts derived from the
    // verified executable. The local fixpoint remains only as a v1 compatibility
    // completion path for facts that are legitimately `Unknown` after type
    // erasure; it is no longer the source of ordinary scalar/call ABI facts.
    let mut ty = facts.native_type_seed(n_regs, false)?;
    let declared_signature = unit.native_signatures.get(&func.name);
    if let Some(signature) = declared_signature {
        for (reg, declared) in signature.params.iter().take(func.params).enumerate() {
            if let Some(native_ty) = native_declared_type_name_to_ty(declared) {
                ty[reg] = Some(native_ty);
            }
        }
    }
    let declared_return_ty = declared_signature
        .and_then(|signature| signature.return_type.as_deref())
        .and_then(native_declared_type_name_to_ty);
    // The whole-function tier has no per-call runtime param classification (unlike OSR's
    // `try_osr`), so it cannot identify a heap-typed param register here (there is no
    // `heap_param` predicate on this path; heap collection key/value PARAMS are not specially
    // typed). A heap key/value LOCAL still flows its `Handle` type from its in-region definition.
    native_infer_types(
        &code,
        func,
        facts,
        &ip_map,
        &reachable,
        declared_return_ty,
        profile,
        compiled_callees,
        self_call_sites,
        group_call_sites,
        &mut ty,
    )?;

    // TV2 flat-array classification. A `Handle` *parameter* whose uses are list
    // reads (`ListGet`/`ListLen`) can be reclassified as read-only `FlatInt`/
    // `FlatFloat`. If the same Int list param is also written only through
    // `ListSet<Int>`, classify it as `FlatIntMut` so native can bounds-check and
    // write the buffer directly under the heap transaction snapshot.
    let flat_param_kind: Vec<Option<NativeTy>> = {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Unseen,
            Flat(NativeTy),
            Disq,
        }
        let mut st = vec![S::Unseen; n_regs];
        let is_handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::GetFieldSlot { base, .. } if is_handle_param(*base) => {
                    st[*base] = S::Disq;
                }
                RegInstr::ListGet { dst, list, .. } if is_handle_param(*list) => {
                    let kind = if ty[*dst] == Some(NativeTy::Float) {
                        NativeTy::FlatFloat
                    } else {
                        NativeTy::FlatInt
                    };
                    st[*list] = match st[*list] {
                        S::Unseen => S::Flat(kind),
                        S::Flat(k) if k == kind => S::Flat(kind),
                        S::Flat(NativeTy::FlatIntMut) if kind == NativeTy::FlatInt => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloatMut) if kind == NativeTy::FlatFloat => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                RegInstr::ListSet { list, .. } if is_handle_param(*list) => {
                    st[*list] = match st[*list] {
                        S::Unseen | S::Flat(NativeTy::FlatInt) | S::Flat(NativeTy::FlatIntMut) => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloat) | S::Flat(NativeTy::FlatFloatMut) => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                RegInstr::ListPush { list, .. } if is_handle_param(*list) => {
                    st[*list] = S::Disq;
                }
                RegInstr::ListSort { list, .. } if is_handle_param(*list) => {
                    st[*list] = S::Disq;
                }
                // `List.is_empty<T>` reads only the length (the `lens` slot), never
                // elements, so it can ride the flat path. The typed call names the
                // element type, letting us pin the flat kind even when the param is
                // used ONLY via is_empty (a length read is element-type-agnostic, so
                // the kind only sets how the buffer pointer is marshalled — never
                // dereferenced here). Non-Int/Float element types stay neutral.
                RegInstr::CallTypedIntrinsic {
                    intrinsic: RegIntrinsic::ListIsEmpty,
                    args,
                    type_arg,
                    ..
                } if !args.is_empty()
                    && is_handle_param(args[0])
                    && matches!(type_arg.as_str(), "Int" | "Float") =>
                {
                    let kind = if type_arg.as_str() == "Float" {
                        NativeTy::FlatFloat
                    } else {
                        NativeTy::FlatInt
                    };
                    st[args[0]] = match st[args[0]] {
                        S::Unseen => S::Flat(kind),
                        S::Flat(k) if k == kind => S::Flat(kind),
                        S::Flat(NativeTy::FlatIntMut) if kind == NativeTy::FlatInt => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloatMut) if kind == NativeTy::FlatFloat => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                // `ListLen` is kind-neutral — neither pins nor disqualifies.
                _ => {}
            }
        }
        st.into_iter()
            .map(|s| match s {
                S::Flat(k) => Some(k),
                _ => None,
            })
            .collect()
    };
    for reg in parallel_indices(0..func.params) {
        if let Some(kind) = flat_param_kind[reg] {
            ty[reg] = Some(kind);
        }
    }

    // SOUNDNESS: the interpreter deep-copies every non-`mut` heap param at the prologue
    // (`DeepCopy`); native lowers `DeepCopy` to a Nop but now performs in-place heap
    // writes. Decline native if a DeepCopy'd heap param's value could be mutated in place,
    // stored, returned, or otherwise leaked (directly or via an alias) — otherwise it would
    // propagate to the caller while the interpreter only touches the copy. A `String`/`Bytes`
    // param is exempt (immutable, safe to share). Types are now final.
    let immutable_leaf_params: Vec<bool> = (0..func.params)
        .map(|p| {
            declared_signature
                .and_then(|s| s.params.get(p))
                .is_some_and(|t| native_declared_type_is_immutable_leaf(t))
        })
        .collect();
    if native_deepcopy_param_unsoundly_mutated(&code, &ty, n_regs, &immutable_leaf_params, |i| {
        reachable[i]
    }) {
        return None;
    }

    // scalar replacement: a scalar-replaced Option's payload register must be a SCALAR (Int/Float/
    // Bool, or fully unconstrained — defaults to Int). If inference proved it a
    // `Handle`/flat array, the Some payload was a heap value, so the Option was not
    // truly scalar-replaceable ⇒ bail and leave the function on the interpreter
    // path. (Conservative: any doubt ⇒ don't scalar-replace.)
    for &payload in &scalar_payload_regs {
        if matches!(
            ty[payload],
            Some(
                NativeTy::Handle
                    | NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut
            )
        ) {
            return None;
        }
    }

    // Output-handle provenance. A Handle produced by an output-allocating host
    // helper names the call context's heap-result table and may escape as the function's return or
    // feed another native host helper that consumes heap handles. Handles fetched by
    // `FieldHandle`/`ListGetHandle` still name scratch heap args and may only feed
    // native reads, not escape. Propagate through simple moves.
    let mut output_handle = vec![false; n_regs];
    let mut changed_output = true;
    while changed_output {
        changed_output = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::CallIntrinsic { intrinsic, dst, .. }
                    if native_host_intrinsic(*intrinsic)
                        .is_some_and(|spec| spec.produces_output_handle())
                        && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    dst,
                    ..
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
                    .is_some_and(|spec| spec.produces_output_handle())
                    && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::StringConcat { dst, .. } if !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::CallKnown { dst, .. }
                    if compiled_callees
                        .get(&ip_map[i])
                        .is_some_and(|callee| callee.ret_ty == NativeTy::Handle)
                        && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::LoadString { dst, .. } if !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                // NOTE: a `SetFieldSlot` base must NOT be seeded as an output handle.
                // A `FieldHandle`/`ListGetHandle`-derived base is call-scoped scratch
                // (a heap-arg-table index), not a heap-result-table value; marking it
                // here let it escape via `Return` and the host materialized garbage.
                // Genuine output structs are already marked by their allocating helper,
                // and param bases are returnable via `handle_param`.
                RegInstr::Move { dst, src } if output_handle[*src] && !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                _ => {}
            }
        }
    }
    let mut escaping_output_handle = vec![false; n_regs];
    let mut changed_escape = true;
    while changed_escape {
        changed_escape = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::Return { src }
                    if output_handle[*src] && !escaping_output_handle[*src] =>
                {
                    escaping_output_handle[*src] = true;
                    changed_escape = true;
                }
                RegInstr::CallIntrinsic {
                    intrinsic, args, ..
                } if native_host_intrinsic(*intrinsic)
                    .is_some_and(|spec| spec.consumes_output_handles()) =>
                {
                    let spec = native_host_intrinsic(*intrinsic)?;
                    for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                        if expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    args,
                    ..
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
                    .is_some_and(|spec| spec.consumes_output_handles()) =>
                {
                    let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                    for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                        if expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::StringConcat { left, right, .. } => {
                    for arg in [*left, *right] {
                        if output_handle[arg] && !escaping_output_handle[arg] {
                            escaping_output_handle[arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::CallKnown { args, .. } if compiled_callees.contains_key(&ip_map[i]) => {
                    let callee = compiled_callees.get(&ip_map[i])?;
                    for (arg, expected) in args.iter().zip(callee.param_tys.iter()) {
                        if *expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::ListLen { list, .. }
                | RegInstr::ListGet { list, .. }
                | RegInstr::ListPush { list, .. }
                | RegInstr::ListSort { list, .. } => {
                    if output_handle[*list] && !escaping_output_handle[*list] {
                        escaping_output_handle[*list] = true;
                        changed_escape = true;
                    }
                }
                RegInstr::GetFieldSlot { base, .. } => {
                    if output_handle[*base] && !escaping_output_handle[*base] {
                        escaping_output_handle[*base] = true;
                        changed_escape = true;
                    }
                }
                RegInstr::Move { dst, src }
                    if escaping_output_handle[*dst] && !escaping_output_handle[*src] =>
                {
                    escaping_output_handle[*src] = true;
                    changed_escape = true;
                }
                _ => {}
            }
        }
    }

    let int = |reg: usize| ty[reg] == Some(NativeTy::Int);
    let float = |reg: usize| ty[reg] == Some(NativeTy::Float);
    // A heap-read result register that is either provably `Int` or fully
    // unconstrained (no use pins its type) — both lower via the Int read helper
    // (an unconstrained register defaults to `Int` everywhere else too). This
    // keeps the "ambiguous must not silently pick Float" rule: Float is emitted
    // only when `float(dst)` is provably true.
    let int_or_free = |reg: usize| matches!(ty[reg], None | Some(NativeTy::Int));
    let bool_ty = |reg: usize| ty[reg] == Some(NativeTy::Bool);
    // Numeric = Int or Float; `same` = both operands typed and identical (so a
    // polymorphic op lowers consistently and native equality matches `VmValue`).
    let numeric = |reg: usize| matches!(ty[reg], Some(NativeTy::Int | NativeTy::Float));
    let same = |a: usize, b: usize| ty[a].is_some() && ty[a] == ty[b];
    let int_pair_or_same_numeric =
        |a: usize, b: usize| (numeric(a) && same(a, b)) || (int_or_free(a) && int_or_free(b));
    let int_triple_or_same_numeric = |a: usize, b: usize, c: usize| {
        (numeric(a) && same(a, b) && same(a, c))
            || (int_or_free(a) && int_or_free(b) && int_or_free(c))
    };
    // A parameter handle enters via the caller's heap args (`try_native`) and may be
    // returned unchanged by the pass-through slice.
    let handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
    // A *native-readable* handle (Pending #1): any Handle register — a param/live-in
    // (marshalled into the heap-arg window) or a loop-internal handle produced by a
    // `FieldHandle`/`ListGetHandle` read (a stored struct/closure fetched as a fresh
    // table index). Used by the heap reads and closure ops, whose runtime helper +
    // identity guard/bail make a wrong handle sound (re-run from the top).
    let handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
    // A TV2 flat-array param (pointer + length, read directly in-register).
    let flat_param = |reg: usize| {
        matches!(
            ty[reg],
            Some(
                NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut
            )
        ) && reg < func.params
    };
    let r = |reg: usize| reg as u32;
    let cmp = |op: &RegIntCompare| match op {
        RegIntCompare::Less => JitCompare::Lt,
        RegIntCompare::LessEqual => JitCompare::Le,
        RegIntCompare::Greater => JitCompare::Gt,
        RegIntCompare::GreaterEqual => JitCompare::Ge,
    };

    let split_len_source = native_split_len_sources(&code, &reachable, n_regs);
    let pad_left_len_source = native_pad_left_len_sources(&code, &reachable, n_regs);
    let mut string_literals: Vec<Rc<String>> = Vec::new();
    let mut string_literal_ids: HashMap<Rc<String>, i64> = HashMap::new();
    let intern_string_literal = |literals: &mut Vec<Rc<String>>,
                                 ids: &mut HashMap<Rc<String>, i64>,
                                 value: &Rc<String>|
     -> Option<i64> {
        if let Some(index) = ids.get(value) {
            return Some(*index);
        }
        let index = i64::try_from(literals.len()).ok()?;
        literals.push(Rc::clone(value));
        ids.insert(Rc::clone(value), index);
        Some(index)
    };

    let mut jit_code = Vec::with_capacity(code.len());
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            // Dead code (e.g. the lowerer's defensive trailing `Unit` return):
            // keep an index-aligned `Nop`, never executed.
            jit_code.push(JitInstr::Nop);
            continue;
        }
        let jit = match instr {
            RegInstr::LoadInt { dst, value } => JitInstr::LoadInt {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::LoadFloat { dst, value } => JitInstr::LoadFloat {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::LoadBool { dst, value } => JitInstr::LoadBool {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::LoadString { dst, value } => {
                require(escaping_output_handle[*dst])?;
                let literal_id =
                    intern_string_literal(&mut string_literals, &mut string_literal_ids, value)?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::ImmI64(literal_id)],
                }
            }
            RegInstr::Move { dst, src } => {
                ty[*src]?; // src must be typed
                if (split_len_source[*dst].is_some()
                    && split_len_source[*dst] == split_len_source[*src])
                    || (pad_left_len_source[*dst].is_some()
                        && pad_left_len_source[*dst] == pad_left_len_source[*src])
                {
                    JitInstr::Nop
                } else {
                    JitInstr::Move {
                        dst: r(*dst),
                        src: r(*src),
                    }
                }
            }
            RegInstr::CallKnown {
                dst,
                args,
                mut_args,
                ..
            } if compiled_callees.contains_key(&ip_map[i]) => {
                let callee = compiled_callees.get(&ip_map[i])?;
                require(
                    args.len() == callee.param_tys.len()
                        && native_call_mut_args_supported(mut_args, &callee.param_tys),
                )?;
                for (arg, expected) in args.iter().zip(callee.param_tys.iter()) {
                    require(ty[*arg] == Some(*expected))?;
                }
                require(ty[*dst] == Some(callee.ret_ty))?;
                JitInstr::CallNative {
                    callee: callee.id,
                    dst: r(*dst),
                    args: args.iter().map(|arg| r(*arg)).collect(),
                }
            }
            RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. } => {
                // Always a Nop in a native-eligible function: these are pure, leaf,
                // side-effect-free, and never mutate a container, so an independent
                // copy is never observably distinct from the original — for a scalar
                // register or a heap handle/flat param alike. (The previous `ty[reg]?`
                // also *rejected the whole function* when `reg` was untyped — e.g. an
                // unused parameter pins no type — needlessly disqualifying otherwise
                // eligible functions; an untyped register defaults to a scalar `Int`
                // everywhere else, so a copy of it is likewise a no-op.)
                JitInstr::Nop
            }
            RegInstr::TailCallGuard => JitInstr::TailCallGuard {
                max_depth: u32::try_from(DEFAULT_MAX_DEPTH).ok()?,
            },
            RegInstr::AddInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Add {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Sub {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Mul {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Div {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::Mod {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitAnd {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitOr {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitXor {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::Shl {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::Shr {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::LessInt { dst, lhs, rhs }
            | RegInstr::LessEqualInt { dst, lhs, rhs }
            | RegInstr::GreaterInt { dst, lhs, rhs }
            | RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                require(int_pair_or_same_numeric(*lhs, *rhs))?;
                let op = match instr {
                    RegInstr::LessInt { .. } => JitCompare::Lt,
                    RegInstr::LessEqualInt { .. } => JitCompare::Le,
                    RegInstr::GreaterInt { .. } => JitCompare::Gt,
                    _ => JitCompare::Ge,
                };
                JitInstr::Compare {
                    dst: r(*dst),
                    op,
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::Equal { dst, lhs, rhs } => {
                // Same statically-known type so native equality matches the
                // interpreter's `VmValue` equality (Int/Bool via icmp, Float via fcmp).
                require(same(*lhs, *rhs))?;
                JitInstr::Equal {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::NotEqual { dst, lhs, rhs } => {
                require(same(*lhs, *rhs))?;
                JitInstr::NotEqual {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::Jump { target } => JitInstr::Jump { target: r(*target) },
            RegInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                require(bool_ty(*cond))?;
                JitInstr::JumpIfBool {
                    cond: r(*cond),
                    expected: *expected,
                    target: r(*target),
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                require(int_pair_or_same_numeric(*lhs, *rhs))?;
                JitInstr::JumpIfIntCompare {
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                    op: cmp(op),
                    expected: *expected,
                    target: r(*target),
                }
            }
            RegInstr::Return { src } => {
                // The native ABI returns 64 bits boxed by the caller as the
                // function's return type. A scalar (`Int`/`Float`) return is the
                // unchanged path. Heap-result return ABI also accepts returning a
                // heap parameter unchanged, or a handle freshly allocated into
                // the call context's heap-result table by an output-allocating host helper. Handles
                // produced by `FieldHandle`/`ListGetHandle` remain scratch-only and
                // cannot escape.
                require(
                    int_or_free(*src)
                        || float(*src)
                        || bool_ty(*src)
                        || handle_param(*src)
                        || escaping_output_handle[*src],
                )?;
                JitInstr::Return { src: r(*src) }
            }
            RegInstr::RuntimeError { .. } => JitInstr::Bail,
            RegInstr::StringConcat { dst, left, right } => {
                require(handle_reg(*left) && handle_reg(*right) && escaping_output_handle[*dst])?;
                JitInstr::HostCall {
                    helper: native_string_concat_host().helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*left)),
                        vm_jit::HostArg::Reg(r(*right)),
                    ],
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } => {
                require(handle_reg(*base))?;
                if handle_reg(*dst) {
                    // A heap-valued field (a stored closure) fetched as a fresh
                    // handle so a downstream closure read can address it.
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else if float(*dst) {
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else {
                    // Int or unconstrained → Int helper (a non-Int field then
                    // bails at the helper). A Bool dst is rejected here.
                    require(int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                }
            }
            RegInstr::SetFieldSlot {
                dst: _,
                base,
                slot,
                value,
            } => {
                // Pick the store helper by the value's type, mirroring the read side:
                // Float → `FieldSetFloat`; Int/unconstrained → `FieldSetInt`; (transactional heap mutation) a
                // heap value → `FieldSetHandle` (sets the field to a resolved heap value).
                // A non-matching field then bails at the helper.
                require(handle_reg(*base))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::FieldSetFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::FieldSetInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::FieldSetHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*base),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(int(*dst))?;
                if let Some((value, delimiter)) = split_len_source[*list] {
                    require(handle_reg(value) && handle_reg(delimiter))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::StringSplitCount,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(value)),
                            vm_jit::HostArg::Reg(r(delimiter)),
                        ],
                    }
                } else if flat_param(*list) {
                    JitInstr::ListLenDirect {
                        dst: r(*dst),
                        base: r(*list),
                    }
                } else {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListLen,
                        dst: r(*dst),
                        args: vec![vm_jit::HostArg::Reg(r(*list))],
                    }
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(int(*index))?;
                // TV2 flat params lower to direct in-register reads; the flat kind
                // (set by classification) matches the dst kind by construction.
                if matches!(
                    ty[*list],
                    Some(NativeTy::FlatFloat | NativeTy::FlatFloatMut)
                ) {
                    require(float(*dst))?;
                    JitInstr::ListGetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if matches!(ty[*list], Some(NativeTy::FlatInt | NativeTy::FlatIntMut)) {
                    require(int_or_free(*dst))?;
                    JitInstr::ListGetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if handle_reg(*dst) {
                    // A heap-valued element (a struct holding a stored closure)
                    // fetched as a fresh handle for a downstream field/closure read.
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else if float(*dst) {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else {
                    require(handle_reg(*list) && int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                }
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => {
                require(int(*index) && int(*dst) && (int_or_free(*value) || float(*value)))?;
                if ty[*list] == Some(NativeTy::FlatIntMut) {
                    require(int_or_free(*value))?;
                    JitInstr::ListSetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else if ty[*list] == Some(NativeTy::FlatFloatMut) {
                    require(float(*value))?;
                    JitInstr::ListSetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else {
                    require(handle_reg(*list))?;
                    let helper = if float(*value) {
                        vm_jit::HostHelper::ListSetFloat
                    } else {
                        vm_jit::HostHelper::ListSetInt
                    };
                    JitInstr::HostCall {
                        helper,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                            vm_jit::HostArg::Reg(r(*value)),
                        ],
                    }
                }
            }
            RegInstr::ListPush { dst, list, value } => {
                // Float → ListPushFloat; Int/unconstrained → ListPushInt; (transactional heap mutation) heap
                // value (e.g. `String`/nested collection) → ListPushHandle, resolving the
                // value handle and appending it. A wrong-element-type list bails.
                require(handle_reg(*list) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::ListPushFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::ListPushInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::ListPushHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*list)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::ListSort { dst, list } => {
                require(handle_reg(*list) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ListSortInt,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*list))],
                }
            }
            RegInstr::MapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*dst))?;
                // Three shapes: Int-key/Float-value, Int-key/Int-value, and (transactional heap mutation)
                // heap-key (e.g. `String`)/Int-value. A heap key is resolved + hashed by
                // the host's own `VmMapKey` in the helper, never re-hashed in native.
                let helper = if int(*key) && float(*value) {
                    vm_jit::HostHelper::MapInsertFloat
                } else if int(*key) && int_or_free(*value) {
                    vm_jit::HostHelper::MapInsertInt
                } else if handle_reg(*key) && int(*value) {
                    vm_jit::HostHelper::MapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SetInsertInt; (transactional heap mutation) heap value (e.g. `String`) →
                // SetInsertHandle, which resolves + hashes the value via the host's key.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedSetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SortedSetInsertInt; (transactional heap mutation) heap value → SortedSetInsertHandle.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SortedSetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SortedSetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedMapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*value) && int(*dst))?;
                // Int key → SortedMapInsertInt; (transactional heap mutation) heap key → SortedMapInsertHandleKeyInt.
                let helper = if int(*key) {
                    vm_jit::HostHelper::SortedMapInsertInt
                } else if handle_reg(*key) {
                    vm_jit::HostHelper::SortedMapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushBack { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushBackFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushBackInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushBackHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushFront { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushFrontFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushFrontInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushFrontHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePopFront { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopFrontFloat
                } else {
                    vm_jit::HostHelper::DequePopFrontInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::DequePopBack { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopBackFloat
                } else {
                    vm_jit::HostHelper::DequePopBackInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::MatchSortedMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchSortedMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchSortedMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::NativeClosureId { dst, closure } => {
                // The closure handle is a native-readable handle; reads its
                // function id once into `dst` for the polymorphic dispatcher.
                require(handle_reg(*closure))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureId,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*closure))],
                }
            }
            RegInstr::NativeFieldClosureId { dst, base, slot } => {
                require(handle_reg(*base) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureId,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                    ],
                }
            }
            RegInstr::NativeClosureCapture {
                dst,
                closure,
                index,
            } => {
                // The closure handle is a native-readable handle; materialize
                // capture `index`'s scalar bits into `dst` (the inlined body's
                // capture register). `dst` may be Int/Bool (i64 used directly) or
                // Float (the i64 slot is `f64::to_bits`, bit-reinterpreted to f64 in
                // codegen); an unconstrained `dst` defaults to Int. A non-scalar
                // (Handle/flat) `dst` bails. A non-scalar capture VALUE additionally
                // bails out-of-band in the host helper at runtime.
                require(
                    handle_reg(*closure) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)),
                )?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*closure)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            RegInstr::NativeFieldClosureCapture {
                dst,
                base,
                slot,
                index,
            } => {
                require(handle_reg(*base) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            // `Int.to_float`: signed-int→f64 conversion (`fcvt_from_sint`). The
            // src register holds an Int (i64) and dst a Float (f64). The
            // interpreter's `IntToFloat` does `i as f64`; this is the identical
            // value-preserving conversion (not a bitcast).
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::IntToFloat,
                args,
                dst,
            } => {
                require(int(args[0]) && float(*dst))?;
                JitInstr::IntToFloat {
                    dst: r(*dst),
                    src: r(args[0]),
                }
            }
            // `Math.floor`/`Math.ceil`: round the f64 arg then a saturating f64→i64
            // cast, identical to the interpreter's `f.floor()/.ceil() as i64`. src is
            // a Float (f64), dst an Int (i64).
            RegInstr::CallIntrinsic {
                intrinsic: rounding_intrinsic @ (RegIntrinsic::MathFloor | RegIntrinsic::MathCeil),
                args,
                dst,
            } => {
                require(float(args[0]) && int(*dst))?;
                let rounding = match rounding_intrinsic {
                    RegIntrinsic::MathFloor => vm_jit::FloatRounding::Floor,
                    _ => vm_jit::FloatRounding::Ceil,
                };
                JitInstr::FloatToInt {
                    dst: r(*dst),
                    src: r(args[0]),
                    rounding,
                }
            }
            // `List.is_empty` on a flat-array param reads the length directly from the
            // param's `lens` slot and compares to zero — no per-iteration host call.
            // (Non-flat/`Handle` receivers fall through to the host-helper path below.)
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
                ..
            } if args.len() == 1 && flat_param(args[0]) => {
                require(bool_ty(*dst))?;
                JitInstr::ListIsEmptyDirect {
                    dst: r(*dst),
                    base: r(args[0]),
                }
            }
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringSplit,
                dst,
                ..
            } if split_len_source[*dst].is_some() => JitInstr::Nop,
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                dst,
                ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                dst,
                ..
            } if pad_left_len_source[*dst].is_some() => JitInstr::Nop,
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                dst,
                ..
            } if args.len() == 1 && pad_left_len_source[args[0]].is_some() => {
                require(int(*dst))?;
                let (value, width, fill) = pad_left_len_source[args[0]]?;
                require(handle_reg(value) && int(width) && handle_reg(fill))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeftLen,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(value)),
                        vm_jit::HostArg::Reg(r(width)),
                        vm_jit::HostArg::Reg(r(fill)),
                    ],
                }
            }
            RegInstr::CallIntrinsic {
                intrinsic,
                args,
                dst,
            } if native_host_intrinsic(*intrinsic).is_some() => {
                let spec = native_host_intrinsic(*intrinsic)?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                if spec.produces_output_handle() {
                    require(escaping_output_handle[*dst])?;
                }
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
                }
            }
            RegInstr::CallTypedIntrinsic {
                intrinsic,
                type_arg,
                args,
                dst,
            } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_some() => {
                let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                if spec.produces_output_handle() {
                    require(escaping_output_handle[*dst])?;
                }
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
                }
            }
            // `native_subset_instruction` already rejected everything else.
            _ => return None,
        };
        jit_code.push(jit);
    }

    // Return type = the type of any reachable `Return`'s source (all consistent,
    // validated numeric above); defaults to `Int` for an empty body.
    let ret_type = code
        .iter()
        .enumerate()
        .find_map(|(i, instr)| match instr {
            RegInstr::Return { src } if reachable[i] => ty[*src],
            _ => None,
        })
        .unwrap_or(NativeTy::Int);

    let native_reg_types: Vec<NativeTy> = (0..n_regs)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int))
        .collect();
    let memo_scopes = native_memoize_loop_invariant_runtime_helper_calls(
        &code,
        &reachable,
        &mut jit_code,
        &native_reg_types,
        func.params,
        None,
    );
    native_forward_direct_list_store_loads(&mut jit_code);

    let reg_types = native_reg_types
        .iter()
        .map(|ty| ty.jit_value_type())
        .collect();

    // Parameter types (for the caller's argument unboxing); an unconstrained
    // parameter defaults to `Int` (and a mismatching argument then just falls back).
    let param_types: Vec<NativeTy> = native_reg_types[..func.params].to_vec();

    let instruction_origins = origins
        .iter()
        .copied()
        .map(NativeInstructionOrigin::to_jit)
        .collect::<Option<Vec<_>>>()?;

    let jit_fn = vm_jit::JitFunction {
        n_params: func.params as u32,
        n_regs: native_reg_types.len() as u32,
        reg_types,
        zero_init_regs: scalar_payload_regs.iter().map(|&reg| reg as u32).collect(),
        code: jit_code,
        instruction_origins,
        source_instruction_count: u32::try_from(func.code.len()).ok()?,
        memo_scopes,
        cold_blocks: profile_guidance.cold_blocks,
        resume_live_regs: Vec::new(),
    };
    // A self-recursive function uses re-run-from-top deopt (its `CallSelf` is
    // non-chaining and its native frame chain has no bounded deopt payload), so it is
    // never precise-resumable regardless of ip-map identity.
    let precise_resume_safe = self_call_sites.is_empty()
        && group_call_sites.is_empty()
        && n_regs == func.regs
        && origins
            .iter()
            .all(|origin| origin.source_ip < func.code.len() && origin.resume_ip < func.code.len());
    Some(NativeTranslation {
        jit_fn,
        return_ty: ret_type,
        param_tys: param_types,
        string_literals,
        precise_resume_safe,
    })
}

/// Convert a failed eligibility predicate into the translator's `None` decline.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn require(condition: bool) -> Option<()> {
    condition.then_some(())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct OsrTranslationRequest<'a> {
    pub(in crate::reg_vm) function: &'a RegFunction,
    pub(in crate::reg_vm) facts: &'a VerifiedFunctionFacts,
    pub(in crate::reg_vm) profile: Option<&'a FunctionProfile>,
    pub(in crate::reg_vm) code: &'a [RegInstr],
    pub(in crate::reg_vm) register_count: usize,
    pub(in crate::reg_vm) parameter_count: usize,
    pub(in crate::reg_vm) capture_count: usize,
    pub(in crate::reg_vm) region: OsrLoop,
    pub(in crate::reg_vm) ip_map: &'a [usize],
    pub(in crate::reg_vm) parameter_types: &'a [Option<NativeTy>],
    pub(in crate::reg_vm) immutable_leaf_params: &'a [bool],
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_osr_loop_profiled(
    request: OsrTranslationRequest<'_>,
) -> Option<OsrTranslation> {
    let OsrTranslationRequest {
        function: func,
        facts,
        profile,
        code,
        register_count: n_regs,
        parameter_count: n_params,
        capture_count: captures,
        region: lp,
        ip_map,
        parameter_types: param_native_types,
        immutable_leaf_params,
    } = request;
    // The direct verified-bytecode OSR path consumes the same bounded typed
    // block IR as continuations. Transformed/inlined streams retain their
    // existing proven facts until their origin map can project typed values
    // without guessing.
    let typed_ir;
    let typed_code;
    let code = if std::ptr::eq(code, func.code.as_slice()) {
        let mut included = vec![false; code.len()];
        included.get_mut(lp.header..lp.exit)?.fill(true);
        typed_ir = Some(TypedRegionIr::derive(func, facts, &included)?);
        typed_code = typed_ir.as_ref()?.lower_to_reg_code(func)?;
        typed_code.as_slice()
    } else {
        typed_ir = None;
        code
    };
    let profile_guidance = native_osr_profile_guidance(profile, code, n_regs, lp, ip_map);
    translate_osr_loop_inner(OsrLoweringRequest {
        code,
        register_count: n_regs,
        parameter_count: n_params,
        capture_count: captures,
        region: lp,
        cold_blocks: profile_guidance.cold_blocks,
        profile_hot_branch_edges: profile_guidance.hot_branch_edges,
        parameter_types: param_native_types,
        immutable_leaf_params,
        verified_facts: Some(facts),
        typed_ir: typed_ir.as_ref(),
        source_ip_map: Some(ip_map),
        source_instruction_count: func.code.len(),
        enable_flat_buffers: true,
    })
}

/// Union-find `find` with path-halving, used by the OSR translator's Handle-`Move`
/// alias-class computations. A proper union-find is REQUIRED there: the older
/// `alias[dst] = alias[src]` propagation-to-fixpoint oscillates forever on a cyclic
/// Handle-`Move` graph (which a two-armed `Result<Handle,_>` dissolution produces),
/// hanging the translator.
#[cfg(feature = "native-jit")]
fn osr_uf_find(a: &mut [usize], mut x: usize) -> usize {
    while a[x] != x {
        a[x] = a[a[x]]; // path-halving
        x = a[x];
    }
    x
}

#[cfg(feature = "native-jit")]
struct OsrLoweringRequest<'a> {
    code: &'a [RegInstr],
    register_count: usize,
    parameter_count: usize,
    capture_count: usize,
    region: OsrLoop,
    cold_blocks: Vec<u32>,
    profile_hot_branch_edges: HashMap<usize, bool>,
    parameter_types: &'a [Option<NativeTy>],
    immutable_leaf_params: &'a [bool],
    verified_facts: Option<&'a VerifiedFunctionFacts>,
    typed_ir: Option<&'a TypedRegionIr>,
    source_ip_map: Option<&'a [usize]>,
    source_instruction_count: usize,
    enable_flat_buffers: bool,
}
