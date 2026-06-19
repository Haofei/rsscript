# RSScript Specifications & Design Docs

Normative specifications and design drafts for RSScript. The project root keeps
only `README.md` (overview) and `AGENT.md` (the prompt-sized language guide for
LLMs); everything else lives here, grouped into categorized subfolders.

## Spec

| Doc | What it governs |
|-----|-----------------|
| [RSScript_v0.7_Spec.md](spec/RSScript_v0.7_Spec.md) | The language. Opens with a binding **Constitution** (Articles I–IX) that overrides every chapter. |
| [RSScript_Execution_Spec_v0.1.md](spec/RSScript_Execution_Spec_v0.1.md) | The execution engine: reg-VM + JIT tiers — the single home. Normative parity contract (interp ≡ tier-0 ≡ native ≡ AOT), calling convention, sandbox/hardening, host-helper ABI (§0–11), plus the consolidated implementation baseline, JIT phase status, and per-feature parity ledger (Part II appendices). Subordinate to the language spec. |
| [RSScript_Package_Manager_Design_v0.6.md](spec/RSScript_Package_Manager_Design_v0.6.md) | The package manager (`rss pkg`): `.rssi` contracts, `rsspkg.toml`/`.lock`, semantic dependency review. |
| [Review_Evidence_IR_Spec_v0.2.md](spec/Review_Evidence_IR_Spec_v0.2.md) | REIR — the review-evidence IR consumed by `--reir` tooling and CI gates. |

## Architecture

| Doc | What it governs |
|-----|-----------------|
| [ARCHITECTURE.md](architecture/ARCHITECTURE.md) | Module boundaries of the checker/lowering implementation. |

## Ledgers

| Doc | What it governs |
|-----|-----------------|
| [rss-selfhost-ledger.md](ledgers/rss-selfhost-ledger.md) | Self-hosting progress ledger for the RSScript-in-RSScript toolchain. |

## Development

| Doc | What it governs |
|-----|-----------------|
| [DEVELOPMENT.md](development/DEVELOPMENT.md) | Local verification flow and development discipline. |
| [DOCKER.md](development/DOCKER.md) | Containerized, cross-platform dev environment (Docker / VS Code / Codespaces). |

## Planning

| Doc | Idea |
|-----|------|
| [spec-todo.md](planning/spec-todo.md) | Prioritized list of unimplemented spec surface (the §3.2 / §20.1 superset). |
| [ml-perf-todo.md](planning/ml-perf-todo.md) | ML-framework perf plan: native tensor kernels (fix VM big-matrix cliff) + AOT build-time levers. |
| [cross-isolate-design.md](planning/cross-isolate-design.md) | Feasibility + smallest-sound-slice plan for the cross-isolate message API (§20.2-3): message-channel core landed, multi-heap isolates still future. |
| [RSScript_AI_Generation_Feedback_v0.1.md](planning/RSScript_AI_Generation_Feedback_v0.1.md) | Agent-facing generation oracle plus fast interpreter feedback loop for AI-generated RSScript. |
| [declarative-rewrite-roadmap.md](planning/declarative-rewrite-roadmap.md) | Highest-leverage feature for the ML port: escaping/storable closures (keystone) → a PatternMatcher/graph_rewrite library, so the scheduler transliterates tinygrad instead of paraphrasing it. |

## Conventions

- The **Constitution** (in the language spec) is the highest authority; a chapter
  that conflicts with an article is in error (§0 normative hierarchy).
- Generated artifacts are produced from a single source of truth with a
  freshness-guard test (e.g. the VS Code grammar from the lexer `KEYWORDS` table);
  the design drafts above extend that discipline.
- Some tests read these files by path (`tests/checker_package.rs`,
  `reir/tests/reir_tests.rs`); if you rename or move a spec, update those joins.
