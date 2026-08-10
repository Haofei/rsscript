# ADR 0020: Runtime binary operation identity is ABI-owned

## Status

Accepted

## Problem

The register VM used `BinaryOp` through the legacy source-shaped
`rsscript-exec-ir` crate for ordinary runtime arithmetic and comparison. That
made the compatibility IR a mandatory dependency of the execution engine even
where no source-shaped lowering was requested.

## Decision and non-goals

`rsscript-abi-model::BinaryOp` is now the canonical semantic identity for
binary operations. `rsscript-exec-ir` re-exports the same type so existing
legacy lowering callers retain their source compatibility. Syntax and checked
MIR continue to map their own parser/IR operations at their respective
lowering boundaries.

This decision does not define the bytecode-v2 opcode encoding, replace
`MirBinaryOp`, or remove the remaining legacy executable-IR lowering path.
Those steps require separate backend and verifier changes.

## Compatibility and security impact

This is an additive pre-1.0 Rust API relocation with a compatibility
re-export. It changes no artifact bytes, Provider signatures, or execution
semantics. The runtime no longer needs the legacy IR to name a fundamental
arithmetic operation, reducing the dependency surface required before the VM
can become execution-only.

## Evidence

The ABI, executable-IR, compiler lowering, and VM test suites compile against
the shared operation type. Architecture tests continue to require frontend-free
MIR and verifier-owned bytecode loading.
