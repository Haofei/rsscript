# Differential test corpus

A single `.rss` program + a `.toml` sidecar drives the test framework. One
normal libtest case runs the corpus inside the `differential` target, so the
public test taxonomy stays small.

The guiding principle for a two-backend language: **parity by default**. An
`execution` fixture that lists `compiled` is run on *both* the register VM and the
Rust-lowered backend, and the two outputs must match (plus any declared
expectation). The engine lives in `tests/common/mod.rs` (`run_vm_source`,
`run_compiled_source`, `error_codes`).

## Layout

```
tests/corpus/<area>/<name>.rss     # the program
tests/corpus/<area>/<name>.toml    # what to expect
```

## Sidecar schema

```toml
kind     = "execution" | "diagnostics"

# execution only:
backends = ["vm", "compiled"]   # default ["vm"]; "compiled" adds VM<->compiled parity
args     = ["..."]              # program args (Args.*)
stdout   = "...\n"              # expected stdout (optional)
value    = "42"                 # expected `main` return display, VM only (optional)

# diagnostics only:
codes    = ["RS0206"]          # exact set of error-severity codes the checker reports

# all fixtures:
tags     = ["arithmetic"]      # capability tags; feed the coverage gate (required, non-empty)
```

## Adding a fixture

1. Write the `.rss` program. Use `Log.write(...)` to produce `stdout` so the
   compiled backend can be compared (the VM-only `value` field can't be observed
   from a compiled binary). No statement semicolons, no parenthesized grouping
   (outside the parser surface today).
2. Add the `.toml` sidecar with at least one `tags` entry.
3. `cargo test -p rsscript-engine --test differential differential_corpus::corpus_fixtures_pass -- --exact`.

## Coverage gate

`coverage::required_tags` fails if any capability in `REQUIRED_TAGS`
(`tests/differential_corpus.rs`) has no fixture. When you add a new language/runtime
capability, add a fixture (and, if it's genuinely new, a required tag) so "test
all aspects" stays enforced rather than aspirational.

## Related layers

- `tests/properties.rs` — property-based runtime checks (VM vs a Rust
  oracle) via `proptest`.
- `tests/vm.rs` / `tests/vm_eval.rs` — focused VM/parity unit tests (the
  corpus is the preferred home for new end-to-end execution cases).
- `src/native_plugin/` + `native-abi/` unit tests — the dynamic native bridge.
