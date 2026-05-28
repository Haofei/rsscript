# RSScript Rust Front End

This is a Rust implementation of an RSScript v0.4.1 review-first front-end MVP.

RSScript is designed for AI-generated code that humans still need to review. It
makes mutation, retention, resource lifetime, and local-performance boundaries
visible in source code and machine-readable diagnostics.

The current project is a checker prototype. It is intentionally focused on parsing
and reporting review-critical semantic diagnostics, not executing RSScript programs.

It currently implements:

- `rss check <file.rss>` with human diagnostics
- `rss check --json <file.rss>` with serde-backed machine-readable diagnostics
- `rss check --explain <code>` for diagnostic code explanations
- `rss fmt <file.rss>` as a parse/check gate that prints the source unchanged when valid
- `rss review <old.rss> <new.rss>` for a first-pass API/type/effect/freshness diff with path-aware local/manage boundary summaries
- Lexer, lightweight parser, syntax AST, HIR signature table, and semantic checks for the review-critical v0.4.1 rules
- Builtin signatures for the current fixture stdlib surface, including `Image`, `File`, `Map`, `ResourcePool`, `Json`, `Csv`, database/resource helpers, and cache/config helpers
- HIR type and field tables for class/struct/resource declarations and handle fields
- HIR constructor signatures derived from declared type fields
- HIR call-site facts with resolved builtin, user function, constructor, enum variant, and unknown callees
- HIR body binding facts for parameters, managed lets, local lets, and best-effort initial value types
- HIR field-access facts with resolved base type, field type, and handle status where known
- HIR effect events for `manage`, `take`, and retaining calls
- HIR return facts with initial freshness proof classification
- HIR/local-flow use-after-move facts derived from statement uses and flow entry state
- HIR/local-flow managed-to-local facts derived from local bindings and flow entry state
- Resolved HIR statement/expression trees for function bodies, including typed identifiers, resolved calls, field accesses, and per-expression ownership events
- Per-function HIR body views that group bindings, calls, fields, effects, and returns
- Initial local flow graph nodes with successor edges derived from resolved HIR statements, including branch and loop `break` / `continue` control flow, as the staging point for CFG-backed CleanLocal dataflow
- Initial local flow state propagation for local bindings, managed bindings, scoped `with` resource bindings, `manage` / `take` moves, and retaining calls
- HIR-driven mode checks for local-only features
- HIR-driven call checks for named arguments, data effects, and unknown callees
- HIR-driven body traversal for body semantics and local state updates
- Body checks for managed-to-local, use-after-move, HIR/local-flow `fresh` returns, active resource escape, resolved handle-field `take`, managed closure captures, and resource escape traversal now consume HIR body facts and local flow entry state
- HIR/local-flow retaining API checks that only reject values local at the retaining call site
- Local ownership, use-after-move, `fresh` return, managed closure capture, resource-retain escape, and handle-field `take` checks now index statement uses, binding types, fresh-return issue facts, take-handle facts, closure uses, and move/retain events from resolved HIR body trees
- ResourcePool lease checks that require `ResourcePool.borrow(...)` to be scoped by `with`
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
- ResourcePool borrow lease escape outside `with`
- local capture by managed closures
- `take` of handle fields
- API, type layout, path-aware local/manage boundary, effect, mode, and freshness changes in `rss review`
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

1. Expand local/resource dataflow precision around nested closures, retaining APIs, and typed `ResourcePool<T>` lease propagation.
2. Expand `rss review` boundary diffs from path-aware summaries into structured risk categories.

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
