# ADR 0142: Package capture owns native dependency identity

- Status: Accepted
- Date: 2026-08-14

## Problem

`NativeRustDependency` described dependencies discovered from a package
manifest, but was defined by `rust_lower`. This made package capture depend on
the experimental Rust/AOT lowering implementation and incorrectly made the
lowerer the owner of immutable package-graph identity.

## Decision and non-goals

`rsscript-compiler::package` owns `NativeRustDependency`. Native package
loading and `PackageLoweringInput` use that package-owned value. The Rust
lowerer consumes it and retains a compatibility re-export while legacy callers
migrate.

This does not make native dependencies part of the neutral Artifact contract,
does not add a native plugin capability, and does not complete moving Rust/AOT
lowering out of the compiler.

## Compatibility and migration

The Rust type fields and the existing `rust_lower::NativeRustDependency`
import path remain available during the transition. Package snapshots preserve
the same native dependency metadata and ordering. No language, Artifact,
Provider ABI, or persisted bundle schema changes.

## Verifier and security impact

None. This change only corrects in-process ownership of package metadata; it
does not alter untrusted Artifact validation, Provider linkage, or execution
authority.

## Provider and backend impact

Experimental Rust/AOT lowering now explicitly consumes package input instead
of defining it. Provider and VM contracts are unchanged.

## Evidence

The architecture test rejects a package-loader dependency on `rust_lower` and
requires the model to be package-owned. Compiler execution builds and SDK
compatibility/static tests preserve current callers.
