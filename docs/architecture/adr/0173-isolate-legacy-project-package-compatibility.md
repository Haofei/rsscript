# ADR 0173: Isolate legacy project package compatibility

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed SDK `project` feature used the OS-facing workspace loader for its
normal `compile_package` path, but still selected the compiler `package`
feature because `ProjectCompiler` also exposed legacy package snapshot and
build methods. As a result, ordinary project capture widened the compiler
dependency closure to package review, persistence, and native compatibility
code even when callers only needed loader-to-frontend compilation.

## Decision and non-goals

`project` now selects only the workspace loader and reviewed bytecode
compilation path. `ProjectCompiler` owns explicit-base capture and builds only
the resulting immutable `FrontendInputSnapshot`.

The existing package-analysis/native snapshot API remains available only as
`project::legacy::PackageCompatibility` behind the explicit SDK
`compatibility` feature. This record does not delete the remaining compiler
package implementation, alter package analysis artifacts, or introduce a new
manifest format.

## Compatibility and migration

New callers use:

```rust
let captured = ProjectCompiler::new().capture_frontend_from(base, package)?;
let artifact = ProjectCompiler::new().build_captured(&captured)?;
```

Compatibility callers that intentionally require package review/native state
must opt into `compatibility` and call `PackageCompatibility::{snapshot,build}`
explicitly. The root compatibility re-exports are unchanged for the current
migration window.

## Security and architecture impact

The normal project path no longer compiles compiler package persistence or
native authorization code merely because it reads a directory. The compiler's
reviewed input remains a captured immutable in-memory snapshot, while OS
traversal stays in `rsscript-workspace-loader`.

## Evidence

SDK tests cover normal loader capture, immutable frontend builds, cancellation,
and the isolated compatibility snapshot path. Architecture tests assert that
the `project` feature does not select `rsscript_compiler/package`, while its
legacy module remains explicit. Targeted SDK tests and clippy pass.
