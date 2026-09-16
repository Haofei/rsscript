# Contributing

Changes to syntax, public intrinsics, the Provider ABI, bytecode, execution
reports, or the runner protocol require an ADR (`docs/architecture/adr/`) and
compatibility fixtures. A program the checker accepts must build, verify, and
run; `fixture_build_corpus` enforces it.

Run the supported Core gate before submitting changes:

```bash
cargo fmt --all --check
cargo run --locked -p rsscript-xtask -- validate-ci
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked -p rsscript-sdk --features execution
cargo test --locked -p rsscript-cli --features execution
```

The full local gate, including the fixture corpora and the eval scorer, is in
[docs/development/DEVELOPMENT.md](docs/development/DEVELOPMENT.md).

Provider implementations must use `WireValue`, instance-owned authority, and
the Provider conformance harness. Security-sensitive changes should include
failure-path and cancellation/cleanup tests.
