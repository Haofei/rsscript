# RSScript Specifications & Design Docs

Normative specifications and design drafts for RSScript. The project root keeps
only `README.md` (overview) and `AGENT.md` (the prompt-sized language guide for
LLMs); everything else lives here.

## Normative specs

| Doc | What it governs |
|-----|-----------------|
| [RSScript_v0.6_Spec.md](RSScript_v0.6_Spec.md) | The language. Opens with a binding **Constitution** (Articles I–IX) that overrides every chapter. |
| [RSScript_Package_Manager_Design_v0.6.md](RSScript_Package_Manager_Design_v0.6.md) | The package manager (`rss pkg`): `.rssi` contracts, `rsspkg.toml`/`.lock`, semantic dependency review. |
| [Review_Evidence_IR_Spec_v0.2.md](Review_Evidence_IR_Spec_v0.2.md) | REIR — the review-evidence IR consumed by `--reir` tooling and CI gates. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Module boundaries of the checker/lowering implementation. |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Local verification flow and development discipline. |

## Design drafts (not yet normative)

| Doc | Idea |
|-----|------|
| [RSScript_Constrained_Generation_v0.1_Draft.md](RSScript_Constrained_Generation_v0.1_Draft.md) | Compiler-as-decoding-oracle: forbid illegal tokens during LLM generation (Article IX tooling). |
| [RSScript_Interpreter_v0.1_Draft.md](RSScript_Interpreter_v0.1_Draft.md) | Fast tree-walking interpreter for ms-level behavioral feedback (agent loop, `rss test`). |

## Conventions

- The **Constitution** (in the language spec) is the highest authority; a chapter
  that conflicts with an article is in error (§0 normative hierarchy).
- Generated artifacts are produced from a single source of truth with a
  freshness-guard test (e.g. the VS Code grammar from the lexer `KEYWORDS` table);
  the design drafts above extend that discipline.
- Some tests read these files by path (`tests/checker_package.rs`,
  `reir/tests/reir_tests.rs`); if you rename or move a spec, update those joins.
