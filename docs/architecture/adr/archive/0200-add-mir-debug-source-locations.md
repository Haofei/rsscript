# ADR 0200: Add MIR debug source locations as side-table metadata

- Status: Accepted
- Date: 2026-08-14

## Problem

Typed MIR deliberately removes syntax nodes and source spelling from executable
operations. Without a non-executable source location, later diagnostics and
source-map consumers would need to reconstruct origin information in every
backend or treat names as executable identity.

## Decision

`MirFunctionDebug` now optionally carries `MirSourceLocation`, containing the
origin file, line, column, and range length for direct checked-HIR lowering.
The location is debug side-table data only. It is not used by MIR validation,
instruction semantics, function identity, or bytecode code generation.

## Consequences

Backends retain typed local identities while tooling has a stable origin for
directly lowered functions. Legacy executable-IR compatibility lowering leaves
the optional location absent until that bridge is removed.
