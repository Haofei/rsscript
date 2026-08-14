# ADR 0143: Rust/AOT lowering is an explicit compiler feature

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's broad `execution` feature compiled the Rust source lowerer even
when a caller only needed package execution compatibility, provider-neutral
IR, or source symbol inventory. That made an experimental backend part of the
ordinary execution dependency and API path.

## Decision and non-goals

Rust source emission is selected by `rsscript-compiler/aot-rust`, which extends
but is not selected by `execution`. The SDK mirrors that distinction with an
explicit `aot-rust` feature; its legacy `compatibility` feature opts in so the
migration corpus retains its current APIs. The CLI's execution composition also
opts in because its legacy `--aot` workflow still exposes Rust emission.

Backend symbol-name projection moves to `lower_names`, an execution-neutral
module used by both symbol inventory and AOT lowering. This decision does not
move the lowerer to a separate experimental workspace or remove its legacy
public compatibility façade.

## Compatibility and migration

Existing compatibility and CLI builds retain generated-Rust APIs. New SDK
embedders using only `execution` do not compile or receive those APIs unless
they explicitly select `aot-rust`. No language, Artifact, bytecode, Provider,
or persisted package contract changes.

## Verifier and security impact

None. The reference VM and bytecode verifier are unchanged. Reducing the
ordinary execution closure does not create an isolation boundary or alter
Provider authority.

## Provider and backend impact

The Rust/AOT backend consumes the shared name projection and package-native
dependency model. Providers and the VM remain independent of the AOT feature.

## Evidence

Architecture tests assert that ordinary execution does not select `aot-rust`,
that the lowerer is feature-gated, and that symbol inventory does not reference
the lowerer. Compiler, SDK, and CLI feature-matrix checks compile all supported
feature selections.
