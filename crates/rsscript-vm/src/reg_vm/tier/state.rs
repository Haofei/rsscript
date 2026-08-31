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


    #[cfg(feature = "native-jit")]
    pub(crate) fn call_count(&self, _function: impl FunctionOrdinal) -> u32 {
        0
    }



    #[cfg(feature = "native-jit")]
    pub(crate) fn profile(&self, _function: impl FunctionOrdinal) -> Option<&FunctionProfile> {
        None
    }



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

}
