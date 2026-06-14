# RSScript fuzzing

Coverage-guided ([cargo-fuzz] / libFuzzer) harness for the RSScript front end.
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

Generative (driven by `rss-testgen`):
- **`differential`** — seed -> well-typed program -> every in-process backend
  (VM / JIT / native) must agree. Counterpart of `tests/generative.rs`'s full
  N-way (which adds the compiled backend, bounded).
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
```

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
  cp ../crates/rsscript/tests/fixtures/pass/*.rss corpus/$t/ 2>/dev/null || true
done
```

A crash writes a reproducer under `artifacts/<target>/`; replay it with:

```sh
cargo +nightly fuzz run <target> artifacts/<target>/crash-<hash>
```

[cargo-fuzz]: https://github.com/rust-fuzz/cargo-fuzz
