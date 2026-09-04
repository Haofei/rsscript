# Canonical native-JIT baselines

Ad-hoc benchmark snapshots belong in CI artifacts, not source control. A file is
accepted here only when produced on controlled hardware and named
`canonical-<os>-<arch>.json`.

No controlled-hardware canonical baseline is checked in yet. Until controlled
runners are provisioned, the release smoke enforces the scalar speedup and the
weekly scorecard publishes diagnostic evidence without treating workstation
timings as a product contract.

`local-linux-aarch64.json` is a **local-first** baseline collected in the dev
container per the maintainer's decision, not a controlled-hardware run. Its
machine contract says `evidence_class: "local-diagnostic"` and
`controlled: false`; the provenance fields also retain `cpu: "unknown"`,
`cpu_affinity: "none"`, and `cpu_governor: "unavailable-macos-host"`. The
controlled comparator rejects this class. It backs the `jit-cranelift-engine`
retention decision only on that explicit local-first basis; a controlled-hardware
baseline remains follow-up work.

Once the first controlled release exists, every later controlled run downloads
the latest immutable `jit-baseline-*` release and runs
`tools/compare-jit-baseline.py`. Runtime, compile-time, code-size, semantic,
native-entry, bail, and previously accepted retention regressions fail the run.

Collection is fail-closed and requires an explicit controlled-runner assertion:

```sh
python3 tools/collect-jit-baseline.py \
  --controlled --cpu-affinity 2 --cpu-governor performance \
  --samples 25 --warmup 3 \
  --output benchmarks/vm-jit/baseline/canonical-linux-x86_64.json
```

Local diagnostics use `--local` instead and cannot be accepted by the controlled
baseline comparator.

The collector records the exact commit, CPU, OS/architecture, Rust and Cranelift
versions, fixture digest, affinity/governor, alternating sample order, compile and
resident-code telemetry, normal yields and native/OSR/continuation entries. It
refuses fewer than 20 measured samples. Developer-machine scorecards remain CI
artifacts and must not be copied into this directory as canonical evidence.

`cold_e2e_native_ns` includes VM/JIT initialization, translation, compilation,
and execution. Canonical records retain every alternating interpreter/native
sample plus the median absolute deviation (`*_samples_ns` and `*_mad_ns`), so
controlled evidence remains auditable instead of collapsing to one median.
`warm_native_instrumented_ns` is gathered in a separate diagnostic execution
after code publication, and `instrumented_native_nanos_per_entry` divides that
generated-code time by successful native/OSR/continuation entries. It excludes
Cranelift compile time but includes timing instrumentation; it is not evidence of
a cross-evaluation code cache.

Compilation evidence is split into `translation_nanos`, `validation_nanos`,
`codegen_nanos`, and `finalize_nanos`. `compile_nanos` remains the outer admitted
compile wall clock, so it may be larger than the phase sum because it also includes
VM orchestration and cache publication. The comparator checks every phase instead
of hiding a regression inside the total.

Every measured case also carries helper/BCE/LICM evidence, semantic parity, and
the controlled 15% retention-threshold verdict. Scalar-unroll and SIMD candidate
analysis is test-only until a real transform exists, so it does not expand the
production telemetry or baseline schema.
