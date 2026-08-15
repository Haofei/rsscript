# RSScript fuzzing

Coverage-guided ([cargo-fuzz] / libFuzzer) harness for the RSScript front end,
Artifact verifier, binding descriptors, execution reports, and isolated-runner
protocol framing.
This is a **standalone crate**, detached from the parent workspace, so it never
builds during `cargo build --workspace` or stable CI (libFuzzer needs nightly).

## Targets

Two families. The **raw-bytes** targets feed arbitrary input straight into the
front end; the **generative** targets decode the seed into a well-typed program
via `rss-testgen` and then check a semantic contract.

Raw-bytes (front-end robustness):
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

Generative (driven by `rss-testgen`):
- **`differential`** — seed -> well-typed program -> every in-process backend
  must agree. By default this is VM interpreter + tier-0 JIT; with
  `--features native-jit` it also runs native, force-deopt, forced-safepoint,
  deopt-every-safepoint, and OSR backends. Counterpart of
  `tests/generative.rs`'s full N-way (which adds the compiled backend, bounded).
- **`fail_closed`** — seed -> program -> inject one targeted defect -> the checker
  must reject it (with the expected diagnostic) and produce no Rust. Counterpart
  of `tests/fixtures/fail` and `generated_programs_fail_closed_when_mutated`.

## Running

```sh
cargo install cargo-fuzz                 # once
cargo +nightly fuzz run parse_check
cargo +nightly fuzz run format_idempotent
cargo +nightly fuzz run differential
cargo +nightly fuzz run fail_closed
cargo +nightly fuzz run bytecode_artifact
cargo +nightly fuzz run binding_descriptor
cargo +nightly fuzz run execution_report
cargo +nightly fuzz run runner_protocol
```

The scheduled `Runner hardening` workflow runs a bounded `runner_protocol`
smoke independently from the Core release gate. It exercises untrusted framing,
not OS-level isolation; the reference runner remains defence in depth rather
than a universal sandbox.

Native-JIT/deopt/OSR differential smoke:

```sh
cargo +nightly fuzz run differential --features native-jit -- -runs=5000 -max_total_time=60
```

The Makefile wraps the bounded CI-friendly forms:

```sh
make fuzz-no-panic
make fuzz-jit-differential
make sanitize-jit-boundary
```

Those Makefile targets run through the Docker dev container. On the first run
they install the required nightly toolchain (and `cargo-fuzz` for fuzz targets)
into Docker's cargo/rustup volumes, so later runs reuse the toolchain cache.

Hardening scope:
- `make miri` runs Miri over the pure `rss-testgen` library subset. Miri cannot
  execute Cranelift-generated machine code or real FFI/syscall boundaries.
- `make fuzz-no-panic` runs the raw-byte robustness fuzzer over the front-end
  parse/check/lower pipeline and is part of the scheduled JIT hardening sweep so
  parser/checker panics do not mask JIT differential signal.
- `make fuzz-jit-differential` runs the structured differential fuzzer with the
  native backend set enabled: native, force-deopt, forced safepoints,
  deopt-every-safepoint, and OSR.
- `make sanitize-jit-boundary` runs a bounded AddressSanitizer smoke over
  selected native/JIT boundary tests: compiled native calls, RSScript child-frame
  deopt resume, deopt-every child-frame resume, closure-id guards, and direct
  flat-list access. ASan does not prove JIT code memory safety by itself, but it
  catches host-side raw-pointer and FFI boundary mistakes that Miri cannot cover.

Replay / triage a generative crash deterministically (the seed reproduces both
the program and, for `fail_closed`, the mutation):

```sh
cargo run -p rss-testgen --bin rss-testgen -- "$(xxd -p artifacts/differential/crash-<hash>)"
```

Seed a corpus from the example scripts and fixtures for a faster start:

```sh
for t in parse_check format_idempotent differential fail_closed; do
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
`bytecode_v2` fuzzes the typed numeric v2 executable payload decoder. It starts
from the checked-in canonical seed in `corpus/bytecode_v2/` and asserts every
accepted payload round-trips byte-for-byte:

```text
cargo +nightly fuzz run bytecode_v2
```
