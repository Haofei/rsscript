use crate::diagnostic::Diagnostic;

// The native bridge value type and host-function signature live in the shared
// `rss-native-abi` crate so dynamically loaded plugin cdylibs agree on their
// layout. Re-exported so existing `eval_types::NativeValue` paths stay stable.
pub use rss_native_abi::{NativeInterpreterFn, NativeValue};

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
