# ADR 0205: Move neutral package-analysis schema to the Artifact boundary

- Status: Accepted
- Date: 2026-08-14

`rsscript.package_analysis.v1` is persisted, Provider-neutral semantic
evidence. Its schema, producer identity, exports, external imports, call graph,
resource, task, await, and source-file facts therefore belong to
`rsscript-artifact`, alongside the existing package identity and semantic-diff
contracts.

`rsscript-compiler` now produces the Artifact-owned model and re-exports its
historical package names only from the compatibility package module. The
compiler supplies build-specific producer provenance at emission time; the
Artifact crate does not infer compiler version or source revision from its own
crate metadata.

Artifact Bundles now decode this schema through its typed model and reject
unknown fields rather than retaining it as unchecked `serde_json::Value`.
This is behavior-preserving for valid package-analysis JSON and intentionally
does not make review/risk policy or package filesystem traversal part of the
Artifact contract.
