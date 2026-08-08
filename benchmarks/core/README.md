# Core product metrics

This suite measures the supported compiler → verified Artifact → bounded VM
path. It is intentionally separate from JIT microbenchmarks.

Run the same release-mode command used by CI:

```sh
cargo run --locked --release -p rsscript-xtask -- \
  core-metrics \
  --check benchmarks/core/slo.v1.json \
  --output target/core-metrics.json
```

The report contains p50, p95, and maximum latency for source checking,
compilation, Artifact verification, VM execution, a 1,000-call Provider
workload, and fail-closed rejection of an already-cancelled execution. It also
records Artifact size, deterministic VM work counters, Provider request and
response bytes, and Provider-internal total/maximum call duration. Comparing
the Provider workload's end-to-end duration with its internal duration makes
boundary/VM regressions distinguishable from Provider implementation time. The
checked-in SLO is a regression ceiling for comparable CI runners, not a claim
that every host has identical wall-clock performance.

Change `slo.v1.json` only with a report from the same workload and an explicit
explanation in the commit. Product workload measurements belong in this
directory; experimental JIT results remain under `benchmarks/vm-jit`.
