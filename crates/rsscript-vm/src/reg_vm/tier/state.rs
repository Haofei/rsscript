//! Evaluation-local state for experimental JIT planning.
//!
//! A verified bytecode program is immutable data. JIT planning data therefore
//! never lives on `RegFunction` (which is part of that decoded program): it is
//! owned by one evaluation and indexed directly by stable function ordinal.
//! Every caller carries the verified function ordinal explicitly; neither
//! pointer identity nor an unused program-digest wrapper participates in state
//! lookup.

use super::*;

pub(crate) trait FunctionOrdinal {
    fn function_ordinal(self) -> usize;
}

impl FunctionOrdinal for usize {
    #[inline(always)]
    fn function_ordinal(self) -> usize {
        self
    }
}

impl FunctionOrdinal for &RegFunction {
    #[inline(always)]
    fn function_ordinal(self) -> usize {
        self.ordinal
    }
}

impl FunctionOrdinal for &Rc<RegFunction> {
    #[inline(always)]
    fn function_ordinal(self) -> usize {
        self.ordinal
    }
}

#[derive(Debug, Default)]
struct JitFunctionState {
    tier0_analysis: Option<(bool, bool)>,
    self_recursion_kind: Option<SelfRecursionKind>,
    /// All mutable native-tier feedback belongs to the evaluation, never to the
    /// decoded verified function object. These fields do not exist in the
    /// default VM build.
    #[cfg(feature = "native-jit")]
    native_status: u8,
    /// Callees this function's tier-0 executor would run *inside* its own frame
    /// (`RegVm::run_jit_pure_leaf`), so they never reach `RegVm::drive` and are
    /// never offered to `RegVm::attempt_native`. Bounded and deduplicated at
    /// program load; empty for the overwhelmingly common call-free body.
    #[cfg(feature = "native-jit")]
    tier0_inlined_callees: Vec<usize>,
}

/// Side table for one evaluation of a verified program.
///
/// This intentionally owns only experiment data. It contains no VM value,
/// source, provider, or artifact payload state; dropping the run drops all JIT
/// feedback with it.
#[derive(Debug)]
pub(crate) struct JitState {
    functions: Vec<JitFunctionState>,
}

impl JitState {
    pub(crate) fn for_verified_program(unit: &RegUnit) -> Self {
        let mut functions = Vec::with_capacity(unit.functions.len());
        let eligibility = compute_jit_eligibility(&unit.functions);
        for (function, eligible) in unit.functions.iter().zip(eligibility) {
            functions.push(JitFunctionState {
                tier0_analysis: Some((eligible, jit_function_has_loop(&function.code))),
                #[cfg(feature = "native-jit")]
                tier0_inlined_callees: tier0_inlined_callees(unit, function),
                ..JitFunctionState::default()
            });
        }
        Self { functions }
    }

    #[inline(always)]
    fn state(&self, function: impl FunctionOrdinal) -> &JitFunctionState {
        self.functions
            .get(function.function_ordinal())
            .expect("every JIT function ordinal has a stable function state")
    }

    #[cfg(feature = "native-jit")]
    #[inline(always)]
    fn state_mut(&mut self, function: impl FunctionOrdinal) -> &mut JitFunctionState {
        self.functions
            .get_mut(function.function_ordinal())
            .expect("every JIT function ordinal has a stable function state")
    }

    pub(crate) fn tier0_analysis(
        &self,
        function_ordinal: usize,
        function: &RegFunction,
    ) -> (bool, bool) {
        self.state(function_ordinal)
            .tier0_analysis
            .unwrap_or_else(|| {
                (
                    function.code.iter().all(jit_supported_instruction),
                    jit_function_has_loop(&function.code),
                )
            })
    }

    pub(crate) fn self_recursion_kind(
        &self,
        function: impl FunctionOrdinal,
    ) -> Option<SelfRecursionKind> {
        self.state(function).self_recursion_kind
    }

    pub(crate) fn set_self_recursion_kind(
        &mut self,
        function: impl FunctionOrdinal,
        kind: SelfRecursionKind,
    ) {
        self.functions[function.function_ordinal()].self_recursion_kind = Some(kind);
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn native_status(&self, function: impl FunctionOrdinal) -> u8 {
        self.state(function).native_status
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn set_native_status(&mut self, function: impl FunctionOrdinal, status: u8) {
        self.state_mut(function).native_status = status;
    }

    /// Whether tier-0 would execute a callee of `function` inside its own frame
    /// while that callee could still reach the native tier. `run_jit` handles a
    /// `CallKnown` itself: a pure-leaf callee runs through `run_jit_pure_leaf`
    /// and never becomes a `drive` frame, so `attempt_native` is never offered
    /// the callee's body. A function that declined whole-function native entry —
    /// which any body containing a call does under an armed memory control —
    /// would therefore hide its hot helper from the native tier completely.
    ///
    /// Re-read per entry rather than cached as a verdict: once a callee's native
    /// attempt reaches an invariant decline it is marked
    /// `NATIVE_STATUS_NOT_ELIGIBLE`, and tier-0 may swallow it again.
    #[cfg(feature = "native-jit")]
    pub(crate) fn tier0_hides_native_callee(&self, function: impl FunctionOrdinal) -> bool {
        let callees = &self.state(function).tier0_inlined_callees;
        callees.iter().any(|&callee| {
            self.functions
                .get(callee)
                .is_some_and(|state| state.native_status != NATIVE_STATUS_NOT_ELIGIBLE)
        })
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn call_count(&self, _function: impl FunctionOrdinal) -> u32 {
        0
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn profile(&self, _function: impl FunctionOrdinal) -> Option<&FunctionProfile> {
        None
    }
}

/// The callee ordinals `RegVm::run_jit` would execute through
/// `run_jit_pure_leaf` instead of pushing a `drive` frame for: a `CallKnown`
/// with no `mut` argument whose callee body is entirely tier-0 supported.
/// Deduplicated and bounded so the per-entry check stays a short scan.
#[cfg(feature = "native-jit")]
fn tier0_inlined_callees(unit: &RegUnit, function: &RegFunction) -> Vec<usize> {
    /// A body with more distinct inlined callees than this is not a
    /// "`main` calls one hot helper" shape; keep the per-entry check bounded.
    const MAX_TRACKED_CALLEES: usize = 8;
    let mut callees: Vec<usize> = Vec::new();
    for instr in &function.code {
        let RegInstr::CallKnown {
            function: callee_id,
            mut_args,
            ..
        } = instr
        else {
            continue;
        };
        if !mut_args.is_empty() || callees.contains(callee_id) {
            continue;
        }
        let Some(callee) = unit.functions.get(*callee_id) else {
            continue;
        };
        if !callee.code.iter().all(jit_supported_instruction) {
            // `run_jit` drives this one through `run_frame` -> `drive`, which
            // already offers the callee to the native tier.
            continue;
        }
        if callees.len() == MAX_TRACKED_CALLEES {
            return callees;
        }
        callees.push(*callee_id);
    }
    callees
}

#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::*;

    #[cfg(feature = "native-jit")]
    fn unit() -> RegUnit {
        RegUnit {
            functions: vec![Rc::new(RegFunction::placeholder("main".into()))],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn state_is_indexed_by_verified_function_ordinal() {
        let unit = unit();
        let state = JitState::for_verified_program(&unit);
        assert_eq!(state.tier0_analysis(0, &unit.functions[0]), (true, false));
    }

    #[cfg(feature = "native-jit")]
    fn function(ordinal: usize, name: &str, code: Vec<RegInstr>) -> Rc<RegFunction> {
        let mut function = RegFunction::placeholder(name.into());
        function.ordinal = ordinal;
        function.code = code;
        Rc::new(function)
    }

    #[cfg(feature = "native-jit")]
    fn call(callee: usize, mut_args: Vec<usize>) -> RegInstr {
        RegInstr::CallKnown {
            dst: 0,
            function: callee,
            args: Vec::new(),
            mut_args,
        }
    }

    #[cfg(feature = "native-jit")]
    fn unit_with(functions: Vec<Rc<RegFunction>>) -> RegUnit {
        RegUnit {
            functions,
            ..unit()
        }
    }

    /// A caller whose tier-0 run would execute a pure-leaf callee inside its own
    /// frame hides that callee from `RegVm::attempt_native` entirely, so `drive`
    /// has to keep the caller on the interpreter loop until the callee's own
    /// native attempt reaches an invariant decline.
    #[cfg(feature = "native-jit")]
    #[test]
    fn a_pure_leaf_callee_is_tracked_until_it_is_known_not_native_eligible() {
        let unit = unit_with(vec![
            function(
                0,
                "main",
                vec![call(1, Vec::new()), RegInstr::Return { src: 0 }],
            ),
            function(1, "hot", vec![RegInstr::Return { src: 0 }]),
        ]);
        let mut state = JitState::for_verified_program(&unit);
        assert!(state.tier0_hides_native_callee(0));
        assert!(!state.tier0_hides_native_callee(1));
        state.set_native_status(1, NATIVE_STATUS_NOT_ELIGIBLE);
        assert!(!state.tier0_hides_native_callee(0));
    }

    /// `RegVm::run_jit` sends a `mut`-argument call and a callee it cannot run
    /// itself through `run_frame` -> `drive`, which already offers the callee to
    /// the native tier. Neither needs the interpreter loop.
    #[cfg(feature = "native-jit")]
    #[test]
    fn a_callee_drive_already_sees_is_not_tracked() {
        let unit = unit_with(vec![
            function(0, "main", vec![call(1, vec![0]), call(2, Vec::new())]),
            function(1, "mutating", vec![RegInstr::Return { src: 0 }]),
            function(2, "not_tier0", vec![RegInstr::AwaitJoin { dst: 0, src: 0 }]),
        ]);
        let state = JitState::for_verified_program(&unit);
        assert!(!state.tier0_hides_native_callee(0));
    }
}
