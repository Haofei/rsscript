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
| Rust AOT backend | Experimental | Off | Differential parity with reference VM |
| Cranelift JIT, OSR, deopt | Experimental | Explicit trusted host | Differential parity, safe fallback, and workload evidence |
| Native plugins | Experimental | Off | Trusted provider boundary only |
| REIR review | Integration | Off | Consumes neutral analysis and metadata |
| Self-host frontend and C backend | Research | Off | Corpus and parity regression harness |

A feature is not promoted by implementation count. Promotion requires a stable
contract, conformance coverage, bounded failure behavior, supported-platform CI,
and a threat model consistent with [threat-model.md](threat-model.md).

Rust AOT, Cranelift JIT, REIR, and self-hosting are not feature-development
targets in this matrix. They accept only correctness, security, dependency, and
regression maintenance. The generation-oracle work does not split this
repository, delete a backend, or reorder the Cargo workspace.

## Language conformance

`✓` is Core coverage; `Experimental` and `Partial` are not Core support claims.

| Language area | Spec | Parser | Semantics | VM | Rust AOT | JIT | LSP | Tests |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ownership effects | ✓ | ✓ | ✓ | ✓ | Experimental | Experimental | ✓ | ✓ |
| Retention / escape | ✓ | ✓ | ✓ | ✓ | Experimental | Experimental | ✓ | ✓ |
| Resource lifetime | ✓ | ✓ | ✓ | ✓ | Experimental | Partial | ✓ | ✓ |
| Structured async | ✓ | ✓ | ✓ | ✓ | Partial | Partial | ✓ | ✓ |
| Cancellation cleanup | ✓ | ✓ | ✓ | ✓ | Partial | Partial | ✓ | ✓ |
| External symbols | ✓ | ✓ | ✓ | ✓ | Experimental | Experimental | ✓ | ✓ |
| Dynamic protocols | ✓ | ✓ | ✓ | ✓ | Experimental | Partial | ✓ | ✓ |

Parser acceptance alone never marks a language area supported. Core promotion
requires semantic validation, verified-VM conformance, diagnostics, LSP behavior,
and regression coverage together.
