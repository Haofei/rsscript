use super::{JitError, JitFunction, validate};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValidationMode {
    Standard,
    Osr,
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
}

impl<'a> ValidatedJitFunction<'a> {
    /// Validate a function for normal entry compilation.
    pub fn new(function: &'a JitFunction) -> Result<Self, JitError> {
        Self::for_mode(function, ValidationMode::Standard)
    }

    pub(crate) fn for_osr(function: &'a JitFunction) -> Result<Self, JitError> {
        Self::for_mode(function, ValidationMode::Osr)
    }

    fn for_mode(function: &'a JitFunction, mode: ValidationMode) -> Result<Self, JitError> {
        validate(function, mode == ValidationMode::Osr)?;
        Ok(Self { function, mode })
    }

    pub(crate) fn function(&self) -> &'a JitFunction {
        self.function
    }

    pub(crate) fn mode(&self) -> ValidationMode {
        self.mode
    }
}

/// Validate public JIT IR before it crosses the code-generation boundary.
pub fn validate_function(function: &JitFunction) -> Result<ValidatedJitFunction<'_>, JitError> {
    ValidatedJitFunction::new(function)
}
