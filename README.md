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
- HIR effect events for `manage`, `take`, and retaining calls
- HIR return facts with initial freshness proof classification
- Resolved HIR statement/expression trees for function bodies, including typed identifiers, resolved calls, field accesses, and per-expression ownership events
- Per-function HIR body views that group bindings, calls, fields, effects, and returns
- Initial local flow steps derived from resolved HIR statements as the staging point for CFG-backed CleanLocal dataflow
- AST-driven mode and call checks for local-only features, named arguments, data effects, and retaining APIs
- AST-driven body checks for local moves, early-exit-aware `fresh` returns, resource escape, resolved handle-field `take`, and managed closure captures
- Local ownership, use-after-move, `fresh` return, managed closure capture, resource-retain escape, and handle-field `take` checks now index statement uses, binding types, return proofs, field accesses, closure uses, and move/retain events from resolved HIR body trees
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

1. Continue moving body checkers from syntax AST plus lookup tables onto the resolved HIR statement/expression tree.
2. Extend initial local flow steps into a dedicated CFG with successor edges for CleanLocal dataflow.
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
