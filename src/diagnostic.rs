use serde::Serialize;

pub mod code {
    pub const MISSING_FILE_MODE: &str = "RS0001";
    pub const MISSING_RETURN_TYPE: &str = "RS0002";
    pub const MISSING_PARAMETER_TYPE: &str = "RS0003";
    pub const UNKNOWN_EFFECT: &str = "RS0004";
    pub const FILE_MODE_VIOLATION: &str = "RS0101";
    pub const UNNAMED_ARGUMENT: &str = "RS0201";
    pub const MISSING_DATA_EFFECT: &str = "RS0202";
    pub const MANAGED_TO_LOCAL: &str = "RS0301";
    pub const USE_AFTER_MANAGE: &str = "RS0401";
    pub const LOCAL_VALUE_RETAINED: &str = "RS0501";
    pub const FRESH_RETURN_NOT_CLEAN: &str = "RS0601";
    pub const FRESHNESS_UNKNOWN: &str = "RS0602";
    pub const RESOURCE_FIELD: &str = "RS0701";
    pub const RESOURCE_ESCAPE: &str = "RS0702";
    pub const LOCAL_CAPTURED_BY_MANAGED_CLOSURE: &str = "RS0801";
    pub const TAKE_HANDLE_FIELD: &str = "RS0901";
    pub const OPERATOR_OVERLOAD_ATTEMPT: &str = "RS1001";

    pub const REVIEW_MODE_CHANGED: &str = "RSR001";
    pub const REVIEW_FUNCTION_REMOVED: &str = "RSR002";
    pub const REVIEW_FUNCTION_ADDED: &str = "RSR003";
    pub const REVIEW_PARAMS_CHANGED: &str = "RSR004";
    pub const REVIEW_RETURN_CHANGED: &str = "RSR005";
    pub const REVIEW_EFFECTS_CHANGED: &str = "RSR006";
    pub const REVIEW_TYPE_REMOVED: &str = "RSR007";
    pub const REVIEW_TYPE_ADDED: &str = "RSR008";
    pub const REVIEW_TYPE_KIND_CHANGED: &str = "RSR009";
    pub const REVIEW_TYPE_FIELDS_CHANGED: &str = "RSR010";
    pub const REVIEW_BOUNDARY_CHANGED: &str = "RSR011";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub kind: String,
    pub title: String,
    pub applicability: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub summary: String,
    pub span: Span,
    pub label: String,
    pub causes: Vec<String>,
    pub fixes: Vec<Fix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticExplanation {
    pub code: &'static str,
    pub title: &'static str,
    pub explanation: &'static str,
}

impl Diagnostic {
    pub fn error(
        code: &str,
        summary: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Error,
            summary: summary.into(),
            span,
            label: label.into(),
            causes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub fn warning(
        code: &str,
        summary: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Warning,
            summary: summary.into(),
            span,
            label: label.into(),
            causes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.causes.push(cause.into());
        self
    }

    pub fn with_fix(
        mut self,
        kind: impl Into<String>,
        title: impl Into<String>,
        applicability: impl Into<String>,
    ) -> Self {
        self.fixes.push(Fix {
            kind: kind.into(),
            title: title.into(),
            applicability: applicability.into(),
        });
        self
    }
}

pub fn explain_diagnostic_code(code: &str) -> Option<&'static DiagnosticExplanation> {
    DIAGNOSTIC_EXPLANATIONS
        .iter()
        .find(|explanation| explanation.code == code)
}

pub fn format_diagnostic_explanation(explanation: &DiagnosticExplanation) -> String {
    format!(
        "{}: {}\n\n{}\n",
        explanation.code, explanation.title, explanation.explanation
    )
}

pub fn format_diagnostics_human(diagnostics: &[Diagnostic]) -> String {
    let mut output = String::new();
    for diagnostic in diagnostics {
        output.push_str(&format!(
            "{}[{}]: {}\n\n",
            diagnostic.severity.as_str(),
            diagnostic.code,
            diagnostic.summary
        ));
        output.push_str(&format!(
            "  {}:{}:{}\n    {}{}\n",
            diagnostic.span.file,
            diagnostic.span.line,
            diagnostic.span.column,
            " ".repeat(diagnostic.span.column.saturating_sub(1)),
            "^".repeat(diagnostic.span.length.max(1))
        ));
        if !diagnostic.label.is_empty() {
            output.push_str(&format!("    {}\n", diagnostic.label));
        }
        for cause in &diagnostic.causes {
            output.push_str(&format!("\n  note: {cause}\n"));
        }
        for fix in &diagnostic.fixes {
            output.push_str(&format!("  help: {}\n", fix.title));
        }
        output.push('\n');
    }
    output
}

static DIAGNOSTIC_EXPLANATIONS: &[DiagnosticExplanation] = &[
    DiagnosticExplanation {
        code: code::MISSING_FILE_MODE,
        title: "missing file mode",
        explanation: "Every RSScript source file must declare `mode: managed` or `mode: uses-local` so reviewers can see whether local ownership features are allowed.",
    },
    DiagnosticExplanation {
        code: code::MISSING_RETURN_TYPE,
        title: "missing return type",
        explanation: "Function signatures must spell out their return type. The checker applies this review rule broadly so API contracts do not rely on inference.",
    },
    DiagnosticExplanation {
        code: code::MISSING_PARAMETER_TYPE,
        title: "missing parameter type",
        explanation: "Parameters must have explicit types so call effects, freshness, and resource rules can be checked against a stable signature.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_EFFECT,
        title: "unknown effect",
        explanation: "The effect list contains an effect name outside the currently recognized MVP surface.",
    },
    DiagnosticExplanation {
        code: code::FILE_MODE_VIOLATION,
        title: "file mode violation",
        explanation: "`mode: managed` files cannot use local-only features such as `local`, `manage`, `take`, or `ResourcePool<T>`.",
    },
    DiagnosticExplanation {
        code: code::UNNAMED_ARGUMENT,
        title: "unnamed argument",
        explanation: "RSScript requires named call arguments so signature and review diffs remain readable.",
    },
    DiagnosticExplanation {
        code: code::MISSING_DATA_EFFECT,
        title: "missing call-site data effect",
        explanation: "Arguments for non-Copy parameters must use an explicit `read`, `mut`, or `take` effect matching the callee signature.",
    },
    DiagnosticExplanation {
        code: code::MANAGED_TO_LOCAL,
        title: "managed-to-local conversion",
        explanation: "Managed values cannot be rebound as local values. Create the value locally at its origin if local ownership is required.",
    },
    DiagnosticExplanation {
        code: code::USE_AFTER_MANAGE,
        title: "use after manage",
        explanation: "`manage value` moves a local value into the managed runtime. The original local binding cannot be used afterwards on any reachable path.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_VALUE_RETAINED,
        title: "local value retained",
        explanation: "APIs marked `effects(retains(param))` may store the argument beyond the call. Passing a clean local value directly would let local ownership escape.",
    },
    DiagnosticExplanation {
        code: code::FRESH_RETURN_NOT_CLEAN,
        title: "fresh return is not clean",
        explanation: "A `fresh` function may only return a newly created value, a known fresh call, or a clean local value that has not escaped through manage, take, retain, or capture.",
    },
    DiagnosticExplanation {
        code: code::FRESHNESS_UNKNOWN,
        title: "freshness unknown",
        explanation: "The MVP checker could not prove the returned value is fresh. Current proof support trusts clean locals, struct constructors, and known fresh calls.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_FIELD,
        title: "resource field",
        explanation: "Resource values cannot be stored directly in ordinary class or struct fields. Use `with` or an approved resource container such as `ResourcePool<T>`.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_ESCAPE,
        title: "resource escape",
        explanation: "A resource introduced by `with` must not escape the block through return, manage, retention, or managed closure capture.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
        title: "local captured by managed closure",
        explanation: "A closure bound with `let` is managed and may outlive clean local values. Use a local/noescape callback shape instead.",
    },
    DiagnosticExplanation {
        code: code::TAKE_HANDLE_FIELD,
        title: "take of handle field",
        explanation: "Handle fields are managed references. They cannot be consumed with `take` as if they were inline local fields.",
    },
    DiagnosticExplanation {
        code: code::OPERATOR_OVERLOAD_ATTEMPT,
        title: "operator overload attempt",
        explanation: "The MVP language surface rejects likely user-defined operator overloads to keep review semantics explicit.",
    },
];

pub fn format_diagnostics_json(diagnostics: &[Diagnostic]) -> String {
    let diagnostics: Vec<JsonDiagnostic<'_>> =
        diagnostics.iter().map(JsonDiagnostic::from).collect();
    serde_json::to_string(&diagnostics).expect("diagnostic JSON serialization should not fail")
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    code: &'a str,
    severity: &'a str,
    summary: &'a str,
    spans: Vec<JsonSpan<'a>>,
    causes: &'a [String],
    fixes: &'a [Fix],
}

#[derive(Serialize)]
struct JsonSpan<'a> {
    file: &'a str,
    line: usize,
    column: usize,
    length: usize,
    label: &'a str,
}

impl<'a> From<&'a Diagnostic> for JsonDiagnostic<'a> {
    fn from(diagnostic: &'a Diagnostic) -> Self {
        Self {
            code: &diagnostic.code,
            severity: diagnostic.severity.as_str(),
            summary: &diagnostic.summary,
            spans: vec![JsonSpan {
                file: &diagnostic.span.file,
                line: diagnostic.span.line,
                column: diagnostic.span.column,
                length: diagnostic.span.length,
                label: &diagnostic.label,
            }],
            causes: &diagnostic.causes,
            fixes: &diagnostic.fixes,
        }
    }
}
