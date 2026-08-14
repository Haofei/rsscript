# ADR 0191: Isolate executable IR from default lowering

- Status: Accepted
- Date: 2026-08-14

## Problem

Although the reviewed compiler emitted bytecode from direct checked-HIR MIR,
`rsscript-lowering` still had an unconditional dependency on
`rsscript-exec-ir` and `CompiledIr` eagerly constructed the source-shaped
projection. This kept the compatibility representation in the normal compiler
closure and made the intended MIR boundary only logical rather than physical.

## Decision and non-goals

`rsscript-lowering` now has an empty default feature set. Its executable-IR
dependency, projection, legacy bridge, exports, and legacy lowering tests are
enabled only by `legacy-exec-ir`. The compiler consumes lowering with default
features disabled; only its explicit `package` compatibility feature enables
the legacy bridge. `CompiledIr` constructs and exposes the executable
projection only in that package compatibility configuration.

This does not delete `rsscript-exec-ir` yet. The old bridge remains available
for the differential corpus and unsupported constructs. It will be removed
only after direct MIR lowering reaches parity.

## Evidence

The no-default lowering crate and compiler bytecode closure compile without
`rsscript-exec-ir`; the explicit legacy feature continues to compile its
bridge and tests. SDK execution and compatibility feature matrices cover both
consumer paths.
