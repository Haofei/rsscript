# ADR 0151: Compiler owns checked-MIR Artifact emission

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed SDK accepted an immutable `FrontendInputSnapshot`, but then used
its VM compatibility adapter to lower `CompiledIr`, emit bytecode, and build a
provider-neutral Artifact. That made the normal compile path appear to need
the VM adapter even though `rsscript-codegen-vm` is already independent of the
interpreter.

## Decision

`rsscript-compiler` gains an explicit `bytecode` feature. It consumes checked
HIR through the owned MIR boundary and uses `rsscript-codegen-vm` to emit a
`BytecodeArtifact`; it never depends on `rsscript-vm`. The SDK's reviewed
snapshot and project build paths call this compiler boundary, then construct an
Artifact Bundle. VM construction remains exclusively in the verifier/runtime
phase.

The legacy source-shaped executable-IR encoder remains in the SDK VM adapter,
but that module is compiled only by the explicit `compatibility` feature. A
checked-MIR operation that is not yet supported by code generation fails closed
on the reviewed path rather than silently constructing a legacy VM executable.

## Consequences

Normal SDK execution no longer selects `rsscript-codegen-vm`, lowering, or MIR
as direct SDK dependencies. They are compiler implementation details. The
compiler's default and `language-service` closures remain frontend-only; only
the opt-in `bytecode` feature selects the bytecode/codegen closure.

This does not complete the MIR migration or delete legacy execution support.
Those remain conditioned on the differential corpus and the explicit
compatibility feature.

## Evidence

Feature-closure architecture tests require reviewed SDK execution to select
`rsscript-compiler/bytecode` rather than the codegen crates directly, require
the compiler bytecode feature to be VM-free, and reject VM adapter calls from
reviewed build methods. Compiler, SDK execution/project, and compatibility
migration tests exercise the boundary.
