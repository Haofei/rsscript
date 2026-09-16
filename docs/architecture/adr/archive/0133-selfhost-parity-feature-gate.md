# ADR 0133: Feature-gate self-host parity from Core validation

## Status

Accepted

## Problem

The RSS-written frontend parity corpus is a Research asset, but its test module
was compiled whenever `rsscript-compiler` tests enabled `execution`.  As a
result, `cargo test --workspace` and the release workflow treated self-host
parity as a Core availability requirement.

## Decision and non-goals

`rsscript-compiler` exposes a test-only `selfhost-parity` feature.  The
self-host harness, its VM adapter, and generated interface metadata are
compiled only when both `execution` and `selfhost-parity` are enabled.

The dedicated `selfhost.yml` workflow enables both features.  Release
validation no longer runs the corpus.  This does not promote, remove, or alter
the corpus; it keeps the Research regression signal available without making
it part of the supported release path.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK, or persisted-data contract changes.
Maintainers running the corpus must add
`--features execution,selfhost-parity`; the documented commands and CI are
updated accordingly.

## Verifier and security impact

None.  Verified-bytecode and runtime behavior are unchanged.

## Provider and backend impact

The reference VM remains available to the research harness through the opt-in
feature.  Providers and supported backends are unaffected.

## Evidence

`cargo test --workspace` exercises the Core workspace without compiling the
self-host corpus.  The separate workflow invokes the corpus with the explicit
feature pair.
