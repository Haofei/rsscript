//! Evaluation-local state for experimental JIT planning.
//!
//! A verified bytecode program is immutable data. JIT planning data therefore
//! never lives on `RegFunction` (which is part of that decoded program): it is
//! owned by one evaluation and indexed directly by stable function ordinal.
//! Pointer lookup is only an internal bridge from the decoded register function
//! to that ordinal; no pointer identity is retained as the identity of a program
//! or a cache entry.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VerifiedProgramIdentity(String);

impl VerifiedProgramIdentity {
    pub(super) fn from_executable_digest(digest: impl Into<String>) -> Self {
        Self(digest.into())
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
    _program: VerifiedProgramIdentity,
    ordinal_by_function_pointer: HashMap<usize, u32>,
    functions: Vec<JitFunctionState>,
}

impl JitState {
    pub(crate) fn for_verified_program(
        executable_digest: impl Into<String>,
        unit: &RegUnit,
    ) -> Self {
        let program = VerifiedProgramIdentity::from_executable_digest(executable_digest);
        let mut ordinal_by_function_pointer = HashMap::new();
        let mut functions = Vec::with_capacity(unit.functions.len());
        let eligibility = compute_jit_eligibility(&unit.functions);
        for ((ordinal, function), eligible) in unit.functions.iter().enumerate().zip(eligibility) {
            let ordinal = ordinal
                .try_into()
                .expect("verified function count fits u32");
            ordinal_by_function_pointer.insert(Rc::as_ptr(function) as usize, ordinal);
            functions.push(JitFunctionState {
                tier0_analysis: Some((eligible, jit_function_has_loop(&function.code))),
                ..JitFunctionState::default()
            });
        }
        Self {
            _program: program,
            ordinal_by_function_pointer,
            functions,
        }
    }

    #[inline(always)]
    fn ordinal(&self, function: &RegFunction) -> usize {
        let pointer = function as *const RegFunction as usize;
        self.ordinal_by_function_pointer
            .get(&pointer)
            .copied()
            .expect("JIT state must be constructed from the decoded verified unit") as usize
    }

    #[inline(always)]
    fn state(&self, function: &RegFunction) -> &JitFunctionState {
        self.functions
            .get(self.ordinal(function))
            .expect("every JIT function ordinal has a stable function state")
    }

    #[cfg(feature = "native-jit")]
    #[inline(always)]
    fn state_mut(&mut self, function: &RegFunction) -> &mut JitFunctionState {
        let ordinal = self.ordinal(function);
        self.functions
            .get_mut(ordinal)
            .expect("every JIT function ordinal has a stable function state")
    }

    /// Stable, evaluation-local function identity for native side tables.
    #[cfg(feature = "native-jit")]
    #[inline(always)]
    pub(crate) fn function_ordinal(&self, function: &RegFunction) -> usize {
        self.ordinal(function)
    }

    pub(crate) fn tier0_analysis(&self, function: &RegFunction) -> (bool, bool) {
        self.state(function).tier0_analysis.unwrap_or_else(|| {
            (
                function.code.iter().all(jit_supported_instruction),
                jit_function_has_loop(&function.code),
            )
        })
    }

    pub(crate) fn self_recursion_kind(&self, function: &RegFunction) -> Option<SelfRecursionKind> {
        self.state(function).self_recursion_kind
    }

    pub(crate) fn set_self_recursion_kind(
        &mut self,
        function: &RegFunction,
        kind: SelfRecursionKind,
    ) {
        let ordinal = self.ordinal(function);
        self.functions[ordinal].self_recursion_kind = Some(kind);
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn native_status(&self, function: &RegFunction) -> u8 {
        self.state(function).native_status
    }

    #[cfg(feature = "native-jit")]
    pub(crate) fn set_native_status(&mut self, function: &RegFunction, status: u8) {
        self.state_mut(function).native_status = status;
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn call_count(&self, function: &RegFunction) -> u32 {
        self.state(function).call_count
    }

    #[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
    pub(crate) fn call_count(&self, _function: &RegFunction) -> u32 {
        0
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn branch_count(&self, function: &RegFunction) -> u32 {
        self.state(function).branch_count
    }

    #[cfg(feature = "jit-speculation")]
    pub(crate) fn profile(&self, function: &RegFunction) -> Option<&FunctionProfile> {
        self.state(function).profile.as_deref()
    }

    #[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
    pub(crate) fn profile(&self, _function: &RegFunction) -> Option<&FunctionProfile> {
        None
    }

    #[cfg(feature = "jit-speculation")]
    #[inline(always)]
    pub(crate) fn should_record_call(&self, function: &RegFunction) -> bool {
        self.state(function).call_count < PROFILE_RECORD_LIMIT
    }

    /// Warm-gated, bounded dynamic-call feedback. It observes dispatch only and
    /// never changes an interpreted value or branch decision.
    #[cfg(feature = "jit-speculation")]
    pub(crate) fn record_call_site(
        &mut self,
        function: &RegFunction,
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
        function: &RegFunction,
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn state_keys_function_feedback_by_verified_program_digest_and_ordinal() {
        let unit = unit();
        let first = JitState::for_verified_program("sha256:first", &unit);
        let second = JitState::for_verified_program("sha256:second", &unit);
        assert_eq!(first.function_ordinal(&unit.functions[0]), 0);
        assert_eq!(second.function_ordinal(&unit.functions[0]), 0);
        assert_ne!(first._program, second._program);
    }

    #[cfg(feature = "jit-speculation")]
    #[test]
    fn profile_feedback_is_isolated_per_evaluation() {
        let unit = unit();
        let function = &unit.functions[0];
        let mut first = JitState::for_verified_program("sha256:first", &unit);
        let second = JitState::for_verified_program("sha256:second", &unit);

        for _ in 0..=PROFILE_WARMUP {
            first.record_call_site(function, 7, 42, true);
        }

        assert_eq!(first.call_count(function), PROFILE_WARMUP + 1);
        assert!(
            first
                .profile(function)
                .and_then(|profile| profile.call_sites.get(&7))
                .is_some()
        );
        assert_eq!(second.call_count(function), 0);
        assert!(second.profile(function).is_none());
    }
}
