# ADR 0211: Bind checked source call facts to Artifact analysis

- Status: Accepted
- Date: 2026-08-14

`SourceAnalysisV1` originally identified only the source files in a direct
SDK build. That made `SemanticDiffV1` depend on the legacy package-analysis
path to report call-graph evidence, even though the normal compiler had
already checked those calls.

Direct source analysis now records canonically ordered resolved call edges and
direct external-call facts from the exact checked HIR used to emit bytecode.
The facts remain provider-neutral and make no risk, authority, or deployment
decision. Package analysis may still add richer transitive chains and package
metadata, but normal `rss build` and `rss diff` no longer need that compatibility
path to report direct call behavior.
