# ADR 0022: Semantic ownership of duplicate-declaration diagnostics

## Status

Accepted

## Problem

The compiler declaration check both read the resolved HIR duplicate-symbol
inventory and reconstructed the user-facing diagnostic. That made a semantic
identity rule depend on compiler orchestration, so non-compiler frontend
consumers could not derive the same result from the same HIR.

## Decision and non-goals

`rsscript-semantics::duplicate_declaration_diagnostics` owns diagnostic
construction for duplicate functions, types, constructors, and fields. It
consumes the resolved HIR inventory, including the original and duplicate
source spans. The compiler remains an adapter that appends these diagnostics
to its aggregate result.

This does not move workspace namespace/import validation or backend-specific
`#lower_name` conflicts. Those require package composition or Rust-lowering
knowledge and remain outside this narrow migration step.

## Compatibility and migration

Diagnostic code, message, cause, fix identifier, and source spans remain
unchanged. This is an internal pre-1.0 ownership move; it changes no language,
Artifact, Provider, or SDK wire contract.

## Verifier and security impact

No untrusted Artifact or runtime behavior changes. Centralizing the resolved
identity diagnostic prevents frontend clients from drifting on duplicate-name
interpretation.

## Provider and backend impact

None. Provider and backend validation continue to consume only validated
frontend results.

## Evidence

The semantics unit test checks the duplicate code and second declaration span.
Compiler regression tests retain existing diagnostic coverage. The SDK
architecture test requires the semantic owner and rejects compiler-local HIR
duplicate inventory interpretation.
