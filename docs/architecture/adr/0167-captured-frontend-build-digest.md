# ADR 0167: Bind captured frontend builds to their exact input digest

- Status: Accepted
- Date: 2026-08-14

## Problem

A project capture retains source, interface, and test files, while the pure
compiler intentionally accepts only source and interface inputs. Treating the
whole-workspace digest as the Artifact snapshot digest could therefore bind an
Artifact to files it did not compile, or obscure the exact compiler input.

## Decision

`CapturedProjectSnapshot` now exposes a frontend digest calculated from the
same immutable source/interface snapshot passed to `Compiler`. `ProjectCompiler::build_captured`
builds that captured input directly without rereading a path, and its Artifact
snapshot digest is the frontend digest. The existing workspace content digest
remains available for package-level identity and may include test files.
Its operation-aware counterpart preserves the same capture while forwarding
cancellation and deadline checks into the pure compiler.

`ProjectCompiler::compile_package` and its operation-aware counterpart now use
this captured frontend route as their default product path. The older
`snapshot`/`build` methods remain explicit compatibility APIs for package
analysis, native authorization, and review migration work.

## Non-goals

This does not migrate legacy package review, lock, native, or AOT compatibility
paths. It establishes the loader-to-pure-compiler route they can adopt.

## Compatibility and migration

The new project convenience API is additive. Existing legacy package builds
retain their package snapshot identity; no Artifact reader or Provider ABI
changes.

## Verifier and security impact

The Artifact verifier continues to validate its embedded digest. The change
makes the compiler input represented by that digest explicit and avoids a
project-loader time-of-check/time-of-use ambiguity. No authority or isolation
semantics change.

## Evidence

The SDK project-loader test captures a package through the loader, builds the
captured frontend input, and requires its Artifact digest to equal the
frontend digest; it also proves a pre-cancelled build fails before work begins.
The package convenience API must produce the same Artifact bytes as that
explicit capture and preserve cancellation/deadline outcome codes from the
loader rather than misclassifying them as package snapshot failures.
Architecture tests require the explicit API to remain in the project adapter
rather than the pure compiler façade.
