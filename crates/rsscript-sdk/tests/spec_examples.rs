//! Checks every labelled example in the language specification and its
//! semantics reference against the frontend checker.
//!
//! Both documents promise that each example behaves as its label says. Until
//! this target existed nothing ran them, so a change to the checker could leave
//! an example stale without anyone noticing.
//!
//! # Conventions
//!
//! The conventions are stated for readers in "How to read this document" at the
//! top of `docs/spec/RSScript_Semantics_v0.7.md`; this is the executable form.
//!
//! * A block fenced ```` ```rsscript ```` or ```` ```rsscript-lint ```` is an
//!   example, and the paragraph immediately before it must begin with a label:
//!   * `**Accepted**` — the block checks with no diagnostic at all;
//!   * ``**Rejected — `CODE`**`` — the block emits `CODE` as an error (it may
//!     emit other codes too; the label's prose names them);
//!   * ``**Warning — `CODE`**`` — the block emits `CODE` as a warning and no
//!     error.
//!
//!   An example with no label is a failure: a fragment that is not meant to
//!   check uses a plain ```` ``` ```` fence.
//! * A ```` ```rsscript-lint ```` example is also linted, as
//!   `rss check --lint` does.
//! * A ```` ```rsscript-interface ```` block is a companion interface. It is
//!   supplied, as `rss check --interface` would supply it, to every later
//!   example in the same `##` section.
//!
//! Every example is otherwise checked the way `rss check <file>` checks a single
//! file (`crates/rsscript-cli/src/cli/check.rs`): with the standard package
//! interfaces.

use std::fs;
use std::path::{Path, PathBuf};

use rsscript_diagnostics::{Diagnostic, Severity};
use rsscript_semantics::{analyze_source_with_interfaces, standard_package_interfaces};
use rsscript_syntax::lint_source;

/// The documents whose examples are checked, relative to the repository root.
const DOCUMENTS: &[&str] = &["docs/spec/RSScript_Semantics_v0.7.md"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expectation {
    Accepted,
    Rejected(String),
    Warning(String),
}

#[derive(Debug)]
struct Example {
    document: &'static str,
    /// 1-based line of the opening fence.
    line: usize,
    section: String,
    expectation: Expectation,
    lint: bool,
    source: String,
    interface: Option<String>,
}

impl Example {
    fn location(&self) -> String {
        format!("{}:{} ({})", self.document, self.line, self.section)
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("rsscript-sdk lives two levels below the repository root")
        .to_path_buf()
}

fn is_diagnostic_code(code: &str) -> bool {
    let digits = code
        .strip_prefix("RSL")
        .or_else(|| code.strip_prefix("RS"))
        .unwrap_or("");
    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}

/// Read a label from the first line of the paragraph before an example.
fn parse_label(paragraph: &str) -> Option<Expectation> {
    if paragraph.starts_with("**Accepted**") {
        return Some(Expectation::Accepted);
    }
    type Labelled = fn(String) -> Expectation;
    let labels: [(&str, Labelled); 2] = [
        ("**Rejected — `", Expectation::Rejected),
        ("**Warning — `", Expectation::Warning),
    ];
    for (prefix, expectation) in labels {
        if let Some(rest) = paragraph.strip_prefix(prefix) {
            let (code, tail) = rest.split_once('`')?;
            return (tail.starts_with("**") && is_diagnostic_code(code))
                .then(|| expectation(code.to_string()));
        }
    }
    None
}

/// Extract every example from one document. An example block with no label is
/// returned in the second list, as a location.
fn extract(document: &'static str, text: &str) -> (Vec<Example>, Vec<String>) {
    let lines = text.lines().collect::<Vec<_>>();
    let mut examples = Vec::new();
    let mut unlabelled = Vec::new();
    let mut section = String::from("(before the first section)");
    let mut interface: Option<String> = None;
    // The first line of the most recent paragraph, and whether it is still open.
    let mut paragraph: Option<&str> = None;
    let mut in_paragraph = false;

    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(info) = line.strip_prefix("```") {
            let language = info.trim();
            let close = (index + 1..lines.len())
                .find(|candidate| lines[*candidate].trim_end() == "```")
                .unwrap_or_else(|| panic!("{document}:{}: unterminated fence", index + 1));
            let mut body = lines[index + 1..close].join("\n");
            body.push('\n');
            match language {
                "rsscript-interface" => interface = Some(body),
                "rsscript" | "rsscript-lint" => match paragraph.take().and_then(parse_label) {
                    Some(expectation) => examples.push(Example {
                        document,
                        line: index + 1,
                        section: section.clone(),
                        expectation,
                        lint: language == "rsscript-lint",
                        source: body,
                        interface: interface.clone(),
                    }),
                    None => unlabelled.push(format!("{document}:{} ({section})", index + 1)),
                },
                _ => {}
            }
            paragraph = None;
            in_paragraph = false;
            index = close + 1;
            continue;
        }
        if line.starts_with("## ") {
            section = line.trim_start_matches("## ").to_string();
            interface = None;
        }
        if line.trim().is_empty() {
            in_paragraph = false;
        } else if !in_paragraph {
            paragraph = Some(line);
            in_paragraph = true;
        }
        index += 1;
    }
    (examples, unlabelled)
}

fn check(example: &Example) -> Vec<Diagnostic> {
    let file = "spec_example.rss";
    let mut interfaces = standard_package_interfaces().to_vec();
    if let Some(interface) = &example.interface {
        interfaces.push(("spec_example_companion.rssi", interface.as_str()));
    }
    let mut diagnostics = analyze_source_with_interfaces(file, &example.source, &interfaces);
    if example.lint {
        diagnostics.extend(lint_source(file, &example.source));
    }
    diagnostics
}

fn render(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics".to_string();
    }
    diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            format!("{severity}[{}] {}", diagnostic.code, diagnostic.summary)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The reason an example does not behave as labelled, or `None` if it does.
fn mismatch(example: &Example, diagnostics: &[Diagnostic]) -> Option<String> {
    let emits = |code: &str, severity: Severity| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.severity == severity)
    };
    let any_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error());
    let expected = match &example.expectation {
        Expectation::Accepted if diagnostics.is_empty() => return None,
        Expectation::Accepted => "no diagnostics".to_string(),
        Expectation::Rejected(code) if emits(code, Severity::Error) => return None,
        Expectation::Rejected(code) => format!("error {code}"),
        Expectation::Warning(code) if emits(code, Severity::Warning) && !any_error => {
            return None;
        }
        Expectation::Warning(code) => format!("warning {code} and no error"),
    };
    Some(format!(
        "{}: labelled to produce {expected}, got {}",
        example.location(),
        render(diagnostics)
    ))
}

#[test]
fn labelled_spec_examples_behave_as_labelled() {
    let root = repository_root();
    let mut examples = Vec::new();
    let mut unlabelled = Vec::new();
    for document in DOCUMENTS {
        let path = root.join(document);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let (found, missing) = extract(document, &text);
        assert!(
            !found.is_empty(),
            "{document} should contain at least one labelled example"
        );
        examples.extend(found);
        unlabelled.extend(missing);
    }

    assert!(
        unlabelled.is_empty(),
        "every ```rsscript example needs an **Accepted**, **Rejected — `CODE`**, or **Warning — `CODE`** label on the paragraph before it (use a plain ``` fence for a fragment):\n{}",
        unlabelled.join("\n")
    );

    let failures = examples
        .iter()
        .filter_map(|example| mismatch(example, &check(example)))
        .collect::<Vec<_>>();
    assert!(
        failures.is_empty(),
        "{} of {} spec examples do not behave as labelled:\n{}",
        failures.len(),
        examples.len(),
        failures.join("\n")
    );

    let count = |wanted: fn(&Expectation) -> bool| {
        examples
            .iter()
            .filter(|example| wanted(&example.expectation))
            .count()
    };
    println!(
        "checked {} spec examples: {} accepted, {} rejected, {} warning",
        examples.len(),
        count(|expectation| matches!(expectation, Expectation::Accepted)),
        count(|expectation| matches!(expectation, Expectation::Rejected(_))),
        count(|expectation| matches!(expectation, Expectation::Warning(_))),
    );
}

#[test]
fn labels_parse_only_in_their_documented_shapes() {
    assert_eq!(
        parse_label("**Accepted** — field defaults"),
        Some(Expectation::Accepted)
    );
    assert_eq!(
        parse_label("**Rejected — `RS0015`** (module after a declaration)"),
        Some(Expectation::Rejected("RS0015".to_string()))
    );
    assert_eq!(
        parse_label("**Warning — `RSL001`**"),
        Some(Expectation::Warning("RSL001".to_string()))
    );
    assert_eq!(parse_label("**What lowers.** Lowering accepts"), None);
    assert_eq!(parse_label("**Rejected — `oops`**"), None);
    assert_eq!(parse_label("Accepted"), None);
}
