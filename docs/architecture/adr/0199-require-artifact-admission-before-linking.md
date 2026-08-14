# ADR 0199: Require Artifact admission before Provider linking

- Status: Accepted
- Date: 2026-08-14

## Problem

Artifact verification proves bytecode structure, integrity, and compatibility.
It does not prove that the embedding host accepts the artifact's origin or
provenance. Treating a verified artifact as automatically linkable could make
a host accidentally equate integrity checking with its trust decision.

## Decision

The SDK adds a distinct `AdmittedArtifact` phase. A host converts a
`VerifiedArtifact` through `ArtifactAdmissionPolicy`, and `Runtime::link`
accepts only an `AdmittedArtifact`.

`TrustedInputAdmission` and `VerifiedArtifact::admit_trusted_input` remain
explicit conveniences for a host-controlled input channel. They do not make an
untrusted artifact safe. An isolated runner supplies a policy that records its
fixed runner-profile identity and descriptor digest as non-secret admission
evidence.

## Consequences

Verification, host admission, Provider linking, and execution are now distinct
typed phases. Detached signature, enterprise provenance, or runner-profile
checks can be added without reintroducing a language-level permissions system.
Existing callers must make their trust choice explicit before linking.
