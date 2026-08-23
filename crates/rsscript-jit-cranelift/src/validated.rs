use super::{JitError, JitFunction, JitLimits, validate_with_limits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValidationMode {
    Standard,
    Osr,
}

pub(crate) struct ValidationFacts {
    pub(crate) assigned_in: Vec<Vec<bool>>,
    pub(crate) deopt_in: Vec<Vec<bool>>,
    pub(crate) return_type: Option<super::JitValueType>,
}

/// A JIT function whose structural, type, control-flow, and resource invariants
/// have been checked for one compilation mode.
///
/// The inner function remains borrowed for the lifetime of this proof, so safe
/// code cannot mutate the public IR between validation and code generation.
/// Construction is intentionally limited to this module.
pub struct ValidatedJitFunction<'a> {
    function: &'a JitFunction,
    mode: ValidationMode,
    facts: ValidationFacts,
}

impl<'a> ValidatedJitFunction<'a> {
    /// Validate a function for normal entry compilation.
    pub fn new(function: &'a JitFunction) -> Result<Self, JitError> {
        Self::for_mode(function, ValidationMode::Standard, &JitLimits::default())
    }

    pub(crate) fn for_osr_with_limits(
        function: &'a JitFunction,
        limits: &JitLimits,
    ) -> Result<Self, JitError> {
        Self::for_mode(function, ValidationMode::Osr, limits)
    }

    pub fn with_limits(function: &'a JitFunction, limits: &JitLimits) -> Result<Self, JitError> {
        Self::for_mode(function, ValidationMode::Standard, limits)
    }

    fn for_mode(
        function: &'a JitFunction,
        mode: ValidationMode,
        limits: &JitLimits,
    ) -> Result<Self, JitError> {
        let facts = validate_with_limits(function, mode == ValidationMode::Osr, limits)?;
        Ok(Self {
            function,
            mode,
            facts,
        })
    }

    pub(crate) fn function(&self) -> &'a JitFunction {
        self.function
    }

    pub(crate) fn mode(&self) -> ValidationMode {
        self.mode
    }

    pub(crate) fn assigned_in(&self) -> &[Vec<bool>] {
        &self.facts.assigned_in
    }

    pub(crate) fn deopt_in(&self) -> &[Vec<bool>] {
        &self.facts.deopt_in
    }

    pub(crate) fn return_type(&self) -> Option<super::JitValueType> {
        self.facts.return_type
    }
}

/// Validate public JIT IR before it crosses the code-generation boundary.
pub fn validate_function(function: &JitFunction) -> Result<ValidatedJitFunction<'_>, JitError> {
    ValidatedJitFunction::new(function)
}
