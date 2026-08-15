# ADR 0143: Rust/AOT lowering is an explicit experimental-backend feature

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's broad `execution` feature compiled the Rust source lowerer even
when a caller only needed package execution compatibility, provider-neutral
IR, or source symbol inventory. That made an experimental backend part of the
ordinary execution dependency and API path.

## Decision and non-goals

Rust source emission is selected only by `rsscript-aot-backend`, an excluded
member of the separate `experiments/` workspace. The compiler has no
`aot-rust` feature and owns no generated-Rust implementation modules. The SDK
and CLI mirror that distinction with explicit `aot-rust` features that depend
on the experimental backend directly; the SDK re-export exists only under its
compatibility feature for the migration corpus.

Core keeps the public symbol-inventory projection. The experimental backend
owns Rust-specific name pins, source lowering, source maps, runtime target
mapping, diagnostics, and generated-package publication.

## Compatibility and migration

Existing compatibility and CLI builds retain generated-Rust APIs through the
experimental backend. New SDK embedders using only `execution` do not compile
or receive those APIs unless they explicitly select `aot-rust`. No language,
Artifact, bytecode, Provider, or persisted package contract changes.

## Verifier and security impact

None. The reference VM and bytecode verifier are unchanged. Reducing the
ordinary execution closure does not create an isolation boundary or alter
Provider authority.

## Provider and backend impact

The Rust/AOT backend consumes syntax, semantics, lowering, diagnostics, text,
the interface catalog, and the project-native dependency model directly.
Providers, the VM, and compiler remain independent of the AOT feature.

## Evidence

Architecture tests assert that compiler has no AOT feature or lowerer module,
that the root workspace excludes the backend, and that SDK/CLI opt into the
backend directly. Compiler, SDK, CLI, and backend parity checks compile the
supported feature selections.
