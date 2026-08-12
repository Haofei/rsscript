# ADR 0091: Semantic ownership of module/use layout rules

## Status

Accepted.

## Decision

`rsscript-semantics` owns source-file-local `module`/`use` organization
validation: module ordering and uniqueness, use ordering, and local import
binding uniqueness. The compiler provides the parsed items only.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests verify duplicate import and misplaced-module diagnostics.
Architecture tests reject the former compiler implementation.
