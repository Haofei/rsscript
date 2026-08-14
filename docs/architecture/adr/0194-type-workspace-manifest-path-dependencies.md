# ADR 0194: Type workspace manifest path dependencies

- Status: Accepted
- Date: 2026-08-14

## Problem

The OS-facing workspace loader discovered dependencies by traversing an
untyped `toml::Value` tree. That made the set of filesystem inputs depend on
ad-hoc field lookups and gave project callers no typed contract for the local
dependency forms the loader actually supports.

## Decision

`rsscript-workspace-loader` now parses `rsspkg.toml` into
`WorkspaceManifestV1`. Its public projection contains only explicit local
path dependencies, with their name, declaration section, and manifest-relative
path. Version, git, registry, and other dependency forms may remain present in
a manifest but cannot cause workspace capture to read arbitrary additional
paths.

The loader resolves only this typed projection relative to the owning manifest
directory. The compiler continues to consume the resulting immutable source
snapshot and does not parse manifests.

## Consequences

Local dependency discovery is deterministic and independently testable without
making the workspace loader a registry resolver. A future resolver can add new
typed dependency forms behind a new project-input contract instead of extending
dynamic TOML traversal in compiler-adjacent code.
