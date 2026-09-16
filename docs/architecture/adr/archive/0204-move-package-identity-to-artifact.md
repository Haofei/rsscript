# ADR 0204: Move neutral package identity to the Artifact boundary

- Status: Accepted
- Date: 2026-08-14

`PackageIdentityV1` and `PackageFileKindV1` are persisted analysis facts, not
compiler, review, or native-policy concepts. They now live in
`rsscript-artifact`; compiler package compatibility re-exports the aliases
while its remaining review types are split out.
