use serde::Serialize;

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
        code: "RS0001",
        title: "missing file mode",
        explanation: "Every RSScript source file must declare `mode: managed` or `mode: uses-local` so reviewers can see whether local ownership features are allowed.",
    },
    DiagnosticExplanation {
        code: "RS0002",
        title: "missing return type",
        explanation: "Function signatures must spell out their return type. The checker applies this review rule broadly so API contracts do not rely on inference.",
    },
    DiagnosticExplanation {
        code: "RS0003",
        title: "missing parameter type",
        explanation: "Parameters must have explicit types so call effects, freshness, and resource rules can be checked against a stable signature.",
    },
    DiagnosticExplanation {
        code: "RS0004",
        title: "unknown effect",
        explanation: "The effect list contains an effect name outside the currently recognized MVP surface.",
    },
    DiagnosticExplanation {
        code: "RS0101",
        title: "file mode violation",
        explanation: "`mode: managed` files cannot use local-only features such as `local`, `manage`, `take`, or `ResourcePool<T>`.",
    },
    DiagnosticExplanation {
        code: "RS0201",
        title: "unnamed argument",
        explanation: "RSScript requires named call arguments so signature and review diffs remain readable.",
    },
    DiagnosticExplanation {
        code: "RS0202",
        title: "missing call-site data effect",
        explanation: "Arguments for non-Copy parameters must use an explicit `read`, `mut`, or `take` effect matching the callee signature.",
    },
    DiagnosticExplanation {
        code: "RS0301",
        title: "managed-to-local conversion",
        explanation: "Managed values cannot be rebound as local values. Create the value locally at its origin if local ownership is required.",
    },
    DiagnosticExplanation {
        code: "RS0401",
        title: "use after manage",
        explanation: "`manage value` moves a local value into the managed runtime. The original local binding cannot be used afterwards on any reachable path.",
    },
    DiagnosticExplanation {
        code: "RS0501",
        title: "local value retained",
        explanation: "APIs marked `effects(retains(param))` may store the argument beyond the call. Passing a clean local value directly would let local ownership escape.",
    },
    DiagnosticExplanation {
        code: "RS0601",
        title: "fresh return is not clean",
        explanation: "A `fresh` function may only return a newly created value, a known fresh call, or a clean local value that has not escaped through manage, take, retain, or capture.",
    },
    DiagnosticExplanation {
        code: "RS0602",
        title: "freshness unknown",
        explanation: "The MVP checker could not prove the returned value is fresh. Current proof support trusts clean locals, struct constructors, and known fresh calls.",
    },
    DiagnosticExplanation {
        code: "RS0701",
        title: "resource field",
        explanation: "Resource values cannot be stored directly in ordinary class or struct fields. Use `with` or an approved resource container such as `ResourcePool<T>`.",
    },
    DiagnosticExplanation {
        code: "RS0702",
        title: "resource escape",
        explanation: "A resource introduced by `with` must not escape the block through return, manage, retention, or managed closure capture.",
    },
    DiagnosticExplanation {
        code: "RS0801",
        title: "local captured by managed closure",
        explanation: "A closure bound with `let` is managed and may outlive clean local values. Use a local/noescape callback shape instead.",
    },
    DiagnosticExplanation {
        code: "RS0901",
        title: "take of handle field",
        explanation: "Handle fields are managed references. They cannot be consumed with `take` as if they were inline local fields.",
    },
    DiagnosticExplanation {
        code: "RS1001",
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
