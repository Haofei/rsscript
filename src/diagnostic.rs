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
