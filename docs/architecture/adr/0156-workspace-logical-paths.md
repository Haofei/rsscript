# ADR 0156: Separate workspace logical paths from physical paths

- Status: Accepted
- Date: 2026-08-14

## Problem

The OS-facing workspace loader retained absolute physical paths and
package-relative display paths, but had no explicit stable source identity.
Different dependencies can contain the same relative interface path, while
absolute paths must never influence a reproducible snapshot.

## Decision

Every `WorkspaceSourceFile` now carries a `logical_path`. Root files use a
`root/` prefix. Dependency interface files use a deterministic content-derived
dependency identity followed by their package-relative path. Snapshot digests
use logical paths, file roles, and bytes; physical paths remain only for loader
and editor adaptation.

## Non-goals

This does not define package registry identity, change manifest semantics, or
move all package lowering into the workspace loader.

## Compatibility and migration

`path` and `relative_path` remain available. The snapshot digest intentionally
changes to eliminate ambiguity between same-named dependency files; consumers
must treat it as a content identity, not a long-lived package version.

## Verifier, security, and backend impact

The compiler can consume logical source identity without filesystem access.
Artifact provenance remains bound to immutable input bytes, while Providers,
verification, and VM execution never receive physical workspace paths.

## Evidence

Workspace-loader tests cover absolute-path independence, enumeration-order
stability, and distinct logical paths for matching relative names. LSP and SDK
architecture suites continue to consume the explicit loader boundary.
