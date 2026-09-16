# ADR 0149: Project capture is outside the reviewed Compiler facade

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed SDK `Compiler` exposed both in-memory snapshot compilation and
path-based package capture/build methods. That conflated the compiler's pure
frontend contract with filesystem traversal, package graph capture, and the
legacy compiler package layer.

## Decision and non-goals

The default `execution` feature now exposes `Compiler` only for immutable
`FrontendInputSnapshot` check/build operations. Path-oriented capture,
snapshot build, and `compile_package` live in the explicit
`project::ProjectCompiler` adapter under the `project` feature. The CLI opts
into that adapter as its composition-root convenience.

This does not yet relocate package graph implementation code out of
`rsscript-compiler`, introduce a registry, or change the package manifest.
It establishes the public SDK boundary before that internal extraction.

## Compatibility and migration

This is a pre-1.0 SDK API change. Embedders using path methods migrate to
`rsscript_sdk::project::ProjectCompiler` and enable the `project` feature.
Embedders that already provide source bytes use `Compiler::compile_snapshot`
unchanged. Artifact, bytecode, Provider, and language contracts do not change.

## Verifier and security impact

Artifact verification and runtime behavior are unchanged. The change makes it
mechanically harder for new reviewed embedding code to hide filesystem input
capture behind the compiler API; package capture still completes before the
immutable snapshot is built.

## Provider and backend impact

Providers and bytecode backends are unaffected. The CLI remains the composition
root and explicitly selects `project`; ordinary SDK execution does not require
the project convenience surface.

## Evidence

SDK architecture tests assert that `Compiler` contains no package/path capture
entrypoints and that `ProjectCompiler` owns those operations. SDK execution and
project-feature tests compile and run separately, and the CLI execution build
uses the project adapter.
