# Contributing

RSScript is in an architecture-convergence phase. Changes to syntax, public
intrinsics, Provider ABI, bytecode, execution reports, or the runner protocol
require an ADR and compatibility fixtures.

Run the supported Core gate before submitting changes:

```bash
cargo fmt --all --check
cargo run --locked -p rsscript-xtask -- validate-ci
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked -p rsscript-sdk --features execution
cargo test --locked -p rsscript-cli --features execution
```

Experimental packages use their own workspace and do not define Core release
health:

```bash
cargo test --locked --manifest-path experiments/Cargo.toml --workspace
```

Provider implementations must use `WireValue`, instance-owned authority, and
the Provider conformance harness. Security-sensitive changes should include
failure-path and cancellation/cleanup tests.
