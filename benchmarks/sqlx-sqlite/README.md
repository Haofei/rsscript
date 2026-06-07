# sqlx + SQLite benchmark

Compares the **same** workload across three execution modes:

- `native-rust` — hand-written Rust calling `rss_sqlx_native` directly (`native/`).
- `release-internal` — the RSScript program lowered to Rust and compiled.
- `vm-internal` — the RSScript program run on the register VM, with the sqlx
  native bindings **dynamically loaded** (a cdylib shim is generated from the
  package interface + `bindings.rssbind.toml`, built, and `dlopen`ed — nothing is
  hard-coded).

The workload (`src/main.rss`) creates a `rows`-row table, then issues `size`
pooled `Sqlx.query_strings` calls that each return all `rows` rows — so every call
also marshals a `rows`-element `List<String>` back across the native bridge. The
app code is ordinary RSS using the importable `rss-sqlx` facade — no
`features: native`.

## Run

```sh
./benchmarks/sqlx-sqlite/run.sh --size 500 --rows 200 --iterations 5 --warmup 1
```

`--size` is queries per timed run; `--rows` is rows returned per query (raise it
to make each query heavier in both SQLite work and result marshaling).

Each mode prints one line:

```
bench sqlx-sqlite mode=native-rust       ... mean_ms=...
bench sqlx-sqlite mode=release-internal  ... mean_ms=...
bench sqlx-sqlite mode=vm-internal       ... mean_ms=...
```

The first VM run also builds the shim cdylib (cached under the temp dir, in
`rss-native-plugins/`); later runs reuse it.

## Reading the numbers

Each iteration does real SQLite work through the connection pool, and that work
is **identical** in all three modes — so SQLite time dominates and sets a common
floor. The interesting quantity is the *delta* above native Rust:

- `release-internal` over `native-rust` ≈ RSS lowering overhead (small).
- `vm-internal` over `release-internal` ≈ register-VM interpretation + the
  NativeValue marshaling across the dynamically loaded native bridge.

In practice the SQLite work per call (pool checkout, prepare, execute, fetch) is
large enough that the three modes land **within run-to-run noise of each other** —
i.e. this workload mostly measures SQLite, not the RSS layer. That is the expected
and honest result for a real-query benchmark.

Indicative local numbers (size=500, rows=200 — 100k rows marshaled per run),
for shape only — not a committed baseline; note how close they are even with
large result sets:

| mode             | min_ms | mean_ms |
|------------------|--------|---------|
| native-rust      | ~110   | ~113    |
| release-internal | ~109   | ~117    |
| vm-internal      | ~114   | ~116    |

To actually isolate RSS/VM execution overhead you need a native call far cheaper
than a SQLite query; with real queries the database floor dominates. Raising
`--size` or `--rows` scales all three together rather than separating them — see
the pure-RSS microbenchmarks in `benchmark/` for VM-vs-compiled execution gaps.

## How the VM runs native code

The register VM can't link arbitrary native crates. For a native-backed package
it generates a small cdylib "shim" that wraps the native crate's typed functions
as `rss-native-abi` `NativeInterpreterFn`s and exposes a registry entry; the host
`dlopen`s it and registers the bindings. See `src/native_plugin/` and the shared
`native-abi/` crate. Host and shim are built with the same toolchain (required,
since they share Rust types across the boundary).
