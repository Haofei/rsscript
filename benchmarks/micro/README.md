# RSScript execution benchmark matrix

This directory contains RSScript programs for tracking the register VM
(`rss bench --mode vm-internal --vm reg`) against the generated Rust backend
(`rss bench --mode release-internal`). The default matrix is tuned toward VM
planning: it uses collection, closure, and lookup-heavy programs that expose
the execution gap between the VM and generated release code.

Run the matrix from the repository root:

```sh
./benchmarks/micro/run-matrix.sh
```

The script runs the benchmark driver itself with `cargo run --release --bin rss`
so eval and VM timings measure optimized interpreter/VM code rather than debug
build overhead.

Optional controls:

```sh
./benchmarks/micro/run-matrix.sh --iterations 5 --warmup 1
```

The matrix prints one row per benchmark with mean milliseconds for register VM
and release Rust, plus the `reg/rust` ratio. If the register VM does not support
a benchmark yet, the row reports `unsupported` instead of hiding the coverage
gap. The release-internal mode builds each generated package once before
timing, starts the binary once, and measures a loop inside the generated release
binary. Build time and process startup are not included in the reported release
milliseconds. VM-internal compiles the source to a VM executable once and
measures only repeated VM execution. Eval and VM total modes remain available as
individual probes.
Lower-gap baseline programs such as
`pure_loop_sum.rss`, `function_call_hot_loop.rss`, `json_parse_access.rss`,
`option_result_chain.rss`, `match_option_loop.rss`, and `sorted_map_scan.rss`
are kept in this directory as additional probes but are intentionally not in the
default matrix because their current interpreter/release ratios are lower or
less stable for VM comparison.

## VM feature benchmark rule

Every major VM feature addition must add or update benchmark coverage in this
directory. A major VM feature is any new HIR control-flow surface, value family,
closure/call mechanism, collection implementation, host boundary, or runtime
intrinsic group that can materially affect VM throughput or allocation behavior.

The minimum expectation is a focused `.rss` benchmark plus inclusion in
`run-matrix.sh` when the feature is part of the default VM performance story.
Feature work can still start with a smaller probe, but it should not be treated
as complete without a benchmark that tracks register VM and `release-internal`.

The stack VM comparison gate was used during the register VM migration. The
public benchmark CLI now treats `vm` and `vm-internal` as register VM modes.
