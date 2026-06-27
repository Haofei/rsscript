# RSScript execution benchmark matrix

This directory contains RSScript programs used by Rust benchmark and regression
tests for VM planning. The public `rss` CLI no longer exposes benchmark driver
commands; benchmark execution should live in Cargo tests, criterion-style Rust
harnesses, or Make targets rather than hidden `rss` subcommands.

The default probes are collection, closure, and lookup-heavy programs that
expose the execution gap between the VM and generated release code.
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

The minimum expectation is a focused `.rss` benchmark plus a Rust test or
benchmark harness that exercises it when the feature is part of the default VM
performance story. Feature work can still start with a smaller probe, but it
should not be treated as complete without executable coverage.
