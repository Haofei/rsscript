use std::cell::{BorrowError, BorrowMutError};
use std::fmt;

use crate::diagnostics::RUNTIME_DIAGNOSTIC_PREFIX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    ManagedReadConflict,
    ManagedWriteConflict,
    AssertionFailed,
    InvalidArgument,
    IntegerOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub span: Option<SourceSpan>,
}

impl RuntimeError {
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    pub fn diagnostic_json(&self) -> String {
        let span = self
            .span
            .clone()
            .unwrap_or_else(|| SourceSpan::new("<runtime>", 1, 1, 1));
        serde_json::json!({
            "code": "RS1201",
            "severity": "error",
            "summary": format!("RSScript runtime error: {}", self.message),
            "file": span.file,
            "line": span.line,
            "column": span.column,
            "length": span.length,
            "label": self.message,
            "kind": self.kind.as_str(),
        })
        .to_string()
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeErrorKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ManagedReadConflict => "managed_read_conflict",
            Self::ManagedWriteConflict => "managed_write_conflict",
            Self::AssertionFailed => "assertion_failed",
            Self::InvalidArgument => "invalid_argument",
            Self::IntegerOverflow => "integer_overflow",
        }
    }
}

pub(crate) fn panic_runtime_error(error: RuntimeError) -> ! {
    panic!("{}{}", RUNTIME_DIAGNOSTIC_PREFIX, error.diagnostic_json())
}

pub(crate) fn assertion_failed_error(message: String) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::AssertionFailed,
        message,
        span: None,
    }
}

pub(crate) fn invalid_argument_error(message: String) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::InvalidArgument,
        message,
        span: None,
    }
}

pub(crate) fn integer_overflow_error(message: String) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::IntegerOverflow,
        message,
        span: None,
    }
}

pub(crate) fn managed_read_error(error: BorrowError) -> RuntimeError {
    let _ = error;
    RuntimeError {
        kind: RuntimeErrorKind::ManagedReadConflict,
        message: "managed value is already being written".to_string(),
        span: None,
    }
}

pub(crate) fn managed_write_error(error: BorrowMutError) -> RuntimeError {
    let _ = error;
    RuntimeError {
        kind: RuntimeErrorKind::ManagedWriteConflict,
        message: "managed value is already being read or written".to_string(),
        span: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: &'static str,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl SourceSpan {
    pub const fn new(file: &'static str, line: usize, column: usize, length: usize) -> Self {
        Self {
            file,
            line,
            column,
            length,
        }
    }
}
