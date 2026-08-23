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
