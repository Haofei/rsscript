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
    #[cfg(feature = "jit-speculation")]
    call_count: u32,
    #[cfg(feature = "jit-speculation")]
    branch_count: u32,
    #[cfg(feature = "jit-speculation")]
    profile: Option<Box<FunctionProfile>>,
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

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn call_count(&self, function: impl FunctionOrdinal) -> u32 {
        self.state(function).call_count
    }

    #[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
    pub(crate) fn call_count(&self, _function: impl FunctionOrdinal) -> u32 {
        0
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn branch_count(&self, function: impl FunctionOrdinal) -> u32 {
        self.state(function).branch_count
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn profile(&self, function: impl FunctionOrdinal) -> Option<&FunctionProfile> {
        self.state(function).profile.as_deref()
    }

    #[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
    pub(crate) fn profile(&self, _function: impl FunctionOrdinal) -> Option<&FunctionProfile> {
        None
    }

    #[cfg(feature = "jit-speculation")]
    #[inline(always)]
    pub(crate) fn should_record_call(&self, function: impl FunctionOrdinal) -> bool {
        self.state(function).call_count < PROFILE_RECORD_LIMIT
    }

    /// Warm-gated, bounded dynamic-call feedback. It observes dispatch only and
    /// never changes an interpreted value or branch decision.
    #[cfg(feature = "jit-speculation")]
    pub(crate) fn record_call_site(
        &mut self,
        function: impl FunctionOrdinal,
        instr_idx: usize,
        callee_key: u64,
        captures_scalar: bool,
    ) {
        let state = self.state_mut(function);
        if state.call_count >= PROFILE_RECORD_LIMIT {
            return;
        }
        state.call_count = state.call_count.saturating_add(1);
        if state.call_count <= PROFILE_WARMUP {
            if state.call_count == PROFILE_WARMUP {
                state
                    .profile
                    .get_or_insert_with(|| Box::new(FunctionProfile::default()));
            }
            return;
        }
        if let Some(profile) = state.profile.as_deref_mut() {
            profile.record_call(instr_idx, callee_key, captures_scalar);
        }
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn record_branch_site(
        &mut self,
        function: impl FunctionOrdinal,
        instr_idx: usize,
        taken: bool,
    ) {
        let state = self.state_mut(function);
        if state.branch_count >= PROFILE_RECORD_LIMIT {
            return;
        }
        state.branch_count = state.branch_count.saturating_add(1);
        if state.branch_count <= PROFILE_WARMUP {
            if state.branch_count == PROFILE_WARMUP {
                state
                    .profile
                    .get_or_insert_with(|| Box::new(FunctionProfile::default()));
            }
            return;
        }
        if let Some(profile) = state.profile.as_deref_mut() {
            profile.record_branch(instr_idx, taken);
        }
    }
}

#[cfg(all(test, any(feature = "native-jit", feature = "jit-speculation")))]
mod tests {
    use super::*;

    #[cfg(any(feature = "native-jit", feature = "jit-speculation"))]
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

    #[cfg(feature = "jit-speculation")]
    #[test]
    fn profile_feedback_is_isolated_per_evaluation() {
        let unit = unit();
        let mut first = JitState::for_verified_program(&unit);
        let second = JitState::for_verified_program(&unit);

        for _ in 0..=PROFILE_WARMUP {
            first.record_call_site(0, 7, 42, true);
        }

        assert_eq!(first.call_count(0), PROFILE_WARMUP + 1);
        assert!(
            first
                .profile(0)
                .and_then(|profile| profile.call_sites.get(&7))
                .is_some()
        );
        assert_eq!(second.call_count(0), 0);
        assert!(second.profile(0).is_none());
    }
}
