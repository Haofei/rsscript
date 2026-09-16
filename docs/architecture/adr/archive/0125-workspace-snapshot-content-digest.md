# ADR 0125: Workspace snapshots carry a content identity

- Status: Accepted
- Date: 2026-08-14

## Problem

Compiler and Artifact boundaries need to prove that analysis and executable
output derive from one immutable captured input. The workspace loader exposed
captured files but no stable identity for their content.

## Decision and non-goals

`WorkspaceSnapshot` now includes a SHA-256 content digest over a
domain-separated, canonical multiset of file kind, relative path, and bytes.
Absolute physical paths and filesystem enumeration order are excluded. This is
an input identity, not an origin signature or an Artifact format change.

## Compatibility and migration

The loader API gains a read-only `content_digest` accessor. Existing `load`
compatibility APIs keep returning only files. Compiler package capture has not
yet migrated and therefore does not yet bind this value into Artifacts.

## Verifier and security impact

The digest supports later analysis/Artifact consistency checks but does not
authenticate authorship. Hosts needing provenance must still apply an external
origin-verification policy.

## Provider and backend impact

None. Provider selection and executable backend behavior remain outside input
capture.

## Evidence

Loader tests prove invariance across absolute roots and file enumeration order,
and sensitivity to content changes. The architecture test requires the digest
boundary and accounts for its hash dependency.
