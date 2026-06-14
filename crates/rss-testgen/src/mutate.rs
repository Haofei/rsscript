//! Negative generation: turn a valid program into one the checker must reject.
//!
//! Each mutation appends a single **defective, uncalled** function carrying one
//! targeted error. Appending (rather than editing `main`) keeps the defect in a
//! controlled `-> Unit` context so it is rejected regardless of the base program
//! shape, and leaves the base program intact. The fail-closed contract: the
//! checker reports an error (the "RSScript owns semantics" rule) **and** no Rust
//! is produced — the defect never reaches the backend.
//!
//! This is the generative counterpart of the curated `tests/fixtures/fail`
//! corpus, exercised over arbitrary generated programs via [`crate::seed`].

use crate::generator::GeneratedProgram;
use crate::seed::SeedReader;

/// A targeted defect and the diagnostic class it must trigger.
pub struct Mutation {
    pub name: &'static str,
    /// The RSScript diagnostic code the checker must report.
    pub expected_code: &'static str,
    /// Statements placed in `fn __mm_bad() -> Unit { <body> return Unit }`.
    body: &'static str,
}

/// The mutation catalog. Each is independently verified to produce its code and
/// to block lowering.
pub const MUTATIONS: &[Mutation] = &[
    Mutation {
        name: "type-mismatch",
        expected_code: "RS0207",
        body: "    let mm: Int = \"not an int\"",
    },
    Mutation {
        name: "unknown-binding",
        expected_code: "RS0026",
        body: "    Log.write(message: read mm_undefined_binding)",
    },
    Mutation {
        name: "reassign-immutable",
        expected_code: "RS0311",
        body: "    let mm = 0\n    mm = 1",
    },
    Mutation {
        name: "unhashable-map-key",
        expected_code: "RS0032",
        body: "    let mm = Set.new<Float>()\n    \
               Log.write(message: read String.from_int(value: Set.len(set: read mm)))",
    },
    Mutation {
        name: "try-in-non-result-fn",
        expected_code: "RS0013",
        body: "    let mm = String.from_int(value: 1)?",
    },
];

/// A program mutated to be invalid, with the diagnostic it must produce.
#[derive(Debug, Clone)]
pub struct MutatedProgram {
    pub source: String,
    pub mutation: &'static str,
    pub expected_code: &'static str,
}

/// Apply one seed-selected mutation to `base`.
pub fn mutate(base: &GeneratedProgram, seed: &[u8]) -> MutatedProgram {
    let mut reader = SeedReader::new(seed);
    let mutation = &MUTATIONS[reader.choice(MUTATIONS.len())];
    let source = format!(
        "{}\nfn __mm_bad() -> Unit {{\n{}\n    return Unit\n}}\n",
        base.source, mutation.body
    );
    MutatedProgram {
        source,
        mutation: mutation.name,
        expected_code: mutation.expected_code,
    }
}
