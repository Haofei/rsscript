# RSScript Documentation

This directory has one current document for each concern. Git history is the
archive for superseded plans, review reports, and remediation logs.

## Authority

When documents disagree, use this order:

1. [Language specification](spec/RSScript_v0.7_Spec.md)
2. [Execution specification](spec/RSScript_Execution_Spec_v0.1.md)
3. [REIR specification](spec/Review_Evidence_IR_Spec_v0.2.md)
4. Current code, tests, `rss --help`, and the root [README](../README.md)
5. The references and roadmap below

## Current Documents

| Document | Purpose |
| --- | --- |
| [product.md](product.md) | Product users, Core workflow, and invariants |
| [threat-model.md](threat-model.md) | Trust, isolation, provider, and untrusted-input boundaries |
| [feature-matrix.md](feature-matrix.md) | Core, Experimental, Integration, and Research maturity |
| [support.md](support.md) | Supported and unsupported execution surfaces |
| [status.md](status.md) | Current closure state, accepted limitations, and open engineering debt |
| [roadmap.md](roadmap.md) | Prioritized future work and explicit freezes |
| [releasing.md](releasing.md) | Multi-platform binaries, dry-run, provenance, and SDK distribution |
| [package.md](package.md) | Implemented package artifacts, commands, review model, and trust boundary |
| [architecture/ARCHITECTURE.md](architecture/ARCHITECTURE.md) | Module ownership and dependency rules |
| [development/DEVELOPMENT.md](development/DEVELOPMENT.md) | Local development and verification |
| [development/DOCKER.md](development/DOCKER.md) | Containerized development |
| [self-hosting.md](self-hosting.md) | Experimental self-hosting goal, current coverage, and validation contract |

## Specifications

| Specification | Scope |
| --- | --- |
| [RSScript v0.7](spec/RSScript_v0.7_Spec.md) | Language syntax and semantics |
| [Execution v0.1](spec/RSScript_Execution_Spec_v0.1.md) | Interpreter, JIT, AOT parity, limits, and host ABI |
| [REIR v0.2](spec/Review_Evidence_IR_Spec_v0.2.md) | Review evidence model and reconciliation |

The specifications are intentionally detailed and some tests read them by path.
Do not rename them without updating those tests.

## Maintenance Rules

- Do not add dated status reports, completion ledgers, or a second roadmap.
- Update `status.md` when a boundary closes or a limitation changes.
- Keep product claims consistent with `product.md` and `threat-model.md`.
- Update `roadmap.md` only for work that remains relevant to the current support
  policy.
- Put benchmark measurements beside the benchmark data, not in roadmap prose.
- Put historical rationale in commit messages or an ADR only when the decision
  remains binding.
- Remove a superseded document in the same change that updates its inbound
  links.
