# Feature maturity

`Core` means the contract is part of the supported product. `Experimental`
means correctness is tested but the API or backend may change. `Integration`
means an optional consumer of Core artifacts. `Research` is retained for
regression value and does not drive the release roadmap.

| Surface | Maturity | Default | Contract |
| --- | --- | --- | --- |
| Parser, formatter, diagnostics | Core | On | Language specification |
| Types, ownership, retention, resources | Core | On | Language specification |
| Structured async and cancellation semantics | Core | On | Language and execution specifications |
| External semantic symbols | Core | On | Interface and binding schemas |
| Register VM | Core | On | Reference execution semantics |
| Package snapshot and neutral analysis | Core | On | Versioned artifact schemas |
| Host providers | Experimental | Explicit | Provider ABI and runner configuration |
| Rust AOT backend | Experimental | Off | Differential parity with reference VM |
| Cranelift JIT, OSR, deopt | Experimental | Off | Differential parity and hard limits |
| Native plugins | Experimental | Off | Trusted provider boundary only |
| REIR review | Integration | Off | Consumes neutral analysis and metadata |
| Self-host frontend and C backend | Research | Off | Corpus and parity regression harness |

A feature is not promoted by implementation count. Promotion requires a stable
contract, conformance coverage, bounded failure behavior, supported-platform CI,
and a threat model consistent with [threat-model.md](threat-model.md).

