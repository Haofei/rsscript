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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    let mut output = String::from("[");
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!(
            "{{\"code\":\"{}\",\"severity\":\"{}\",\"summary\":\"{}\",\"spans\":[{{\"file\":\"{}\",\"line\":{},\"column\":{},\"length\":{},\"label\":\"{}\"}}],\"causes\":[{}],\"fixes\":[{}]}}",
            escape_json(&diagnostic.code),
            diagnostic.severity.as_str(),
            escape_json(&diagnostic.summary),
            escape_json(&diagnostic.span.file),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.span.length,
            escape_json(&diagnostic.label),
            diagnostic
                .causes
                .iter()
                .map(|cause| format!("\"{}\"", escape_json(cause)))
                .collect::<Vec<_>>()
                .join(","),
            diagnostic
                .fixes
                .iter()
                .map(|fix| format!(
                    "{{\"kind\":\"{}\",\"title\":\"{}\",\"applicability\":\"{}\"}}",
                    escape_json(&fix.kind),
                    escape_json(&fix.title),
                    escape_json(&fix.applicability)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    output.push(']');
    output
}

fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}
