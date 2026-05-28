# RSScript Rust Front End

This is a Rust implementation of an RSScript v0.4.1 review-first front-end MVP.

The current project is a checker prototype. It is intentionally focused on parsing
and reporting review-critical semantic diagnostics, not executing RSScript programs.

It currently implements:

- `rss check <file.rss>` with human diagnostics
- `rss check --json <file.rss>` with serde-backed machine-readable diagnostics
- `rss fmt <file.rss>` as a parse/check gate that prints the source unchanged when valid
- Lexer, lightweight parser, syntax AST, HIR signature table, and semantic checks for the review-critical v0.4.1 rules
- Builtin signatures for the current fixture stdlib surface, including `Image`, `File`, `Map`, `ResourcePool`, `Json`, `Csv`, and cache/config helpers
- Fixture-based pass/fail scenario tests under `tests/fixtures`

Implemented diagnostic classes include:

- file mode violations
- missing named arguments
- missing `read` / `mut` / `take` call-site effects for known signatures
- managed-to-local attempts
- use after `manage`
- retaining local values
- fresh functions returning managed values
- resource fields and resource escape from `with`
- local capture by managed closures
- `take` of handle fields
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

1. Migrate remaining token-scanning body analysis onto the syntax AST.
2. Expand HIR from function signatures into resolved statements, expressions, fields, and type kinds.
3. Split semantic checks into focused modules for calls, local state, freshness, resources, handles, and forbidden features.
4. Add CleanLocal dataflow for `fresh`, `manage`, `take`, retaining APIs, and closure capture.
5. Add `rss review old.rss new.rss` for API/effect/freshness diffing.

Run:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo run --bin rss -- check path/to/file.rss
cargo run --bin rss -- check --json path/to/file.rss
```
