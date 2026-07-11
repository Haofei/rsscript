# RSScript Specifications & Design Docs

Normative specifications and design drafts for RSScript. The project root keeps
only `README.md` (overview) and `AGENT.md` (the prompt-sized language guide for
LLMs); everything else lives here, grouped into categorized subfolders.

## Spec

| Doc | What it governs |
|-----|-----------------|
| [RSScript_v0.7_Spec.md](spec/RSScript_v0.7_Spec.md) | The language. Opens with a binding **Constitution** (Articles I–IX) that overrides every chapter. |
| [RSScript_Execution_Spec_v0.1.md](spec/RSScript_Execution_Spec_v0.1.md) | The execution engine: reg-VM + JIT tiers — the single home. Normative parity contract (interp ≡ tier-0 ≡ native ≡ AOT), calling convention, sandbox/hardening, host-helper ABI (§0–11), plus the consolidated implementation baseline, JIT phase status, and per-feature parity ledger (Part II appendices). Subordinate to the language spec. |
| [RSScript_Package_Manager_Design_v0.6.md](spec/RSScript_Package_Manager_Design_v0.6.md) | The package manager (`rss pkg`): `.rssi` contracts, `rsspkg.toml`/`.lock`, semantic dependency review. This is still a v0.6 design document; use the root README and `rss --help` for implemented command shape. |
| [Review_Evidence_IR_Spec_v0.2.md](spec/Review_Evidence_IR_Spec_v0.2.md) | REIR — the review-evidence IR consumed by `--reir` tooling and CI gates. |

## Architecture

| Doc | What it governs |
|-----|-----------------|
| [ARCHITECTURE.md](architecture/ARCHITECTURE.md) | Module boundaries of the checker/lowering implementation. |

## Ledgers

| Doc | What it governs |
|-----|-----------------|
| [self-hosting.md](self-hosting.md) | Single canonical self-hosting doc: status, validation model, dump formats, and `SH-*` ledger. |

## Development

| Doc | What it governs |
|-----|-----------------|
| [DEVELOPMENT.md](development/DEVELOPMENT.md) | Local verification flow and development discipline. |
| [DOCKER.md](development/DOCKER.md) | Containerized, cross-platform dev environment (Docker / VS Code / Codespaces). |

## Planning

Planning docs are non-normative and can be active roadmaps or historical evidence.
Start with [planning/README.md](planning/README.md), which separates current
roadmaps from shipped/rejected performance notes and records the authority order
for resolving stale claims.

## Conventions

- The **Constitution** (in the language spec) is the highest authority; a chapter
  that conflicts with an article is in error (§0 normative hierarchy).
- Generated artifacts are produced from a single source of truth with a
  freshness-guard test (e.g. the VS Code grammar from the lexer `KEYWORDS` table);
  the design drafts above extend that discipline.
- Some tests read these files by path (`tests/checker_package.rs`,
  `reir/tests/reir_tests.rs`); if you rename or move a spec, update those joins.
