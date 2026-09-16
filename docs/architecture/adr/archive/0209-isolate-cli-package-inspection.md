# ADR 0209: Isolate CLI package inspection from ordinary execution

- Status: Accepted
- Date: 2026-08-14

The command-line application's normal `execution` feature compiles and runs
artifacts through the SDK project loader and its immutable frontend snapshot.
It must not select the compiler's legacy package/review/persistence closure.

The historical package review and deep directory inspection commands therefore
live behind the explicit `package-inspect` feature. The experimental Rust/AOT
path extends that feature. A normal package `rss check` now compiles the same
captured snapshot used by `rss build` and reports versioned artifact analysis
or compiler diagnostics. This preserves a single normal product path while
keeping compatibility inspection available to tooling that intentionally opts
into it.
