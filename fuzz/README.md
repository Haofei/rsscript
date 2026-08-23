# RSScript fuzzing

Coverage-guided ([cargo-fuzz] / libFuzzer) harness for the RSScript front end,
Artifact verifier, binding descriptors, execution reports, isolated-runner
protocol framing, and the structured native-JIT IR validation boundary.
This is a **standalone crate**, detached from the parent workspace, so it never
builds during `cargo build --workspace` or stable CI (libFuzzer needs nightly).

## Targets

The targets feed arbitrary bytes directly into untrusted parsing and wire
boundaries:
- **`parse_check`** — `bytes -> lexer -> parser -> checker -> lowerer` must never
  panic, hang, or stack-overflow. Lowering is only attempted on accepted programs
  (the "RSScript owns semantics, Rust is only a backend" contract). Coverage-guided
  counterpart of `tests/hostile.rs`.
- **`format_idempotent`** — `format(format(x)) == format(x)` for any accepted
  program. Counterpart of the `reformat` transform in `tests/metamorphic.rs`.
- **`bytecode_artifact`** — hostile bytes must be rejected cleanly by the
  section decoder and independent verifier; accepted Artifacts must round-trip
  to exactly the same canonical bytes.
- **`binding_descriptor`** — arbitrary UTF-8/TOML is projected through the
  strict `rsscript.bindings.v1` schema without panicking.
- **`execution_report`** — arbitrary JSON is checked against the strict
  `rsscript.execution_report.v1` consumer contract without panicking.
- **`runner_protocol`** — hostile request/response bytes exercise bounded
  isolated-runner framing and canonical round-trips without panicking.
- **`jit_ir_validate`** — bounded structured JIT functions must either produce a
  clean validation rejection or continue through a small hard-bounded Cranelift
  codegen/finalization probe; malformed register and control-flow combinations
  must never panic. Machine code is never invoked. It is behind `native-jit` so
  normal wire fuzzing does not compile Cranelift.

## Running

```sh
cargo install cargo-fuzz                 # once
cargo +nightly fuzz run parse_check
cargo +nightly fuzz run format_idempotent
cargo +nightly fuzz run bytecode_artifact
cargo +nightly fuzz run binding_descriptor
cargo +nightly fuzz run execution_report
cargo +nightly fuzz run runner_protocol
cargo +nightly fuzz run jit_ir_validate --features native-jit
```

The scheduled `Runner hardening` workflow runs a bounded `runner_protocol`
smoke independently from the Core release gate. It exercises untrusted framing,
not OS-level isolation; the reference runner remains defence in depth rather
than a universal sandbox.

The Makefile wraps bounded CI-friendly forms:

```sh
make fuzz-no-panic
make sanitize-jit-boundary
```

Those Makefile targets run through the Docker dev container. On the first run
they install the required nightly toolchain (and `cargo-fuzz` for fuzz targets)
into Docker's cargo/rustup volumes, so later runs reuse the toolchain cache.

Hardening scope:
- `make fuzz-no-panic` runs the raw-byte robustness fuzzer over the front-end
  parse/check/lower pipeline and is part of the scheduled JIT hardening sweep so
  parser/checker panics do not mask JIT differential signal.
- `make sanitize-jit-boundary` runs a bounded AddressSanitizer smoke over
  selected native/JIT boundary tests: compiled native calls, RSScript child-frame
  deopt resume, deopt-every child-frame resume, closure-id guards, and direct
  flat-list access. ASan does not prove JIT code memory safety by itself, but it
  catches host-side raw-pointer and FFI boundary mistakes that Miri cannot cover.

Seed a corpus from the example scripts and fixtures for a faster start:

```sh
for t in parse_check format_idempotent; do
  mkdir -p corpus/$t
  cp ../examples/scripts/**/*.rss corpus/$t/ 2>/dev/null || true
  cp ../crates/rsscript-compiler/tests/fixtures/pass/*.rss corpus/$t/ 2>/dev/null || true
done
```

A crash writes a reproducer under `artifacts/<target>/`; replay it with:

```sh
cargo +nightly fuzz run <target> artifacts/<target>/crash-<hash>
```

[cargo-fuzz]: https://github.com/rust-fuzz/cargo-fuzz

Native JIT parity is covered by the phase-typed
`native_jit_differential` integration target and the `vm-jit` validation,
deopt, OSR, allocation, and mutated-IR tests. It no longer depends on the
retired dynamic SDK compatibility harness. The scheduled JIT hardening workflow
also runs `jit_ir_validate` with coverage feedback.
