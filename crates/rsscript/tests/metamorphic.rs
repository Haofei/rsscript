//! Metamorphic differential tests.
//!
//! Backend differential testing ([`backend_differential`]) needs an oracle: it
//! generates a program, computes the expected value, and checks the backends.
//! Metamorphic testing needs no oracle. It takes a *known-valid* program and
//! applies a transform that is supposed to preserve meaning, then asserts the
//! transformed program still:
//!
//!   1. type-checks and runs on **every** backend (VM / JIT / native / compiled),
//!   2. produces the **same** observable output as the original, and
//!   3. (for structure-preserving transforms) reports the **same** review
//!      classification.
//!
//! Any divergence is a concrete bug in some pass that has nothing to do with the
//! program's value being "right" — e.g. the formatter altering semantics, a
//! span/offset bug surfacing only after reformatting, declaration-order
//! sensitivity, or an unused binding perturbing a backend. These are exactly the
//! mismatches static review keeps circling without pinning down.

mod common;

use common::differential::backends_agreed_stdout;
use rsscript::{format_source, review_map_sources};

/// A semantics-preserving source-to-source rewrite. `apply` returns `None` when
/// the transform does not match the program's shape, so a transform can pattern
/// match defensively instead of producing an invalid program.
struct Transform {
    name: &'static str,
    /// `true` when the transform leaves the set of functions (and their bodies'
    /// review-relevant content) unchanged, so the review classification must be
    /// byte-for-byte identical. `false` for transforms that add a function or
    /// otherwise legitimately shift the review projection.
    preserves_review: bool,
    apply: fn(&str) -> Option<String>,
}

/// Idempotent reformat: `format_source` must never change behavior. This is the
/// highest-value transform — it is always applicable, always produces a valid
/// program, and exercises the formatter against the full execution pipeline.
fn reformat(source: &str) -> Option<String> {
    Some(format_source("metamorphic.rss", source))
}

/// Appending an uncalled function must not change any other function's behavior
/// or codegen. Adds one function, so the review projection legitimately grows.
fn add_dead_function(source: &str) -> Option<String> {
    Some(format!(
        "{source}\nfn mm_unused_dead_helper(value: Int) -> Int {{\n    \
         return value * value + 1\n}}\n"
    ))
}

/// Comments and blank lines are pure trivia: stripping them through the lexer and
/// re-parsing must not move spans in a way that changes behavior or the function
/// count. Inserted before every top-level `fn`.
fn inject_trivia(source: &str) -> Option<String> {
    if !source.contains("\nfn ") && !source.starts_with("fn ") {
        return None;
    }
    let mut out = String::from("// metamorphic: leading trivia\n\n");
    for line in source.lines() {
        if line.starts_with("fn ") {
            out.push_str("// metamorphic: per-function trivia\n");
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// An unused local in `main` must not change `main`'s output. Only applies to the
/// canonical `fn main() -> Unit {` header used by these fixtures.
fn add_unused_local(source: &str) -> Option<String> {
    let header = "fn main() -> Unit {\n";
    let idx = source.find(header)?;
    let insert_at = idx + header.len();
    let mut out = String::with_capacity(source.len() + 32);
    out.push_str(&source[..insert_at]);
    out.push_str("    let mm_unused_local = 7 * 6\n");
    out.push_str(&source[insert_at..]);
    Some(out)
}

/// Move the last top-level function ahead of the first. Top-level declarations
/// are order-independent, so this must not change anything. Applies only when
/// there are at least two top-level functions.
fn reorder_top_level_functions(source: &str) -> Option<String> {
    let blocks = split_top_level_functions(source)?;
    if blocks.len() < 2 {
        return None;
    }
    let mut reordered = Vec::with_capacity(blocks.len());
    reordered.push(blocks[blocks.len() - 1].clone());
    reordered.extend(blocks[..blocks.len() - 1].iter().cloned());
    Some(reordered.join("\n"))
}

/// Split a source file into top-level `fn ... { ... }` blocks (brace-balanced).
/// Returns `None` if the file has any non-function top-level content, so the
/// reorder transform stays conservative.
fn split_top_level_functions(source: &str) -> Option<Vec<String>> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut i = 0;
    // Skip leading whitespace.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() {
        if !source[i..].starts_with("fn ") {
            return None;
        }
        // Find the opening brace of this function.
        let open = source[i..].find('{')? + i;
        let mut depth = 0;
        let mut j = open;
        let close = loop {
            match bytes.get(j)? {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break j;
                    }
                }
                _ => {}
            }
            j += 1;
        };
        blocks.push(source[i..=close].to_string());
        i = close + 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    (!blocks.is_empty()).then_some(blocks)
}

fn transforms() -> Vec<Transform> {
    vec![
        Transform {
            name: "reformat",
            preserves_review: true,
            apply: reformat,
        },
        Transform {
            name: "inject-trivia",
            preserves_review: true,
            apply: inject_trivia,
        },
        Transform {
            name: "add-unused-local",
            preserves_review: true,
            apply: add_unused_local,
        },
        Transform {
            name: "reorder-top-level-functions",
            preserves_review: true,
            apply: reorder_top_level_functions,
        },
        Transform {
            name: "add-dead-function",
            preserves_review: false,
            apply: add_dead_function,
        },
    ]
}

/// Known-valid base programs spanning the executable subset. Each is proven to
/// run identically on every backend (the shapes mirror `backend_differential`).
fn base_programs() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "mm-arithmetic.rss",
            "\
fn main() -> Unit {
    let a = 6
    let b = 7
    Log.write(message: read String.from_int(value: a * b + 0))
    return Unit
}
",
        ),
        (
            "mm-branches.rss",
            "\
fn classify(n: Int) -> Int {
    if n < 10 {
        return 0
    }
    if n <= 100 {
        return 1
    }
    return 2
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: classify(n: read 3)))
    Log.write(message: read String.from_int(value: classify(n: read 42)))
    Log.write(message: read String.from_int(value: classify(n: read 999)))
    return Unit
}
",
        ),
        (
            "mm-loop.rss",
            "\
fn sum_to(n: Int) -> Int {
    let mut total = 0
    let mut i = 1
    while i <= n {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: sum_to(n: read 10)))
    return Unit
}
",
        ),
        (
            "mm-multi-helper.rss",
            "\
fn combine(a: Int, b: Int, c: Int) -> Int {
    let scaled = a * 3 - b
    return scaled + c * 2
}

fn twice(n: Int) -> Int {
    return n + n
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: combine(a: read 7, b: read 4, c: read 5)))
    Log.write(message: read String.from_int(value: twice(n: read 21)))
    return Unit
}
",
        ),
    ]
}

/// Position-independent review projection: function-level classification counts.
/// Line-count fields are intentionally excluded because trivia/unused-local
/// transforms legitimately shift them while leaving the classification intact.
fn review_projection(source: &str) -> (usize, usize, usize, usize) {
    let map = review_map_sources(vec![("metamorphic.rss", source)]);
    let summary = &map.summary;
    (
        summary.total_functions,
        summary.review_required.functions,
        summary.foldable.functions,
        summary.unknown.functions,
    )
}

#[test]
fn metamorphic_variants_preserve_behavior_and_review_facts() {
    let transforms = transforms();
    for (name, base) in base_programs() {
        let base_output = backends_agreed_stdout(name, base, &[]);
        let base_review = review_projection(base);

        let mut applied = 0;
        for transform in &transforms {
            let Some(variant) = (transform.apply)(base) else {
                continue;
            };
            applied += 1;

            let variant_output = backends_agreed_stdout(name, &variant, &[]);
            assert_eq!(
                variant_output, base_output,
                "transform `{}` changed the output of {name}\n--- base ---\n{base}\n\
                 --- variant ---\n{variant}",
                transform.name
            );

            if transform.preserves_review {
                assert_eq!(
                    review_projection(&variant),
                    base_review,
                    "transform `{}` changed review classification of {name}\n\
                     --- variant ---\n{variant}",
                    transform.name
                );
            }
        }

        assert!(applied > 0, "no metamorphic transform applied to {name}");
    }
}
