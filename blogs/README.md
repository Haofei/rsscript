# RSScript Blog Series

This series is an argument in three acts: why AI-generated systems code is hard to review, what a review-first language should look like, and what happens when the language has to write real programs before it has an ecosystem.

## Start Here

- New to the argument: start with [01 — I reviewed 100,000 lines of AI-generated Rust](01-100k-lines-of-ai-rust.md).
- Interested in language design: start with [05 — Explicit is a budget, not a virtue](05-explicit-is-a-budget.md).
- Interested in language tradeoffs: start with [08 — The feature I didn't build](08-the-feature-i-didnt-build.md).

## Act I: The Problem

1. [I reviewed 100,000 lines of AI-generated Rust](01-100k-lines-of-ai-rust.md) — the concrete review pain.
2. [Why AI writes Rust like library code](02-ai-writes-library-code.md) — the structural reason generated code over-abstracts.
3. [Less is more](03-less-is-more-20-80-rust.md) — the first sketch of a smaller Rust-shaped surface.
4. [Why no one built this before](04-why-no-one-built-this-before.md) — prior art and timing.

## Act II: The Language

5. [Explicit is a budget, not a virtue](05-explicit-is-a-budget.md) — where ceremony belongs and where it does not.
6. [How do you teach a language that doesn't exist yet?](06-teaching-a-language-that-doesnt-exist.md) — zero training data and the toolchain layer.

## Act III: The Workbench

7. [The feature I didn't build](08-the-feature-i-didnt-build.md) — why eager pipeline beat generator machinery.
8. [Patterns are just places](09-patterns-are-just-places.md) — pattern matching as existing ownership rules applied to projections.
9. [When you are the entire ecosystem](10-when-you-are-the-entire-ecosystem.md) — why self-checking docs, tests, and tools matter when nobody else is there yet.
