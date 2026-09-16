# ADR 0179: Classify legacy executable IR as experimental

- Status: Accepted
- Date: 2026-08-14

## Problem

`rsscript-exec-ir` is a source-shaped transitional representation. It remains
necessary for explicit compatibility and MIR differential work, but it is not
the target Core execution boundary: M06 requires its deletion once the typed
MIR path reaches parity. Keeping the crate in the root default member set and
the Core tier made that legacy path part of every default product build and
test closure.

## Decision and non-goals

The crate remains a workspace member so explicit compatibility users can select
it, but it is classified as `experimental` and removed from root
`default-members`. It must be selected deliberately rather than treated as a
Core product package.

This change does not delete the crate, alter executable semantics, replace the
MIR parity gate, or change any artifact, Provider, SDK, or runtime contract.

## Compatibility and migration

Existing explicit package selections continue to work because
`rsscript-exec-ir` remains a workspace member. Default root Cargo commands no
longer build or test it solely because it is present in the repository. M06
remains the removal gate; this classification is not a new stable compatibility
promise for the legacy representation.

## Verifier and security impact

No verifier, trust, cancellation, resource, or isolation behavior changes.

## Provider and backend impact

The VM/MIR path remains the Core direction. Any compatibility or differential
consumer of the old representation must opt in explicitly; no Provider or
backend implementation changes are required.

## Evidence

- `cargo metadata --no-deps --format-version 1`
- `cargo test -p rsscript-sdk --test architecture --features compatibility`
- workspace-tier/default-member architecture checks
