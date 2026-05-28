# RSScript Rust Front End

This is a Rust implementation of an RSScript v0.4.1 front-end MVP.

It currently implements:

- `rss check <file.rss>` with human diagnostics
- `rss check --json <file.rss>` with machine-readable diagnostics
- `rss fmt <file.rss>` as a parse/check gate that prints the source unchanged when valid
- Lexer, lightweight parser, AST indexing, and semantic checks for the review-critical v0.4.1 rules

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

Run:

```sh
cargo test
cargo run --bin rss -- check path/to/file.rss
cargo run --bin rss -- check --json path/to/file.rss
```
