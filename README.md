# RSScript Rust Front End

This is a Rust implementation of an RSScript v0.4.1 review-first front-end MVP.

The current project is a checker prototype. It is intentionally focused on parsing
and reporting review-critical semantic diagnostics, not executing RSScript programs.

It currently implements:

- `rss check <file.rss>` with human diagnostics
- `rss check --json <file.rss>` with serde-backed machine-readable diagnostics
- `rss check --explain <code>` for diagnostic code explanations
- `rss fmt <file.rss>` as a parse/check gate that prints the source unchanged when valid
- `rss review <old.rss> <new.rss>` for a first-pass API/type/effect/freshness diff
- Lexer, lightweight parser, syntax AST, HIR signature table, and semantic checks for the review-critical v0.4.1 rules
- Builtin signatures for the current fixture stdlib surface, including `Image`, `File`, `Map`, `ResourcePool`, `Json`, `Csv`, database/resource helpers, and cache/config helpers
- HIR type and field tables for class/struct/resource declarations and handle fields
- HIR constructor signatures derived from declared type fields
- HIR call-site facts with resolved builtin, user function, constructor, enum variant, and unknown callees
- HIR body binding facts for parameters, managed lets, local lets, and best-effort initial value types
- HIR field-access facts with resolved base type, field type, and handle status where known
- AST-driven mode and call checks for local-only features, named arguments, data effects, and retaining APIs
- AST-driven body checks for local moves, early-exit-aware `fresh` returns, resource escape, resolved handle-field `take`, and managed closure captures
- Focused check modules for mode, calls, body semantics, and forbidden operator behavior
- Fixture-based pass/fail scenario tests under `tests/fixtures`

Implemented diagnostic classes include:

- file mode violations
- duplicate declarations that would make symbol or field resolution ambiguous
- missing named arguments
- unknown, missing, and duplicate call arguments for known signatures
- unknown callees outside known functions, constructors, enum variants, and builtin signatures
- missing `read` / `mut` / `take` call-site effects for known signatures
- managed-to-local attempts
- use after `manage`
- retaining local values
- fresh functions returning managed values
- branch/loop/early-exit-sensitive `manage`, `take`, and retaining effects on clean local values
- resource fields and resource escape from `with`
- local capture by managed closures
- `take` of handle fields
- API, type layout, local/manage boundary, effect, mode, and freshness changes in `rss review`
- likely operator overload attempts

Non-goals for this stage:

- Runtime execution
- Bytecode or native code generation
- Garbage collection or managed heap implementation
- Full formatter / pretty printer
- Module system
- Full generic type semantics
- Native FFI
- Self-hosting
- Package manager

Near-term roadmap:

1. Expand HIR from declarations into resolved statements and expressions.
2. Extend CleanLocal dataflow from structured statements into a dedicated CFG representation.
3. Expand `rss review` local/manage boundary diffs from summaries into path-aware risk explanations.

Run:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo run --bin rss -- check path/to/file.rss
cargo run --bin rss -- check --json path/to/file.rss
cargo run --bin rss -- check --explain RS0401
cargo run --bin rss -- review old.rss new.rss
```
