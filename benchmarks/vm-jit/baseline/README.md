# Canonical native-JIT baselines

Ad-hoc benchmark snapshots belong in CI artifacts, not source control. A file is
accepted here only when produced on controlled hardware and named
`canonical-<os>-<arch>.json`.

No canonical timing baseline is checked in yet. Until controlled runners are
provisioned, the release smoke enforces the scalar speedup and the weekly
scorecard publishes diagnostic evidence without treating workstation timings as
a product contract.

Collection is fail-closed and requires an explicit controlled-runner assertion:

```sh
python3 tools/collect-jit-baseline.py \
  --controlled --cpu-affinity 2 --cpu-governor performance \
  --samples 25 --warmup 3 \
  --output benchmarks/vm-jit/baseline/canonical-linux-x86_64.json
```

The collector records the exact commit, CPU, OS/architecture, Rust and Cranelift
versions, fixture digest, affinity/governor, alternating sample order, compile and
resident-code telemetry, normal yields and native/OSR/continuation entries. It
refuses fewer than 20 measured samples. Developer-machine scorecards remain CI
artifacts and must not be copied into this directory as canonical evidence.

`cold_e2e_native_ns` includes VM/JIT initialization, translation, compilation,
and execution. `diagnostic_native_run_nanos` is gathered in a separate
instrumented execution, and `warm_native_entry_avg_nanos` divides that generated-
code time by successful native/OSR/continuation entries; it excludes Cranelift
compile time and must not be compared directly with the uninstrumented cold run.

Every measured case also carries helper/BCE/LICM evidence,
scalar-unroll/SIMD research-candidate counts, semantic parity, and the controlled
15% retention-threshold verdict. A candidate count is not an active optimization
and cannot make that verdict pass by itself.
