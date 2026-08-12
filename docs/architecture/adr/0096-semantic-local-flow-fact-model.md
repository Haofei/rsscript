# ADR 0096: Semantic ownership of local-flow facts

## Status

Accepted.

## Decision

`rsscript-semantics` owns the backend-neutral facts emitted by local ownership
and resource flow: moved uses, managed-to-local uses, retained locals and
closure captures, handle-field takes, fresh-return proof failures, and resource
escapes. Compiler retains the transitional CFG producer only.

## Compatibility and impact

The fact structures keep the same fields and equality behavior. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Existing compiler local-flow tests continue to construct and compare the shared
semantic facts. Architecture tests reject compiler redefinition of the models.
