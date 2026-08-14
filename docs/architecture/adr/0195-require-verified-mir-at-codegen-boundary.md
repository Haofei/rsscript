# ADR 0195: Require verified MIR at the codegen boundary

- Status: Accepted
- Date: 2026-08-14

## Problem

`MirModule::new` validates its private representation, but the bytecode
codegen API still accepted a raw `MirModule`. That left the lowering-to-backend
admission boundary implicit and made it easy for future codegen entry points to
skip the MIR verifier by convention.

## Decision

`rsscript-mir` provides `VerifiedMir`, an owning phase wrapper constructed only
by re-running `MirModule` verification. The normal checked-HIR lowerer returns
that phase directly, and `rsscript-codegen-vm::emit_artifact` accepts
`&VerifiedMir` rather than raw MIR. The legacy adapter may accept raw MIR only
because it immediately verifies it before delegating.

## Consequences

The backend-visible phase sequence is explicit:

```text
checked HIR -> MirModule -> VerifiedMir -> bytecode Artifact
```

This does not claim that the MIR migration is complete. Unsupported lowering
constructs still fail closed or use the explicitly feature-gated legacy path,
but new codegen entry points cannot silently broaden their input boundary.
