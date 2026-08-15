//! Evaluation-local state for experimental JIT planning.
//!
//! A verified bytecode program is immutable data. JIT planning data therefore
//! never lives on `RegFunction` (which is part of that decoded program): it is
//! owned by one evaluation and indexed by the verified executable digest plus a
//! stable function ordinal.  Pointer lookup is only an internal bridge from the
//! decoded register function to that stable key; no pointer identity is retained
//! as the identity of a program or a cache entry.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VerifiedProgramIdentity(String);

impl VerifiedProgramIdentity {
    pub(super) fn from_executable_digest(digest: impl Into<String>) -> Self {
        Self(digest.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct JitFunctionKey {
    pub(super) program: VerifiedProgramIdentity,
    pub(super) ordinal: u32,
}

#[derive(Debug, Default)]
struct JitFunctionState {
    tier0_analysis: Option<(bool, bool)>,
    self_recursion_kind: Option<SelfRecursionKind>,
}

/// Side table for one evaluation of a verified program.
///
/// This intentionally owns only experiment data. It contains no VM value,
/// source, provider, or artifact payload state; dropping the run drops all JIT
/// feedback with it.
#[derive(Debug)]
pub(crate) struct JitState {
    keys_by_function_pointer: HashMap<usize, JitFunctionKey>,
    functions: BTreeMap<JitFunctionKey, JitFunctionState>,
}

impl JitState {
    pub(crate) fn for_verified_program(
        executable_digest: impl Into<String>,
        unit: &RegUnit,
    ) -> Self {
        let program = VerifiedProgramIdentity::from_executable_digest(executable_digest);
        let mut keys_by_function_pointer = HashMap::new();
        let mut functions = BTreeMap::new();
        let eligibility = compute_jit_eligibility(&unit.functions);
        for ((ordinal, function), eligible) in unit.functions.iter().enumerate().zip(eligibility) {
            let key = JitFunctionKey {
                program: program.clone(),
                ordinal: ordinal.try_into().expect("verified function count fits u32"),
            };
            keys_by_function_pointer.insert(Rc::as_ptr(function) as usize, key.clone());
            functions.insert(
                key,
                JitFunctionState {
                    tier0_analysis: Some((eligible, jit_function_has_loop(&function.code))),
                    ..JitFunctionState::default()
                },
            );
        }
        Self {
            keys_by_function_pointer,
            functions,
        }
    }

    fn state(&self, function: &RegFunction) -> &JitFunctionState {
        let pointer = function as *const RegFunction as usize;
        let key = self
            .keys_by_function_pointer
            .get(&pointer)
            .expect("JIT state must be constructed from the decoded verified unit");
        self.functions
            .get(key)
            .expect("every JIT function pointer has a stable function state")
    }

    pub(crate) fn tier0_analysis(&self, function: &RegFunction) -> (bool, bool) {
        self.state(function).tier0_analysis.unwrap_or_else(|| {
            (
                function.code.iter().all(jit_supported_instruction),
                jit_function_has_loop(&function.code),
            )
        })
    }

    pub(crate) fn self_recursion_kind(
        &self,
        function: &RegFunction,
    ) -> Option<SelfRecursionKind> {
        self.state(function).self_recursion_kind
    }

    pub(crate) fn set_self_recursion_kind(
        &mut self,
        function: &RegFunction,
        kind: SelfRecursionKind,
    ) {
        let pointer = function as *const RegFunction as usize;
        let key = self
            .keys_by_function_pointer
            .get(&pointer)
            .expect("JIT state must be constructed from the decoded verified unit");
        self.functions
            .get_mut(key)
            .expect("every JIT function pointer has a stable function state")
            .self_recursion_kind = Some(kind);
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
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        }
    }

    #[test]
    fn state_keys_function_feedback_by_verified_program_digest_and_ordinal() {
        let unit = unit();
        let first = JitState::for_verified_program("sha256:first", &unit);
        let second = JitState::for_verified_program("sha256:second", &unit);
        let first_key = first.functions.keys().next().expect("one function");
        let second_key = second.functions.keys().next().expect("one function");

        assert_eq!(first_key.ordinal, 0);
        assert_eq!(second_key.ordinal, 0);
        assert_ne!(first_key.program, second_key.program);
    }
}
