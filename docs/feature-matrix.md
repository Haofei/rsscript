# Feature maturity

`Core` means the contract is part of the supported product. `Experimental`
means correctness is tested but the API or backend may change. `Integration`
means an optional consumer of Core artifacts. `Research` is retained for
regression value and does not drive the product roadmap.

| Surface | Maturity | Default | Contract |
| --- | --- | --- | --- |
| Parser, formatter, baseline diagnostics | Core | On | Language specification |
| Agent-authored generation oracle | Experimental | On in CLI | `rsscript.generate.*.v1`; [ADR 0232](architecture/adr/0232-parser-owned-generation-oracle.md) |
| Structured diagnostics and machine fixes | Core | On | `rss check --json` and `rss fix --json` |
| Generated machine language context | Experimental | On | `docs/generated/*.json` with freshness checks |
| Generation-oracle evaluation corpus | Experimental | Explicit tooling | `rsscript.agent_eval.v1`; deterministic offline fixtures |
| Types, ownership, retention, resources | Core | On | Language specification |
| Structured async and cancellation semantics | Core | On | Language and execution specifications |
| External semantic symbols | Core | On | Interface and binding schemas |
| Register VM | Core | On | Reference execution semantics |
| Package snapshot and neutral analysis | Core | On | Versioned artifact schemas |
| Artifact Bundle and semantic diff | Core | On | Bundle and `rsscript.semantic_diff.v2` schemas |
| Reference isolated runner | Experimental | On in CLI | Runner protocol v1 and process limits |
| Host providers | Experimental | Explicit | Provider ABI and runner configuration |
| Cranelift JIT, OSR, deopt | Experimental (active) | Explicit trusted host | Differential parity, safe fallback, and workload evidence; see [roadmap.md](roadmap.md) |
| Native plugins | Experimental | Off | Trusted provider boundary only |
| Rust AOT, REIR review, self-host frontend | Archived | Off | `archive/experiments-2026-09` branch; not workspace members |

A feature is not promoted by implementation count. Promotion requires a stable
contract, conformance coverage, bounded failure behavior, supported-platform CI,
and a threat model consistent with [threat-model.md](threat-model.md).

Rust AOT, REIR, and self-hosting are archived and receive no changes. The
Cranelift JIT is different: it is a required VM tier under
active development, and its Experimental maturity records that its accounting
and platform contracts are still converging, not that it is a removal
candidate. The generation-oracle work does not split this repository, delete a
backend, or reorder the Cargo workspace.

## Language conformance

`✓` is Core coverage; `Experimental` and `Partial` are not Core support claims.

| Language area | Spec | Parser | Semantics | VM | JIT | LSP | Tests |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Ownership effects | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Retention / escape | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Resource lifetime | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| Structured async | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| Cancellation cleanup | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| External symbols | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Dynamic protocols | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |

Parser acceptance alone never marks a language area supported. Core promotion
requires semantic validation, verified-VM conformance, diagnostics, LSP behavior,
and regression coverage together.

## Support boundary

Execution limits, cancellation, deadlines, output caps, and child-process
limits are supported availability controls; none of them, and neither the
in-process VM, the JIT, nor a Provider, is a security sandbox. Untrusted,
third-party, or machine-generated scripts require the isolated runner;
successful validation is not authorization to execute in the host process.

Official host Providers remain Experimental and fail closed where their stated
authority boundary cannot be implemented: the rooted filesystem Provider
requires the descriptor-relative, no-follow Unix implementation and rejects
construction elsewhere rather than falling back to canonicalize-then-open. The
reference runner remains Experimental until its platform isolation matrix is
complete. Rust AOT, REIR, and self-hosting are archived on the
`archive/experiments-2026-09` branch and are not supported surfaces.
