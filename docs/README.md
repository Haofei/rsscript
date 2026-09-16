# RSScript Documentation

RSScript is a small, ownership-aware scripting language for scripts embedded in
Rust hosts. One compile produces a provider-neutral verified Artifact, a bounded
interpreter is the reference way to run it, and a trusted host can opt into the
Cranelift JIT. Start with the root [README](../README.md) for the overview,
[product.md](product.md) for scope and guarantees, and the
[language specification](spec/RSScript_v0.7_Spec.md) for the normative text.

This directory has one current document for each concern. Git history is the
archive for superseded plans, review reports, and remediation logs.

## Authority

When documents disagree, use this order:

1. [Language specification](spec/RSScript_v0.7_Spec.md) for the invariants and
   [RSScript Semantics v0.7](spec/RSScript_Semantics_v0.7.md) for every rule as
   implemented, with its diagnostic code and file citation
2. The [native JIT contract](spec/native-jit-contract.md)
3. Current code, tests, `rss --help`, and the root [README](../README.md)
4. The references and roadmap below

## Current Documents

| Document | Purpose |
| --- | --- |
| [product.md](product.md) | Users, Core workflow, invariants, maturity of every surface, language conformance, and the support boundary |
| [threat-model.md](threat-model.md) | Trust and isolation boundaries, untrusted input, and the reference isolated runner |
| [roadmap.md](roadmap.md) | Prioritized future work and explicit freezes |
| [provider-sdk.md](provider-sdk.md) | Writing a Provider: contract layers, lifecycle, replay, conformance, packages, and bindings |
| [compatibility.md](compatibility.md) | Versioned contracts, compatibility rules, releases, and SDK distribution |
| [architecture/ARCHITECTURE.md](architecture/ARCHITECTURE.md) | Code map: what each crate owns, its entry points, the invariants, and where to add things |
| [architecture/workspace-tiers.toml](architecture/workspace-tiers.toml) | Machine-checked package maturity and CI ownership |
| [architecture/contracts.toml](architecture/contracts.toml) | Machine-checked public contract identifiers and owning constants |
| [architecture/runner-platforms.toml](architecture/runner-platforms.toml) | Machine-readable isolation controls and platform limitations |
| [architecture/experimental-retention.toml](architecture/experimental-retention.toml) | Evidence, retention clocks, and product ownership for surfaces outside Core |
| [development/DEVELOPMENT.md](development/DEVELOPMENT.md) | Local development and verification |
| [development/DOCKER.md](development/DOCKER.md) | Containerized development |
| [generated/language-card.md](generated/language-card.md) | Generated keyword, diagnostic, and core-interface quick reference |
| [generated/language-card.json](generated/language-card.json), [grammar.json](generated/grammar.json), [diagnostic-catalog.json](generated/diagnostic-catalog.json), [core-interfaces.json](generated/core-interfaces.json), [signatures.md](generated/signatures.md) | Machine-readable generated language-reference catalogs and the core signature index |
| [planning/2026-09-model-failure-modes.md](planning/2026-09-model-failure-modes.md) | Measured model failure classes and the oracle changes they justified |
| [architecture/adr/](architecture/adr/README.md) | Decision records; live from 0226, older ones archived |

## Specifications

| Specification | Scope |
| --- | --- |
| [RSScript Spec Revision 7](spec/RSScript_v0.7_Spec.md) | Normative invariants for the `0.1.x` language line, including the execution and structured-concurrency rules |
| [RSScript Semantics v0.7](spec/RSScript_Semantics_v0.7.md) | Source-backed reference for the language semantics as implemented, with a diagnostic index |

The specifications are intentionally detailed and some tests read them by path.
Do not rename them without updating those tests.

## Generated language reference

[`generated/language-card.md`](generated/language-card.md) and its linked
grammar, keyword, diagnostic, and core-interface tables are generated from the
owning Rust registries. Machine consumers should use
[`language-card.json`](generated/language-card.json),
[`grammar.json`](generated/grammar.json),
[`diagnostic-catalog.json`](generated/diagnostic-catalog.json), and
[`core-interfaces.json`](generated/core-interfaces.json). Refresh them with
`cargo run -p rsscript-xtask -- language-card`; CI and local verification should
use `language-card --check`.

## Maintenance Rules

- Do not add dated status reports, completion ledgers, or a second roadmap.
- Update the authoritative contract, maturity, support, or roadmap document when
  a boundary closes or a limitation changes; do not create a second status ledger.
- Keep product claims consistent with `product.md` and `threat-model.md`.
- Update `roadmap.md` only for work that remains relevant to the current support
  policy.
- Put benchmark measurements beside the benchmark data, not in roadmap prose.
- Put historical rationale in commit messages or an ADR only when the decision
  remains binding.
- Remove a superseded document in the same change that updates its inbound
  links.
