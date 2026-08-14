# ADR 0206: Keep package-analysis presentation out of the compiler

- Status: Accepted
- Date: 2026-08-14

`PackageAnalysisV1` is an Artifact wire contract. The compiler may produce it,
but JSON presentation is not a compilation responsibility. The historical
`format_package_analysis_json` compiler/SDK compatibility export has therefore
been removed.

The CLI serializes the typed Artifact contract at its application boundary;
embedding callers can use their own serializer or consume an Artifact Bundle.
Review, risk, lockfile, and legacy package-format presentation remain separate
compatibility work and are not reclassified as compiler responsibilities.
