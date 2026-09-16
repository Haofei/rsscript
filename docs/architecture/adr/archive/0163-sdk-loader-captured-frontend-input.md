# ADR 0163: Expose loader-captured frontend input separately from package compatibility

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler package `WorkspaceSnapshot` still combines legacy package review,
native authorization, and temporary filesystem capture. Replacing it directly
with the lightweight workspace loader would either discard those compatibility
checks or keep OS traversal in the compiler-facing API.

## Decision

Add `project::ProjectCompiler::capture_frontend_from(base, package_dir)` to the
SDK. It uses `rsscript-workspace-loader` with an explicit base path, retains its
stable logical-path content digest, and produces a separate
`CapturedProjectSnapshot` exposing an immutable `FrontendInputSnapshot`.
Only source and interface files enter that frontend input; test files remain in
the capture for tooling but do not become executable build sources.

The existing package snapshot remains a compatibility path for native/review
work until its policy concerns move out of compiler package code.

## Non-goals

This does not replace package manifest validation, native authorization,
dependency lock handling, Artifact persistence, or package review. It does not
make the compiler read a filesystem path.

## Compatibility and migration

This is an additive SDK project-feature API. New project embedders can capture
once and invoke `Compiler::compile_snapshot` with the result. Existing package
callers retain their behavior and Artifact/Provider/runtime schemas are
unchanged.

## Verifier, security, and backend impact

The loader's logical paths exclude host-absolute paths, preserving portable
input identity. The compiler consumes only immutable bytes; no Provider,
runtime, or backend capability is added. Tests verify the explicit-base path,
logical-path invariant, interface capture, and pure compiler handoff.
