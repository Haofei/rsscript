# RSScript fuzzing

Coverage-guided ([cargo-fuzz] / libFuzzer) harness for the RSScript front end.
This is a **standalone crate**, detached from the parent workspace, so it never
builds during `cargo build --workspace` or stable CI (libFuzzer needs nightly).

## Targets

- **`parse_check`** — `bytes -> lexer -> parser -> checker -> lowerer` must never
  panic, hang, or stack-overflow. Lowering is only attempted on programs the
  checker accepts (the "RSScript owns semantics, Rust is only a backend"
  contract). The coverage-guided counterpart of `tests/hostile.rs`.
- **`format_idempotent`** — `format(format(x)) == format(x)` for any program the
  checker accepts. The coverage-guided counterpart of the `reformat` transform in
  `tests/metamorphic.rs`.

## Running

```sh
cargo install cargo-fuzz                 # once
cargo +nightly fuzz run parse_check
cargo +nightly fuzz run format_idempotent
```

Seed the corpus from the example scripts and fixtures for a faster start:

```sh
mkdir -p corpus/parse_check
cp ../examples/scripts/**/*.rss corpus/parse_check/ 2>/dev/null || true
cp ../crates/rsscript/tests/fixtures/pass/*.rss corpus/parse_check/ 2>/dev/null || true
```

A crash writes a reproducer under `artifacts/<target>/`; replay it with:

```sh
cargo +nightly fuzz run parse_check artifacts/parse_check/crash-<hash>
```

[cargo-fuzz]: https://github.com/rust-fuzz/cargo-fuzz
