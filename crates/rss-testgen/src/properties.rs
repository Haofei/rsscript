//! Property drivers over a generated (or supplied) program.
//!
//! Each driver encodes one testing contract and is reusable from both the
//! in-suite proptest tests and the libFuzzer targets. They take *source* (not a
//! `GeneratedProgram`) so a fixed corpus file or a hand-written repro can be fed
//! through the same checks.

use crate::backends::{Backend, all_inprocess_backends};

/// A backend divergence: two backends produced different results for the same
/// program. Carries enough to localize and reproduce.
#[derive(Debug, Clone)]
pub struct Divergence {
    pub file: String,
    pub left: String,
    pub left_result: Result<String, String>,
    pub right: String,
    pub right_result: Result<String, String>,
    pub source: String,
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backend divergence on {file}: `{left}` => {lr:?} vs `{right}` => {rr:?}\n\
             --- source ---\n{src}",
            file = self.file,
            left = self.left,
            lr = self.left_result,
            right = self.right,
            rr = self.right_result,
            src = self.source,
        )
    }
}

/// Run `source` on `backends` and require they agree: either all succeed with the
/// identical stdout, or all fail. Returns the agreed stdout on success, or the
/// first `Divergence` found.
///
/// The success/failure *status* must match across backends (a program that traps
/// on one engine must trap on all); when all succeed, the stdout must match too.
/// Error *messages* are intentionally not compared — each backend formats them
/// differently — only the success-vs-failure classification.
pub fn check_agreement(
    file: &str,
    source: &str,
    args: &[&str],
    backends: &[Box<dyn Backend>],
) -> Result<Option<String>, Box<Divergence>> {
    let mut reference: Option<(&'static str, Result<String, String>)> = None;
    for backend in backends {
        let result = backend.run_stdout(file, source, args);
        match &reference {
            None => reference = Some((backend.name(), result)),
            Some((ref_name, ref_result)) => {
                let agree = match (ref_result, &result) {
                    (Ok(left), Ok(right)) => left == right,
                    (Err(_), Err(_)) => true,
                    _ => false,
                };
                if !agree {
                    return Err(Box::new(Divergence {
                        file: file.to_string(),
                        left: ref_name.to_string(),
                        left_result: ref_result.clone(),
                        right: backend.name().to_string(),
                        right_result: result,
                        source: source.to_string(),
                    }));
                }
            }
        }
    }
    Ok(reference.and_then(|(_, result)| result.ok()))
}

/// In-process differential: all VM-family backends must agree. Panics with the
/// divergence (so proptest shrinks / libFuzzer records a crash) on mismatch.
pub fn assert_inprocess_agree(file: &str, source: &str, args: &[&str]) -> Option<String> {
    match check_agreement(file, source, args, &all_inprocess_backends()) {
        Ok(stdout) => stdout,
        Err(divergence) => panic!("{divergence}"),
    }
}

/// Robustness contract: the front end must never panic, hang, or stack-overflow
/// on any input, and lowering is attempted only for programs the checker accepts
/// (the "RSScript owns semantics" rule). Returns whether the program was accepted.
pub fn exercise_front_end(file: &str, source: &str) -> bool {
    let diagnostics = rsscript::analyze_source(file, source);
    let _ = rsscript::format_source(file, source);
    let accepted = !diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == rsscript::Severity::Error);
    if accepted {
        let _ = rsscript::lower_source_to_rust(file, source);
    }
    accepted
}

/// Whether the checker accepts `source` (no error-severity diagnostics). Used by
/// the generative drivers to assert the generator's accept rate stays high.
pub fn checker_accepts(file: &str, source: &str) -> bool {
    !rsscript::analyze_source(file, source)
        .iter()
        .any(|diagnostic| diagnostic.severity == rsscript::Severity::Error)
}
