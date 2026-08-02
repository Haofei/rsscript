use crate::diagnostic::Diagnostic;
use std::collections::BTreeMap;

// The native bridge value type and host-function signature live in the shared
// `rss-native-abi` crate so dynamically loaded plugin cdylibs agree on their
// layout. Re-exported so existing `eval_types::NativeValue` paths stay stable.
pub use rss_native_abi::NativeValue;
pub type ExternalFunction = rss_native_abi::NativeInterpreterFn;

/// Runtime-owned symbol table for functions supplied by package providers.
/// Compilation and lowering only record the symbol name; provider selection is
/// deliberately deferred until execution.
#[derive(Default)]
pub struct ExternalFunctionRegistry {
    functions: BTreeMap<String, ExternalFunction>,
}

impl ExternalFunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, symbol: impl Into<String>, function: ExternalFunction) {
        self.functions.insert(symbol.into(), function);
    }

    pub fn resolve(&self, symbol: &str) -> Option<&ExternalFunction> {
        self.functions.get(symbol)
    }

    pub fn into_bindings(self) -> impl Iterator<Item = (String, ExternalFunction)> {
        self.functions.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalOutput {
    pub value: String,
    pub display_value: String,
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    Diagnostics(Vec<Diagnostic>),
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageBucket {
    pub all: Vec<String>,
    pub supported: Vec<String>,
    pub missing: Vec<String>,
}

impl CoverageBucket {
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn supported_count(&self) -> usize {
        self.supported.len()
    }

    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }
}
