# ADR 0145: Checked HIR is the default MIR source

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompiledIr::mir` tried checked-HIR lowering and silently retried the
source-shaped executable-IR bridge after *any* error. An invalid direct-MIR
lowering could consequently be hidden by the legacy path, and callers could
not distinguish the intended default backend boundary from compatibility.

## Decision and non-goals

`CompiledIr::mir` now exposes only direct checked-HIR to CFG MIR lowering. The
SDK compatibility adapter invokes the legacy executable-IR encoder only for
`MirLoweringError::Unsupported`; an invalid MIR result is returned as an error.
Legacy accessors are explicitly named and hidden from normal API documentation.

This does not delete executable IR or claim that every supported source form
has reached MIR. Unsupported forms retain their compatibility path until
differential corpus coverage permits removal.

## Compatibility and migration

Existing deprecated `executable` accessors remain temporarily. Reviewed SDK
builds retain behavior for unsupported forms, while direct-MIR consumers now
receive an explicit error instead of a hidden fallback. Artifact, Provider,
and language contracts do not change.

## Verifier and security impact

Direct MIR continues through independent codegen and bytecode verification.
The change narrows fallback eligibility and cannot make invalid direct MIR
enter the VM.

## Provider and backend impact

The reference VM compatibility adapter remains the only legacy executable-IR
consumer. AOT/JIT integrations must use direct MIR or make an equally explicit
compatibility choice.

## Evidence

Compiler and SDK migration tests assert direct-MIR behavior, while architecture
tests require explicit unsupported-only fallback and reject an implicit
`CompiledIr::mir` bridge.
