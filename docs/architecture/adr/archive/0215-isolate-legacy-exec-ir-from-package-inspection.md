# ADR 0215: Isolate legacy executable IR from package inspection

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's `package` feature included the source-shaped executable-IR
projection. Commands that only captured, checked, diffed, or inspected package
evidence therefore compiled a legacy execution backend even though no code was
run.

## Decision

`package` now selects package capture, typed analysis, persistence adapters,
and provider-neutral bytecode only. `legacy-exec-ir` is a distinct compiler
compatibility feature that adds the old projection; `execution` and the
experimental AOT route select it transitively. The reviewed SDK project path
and CLI package inspection remain free of this dependency closure.

## Impact

No artifact, Provider, or language behavior changes. The feature split makes
the remaining old IR reachability explicit and reduces the compiler closure for
inspection tooling. Architecture tests require `package` not to enable the old
IR and require the explicit compatibility feature to do so.
