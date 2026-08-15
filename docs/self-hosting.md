# RSScript Self-Hosting

Self-hosting is frozen Research. The retained corpus and parity harness are
regression assets owned under `experiments/fixtures/selfhost`; the root-level
`selfhost` path is a compatibility symlink for the feature-gated harness, not a
Core asset. A standalone compiler, stage1/stage2 bootstrap, and further C backend
expansion are not current product goals.

## Terms

| State | Meaning | Current? |
| --- | --- | --- |
| Self-hosting stress test | RSS frontend tools run through the Rust-built VM and are compared with Rust oracles | Yes |
| Self-hosted frontend | One RSS lexer/parser/AST/checker analyzes the compiler's own RSS sources | No |
| Self-hosted compiler | RSS lowering/backend rebuild the compiler without Rust after bootstrap | No |

Shipping a prebuilt Rust compiler removes Rust from user installation. It does
not make compiler development self-hosting.

## Historical target (not active roadmap)

```text
RSS sources
  -> RSS lexer/parser
  -> materialized Program AST
  -> RSS name/type/effect/package checking
  -> canonical compiler IR
  -> portable bootstrap backend
  -> stable runtime ABI
  -> standalone compiler
```

The bootstrap backend is a portable C emitter. This avoids requiring RSS to
implement an object writer, linker, Cranelift, or LLVM before it can rebuild
itself.

Reproducible bootstrap requires:

```text
stage0: trusted Rust compiler builds the RSS compiler
stage1: resulting compiler builds the same RSS sources
stage2: stage1 compiler builds them again
proof:  canonical stage1 and stage2 IR/output agree
```

## Current Components

| Component | Implementation | Established proof | Main gap |
| --- | --- | --- | --- |
| Scanner | `selfhost/scan.rss` | Shared token primitives and delimiter indexes | Not the sole frontend token source |
| Lexer | `selfhost/lexer.rss` | Canonical full-corpus token parity | Test harness only |
| Recognizer | `selfhost/parser.rss` | Top-level accept/reject parity | Does not own the reusable AST |
| AST | `selfhost/syntax/*`, `selfhost/astdump.rss` | Materialized declarations and a growing body/pattern/expression subset | Legacy rendering/reparse paths remain |
| Checker | `selfhost/check.rss` | Presence and occurrence/span parity for 84 diagnostic families | Rust remains semantic oracle; token probes remain |
| Package contract | `selfhost/package_contract.rss` | `RS1301` coverage for major declaration families | Path-sensitive and edge cases remain |
| Canonical IR | `selfhost/ir/canonical.rss` | Initial signatures/scalar/control/call/place slice | Ownership, generics, closures, intrinsics, package artifacts |
| C backend | `selfhost/backend/c_emit.rss`, `selfhost/runtime/rssrt.{h,c}` | Initial scalar C ABI | Heap and full compiler IR |
| Production lowering/runtime | Rust | Current executable toolchain | Not source-independent |

The Rust test harness in `crates/rsscript-compiler/src/selfhost_parity.rs` resolves these
RSS modules, runs them through the normal VM compiler, and compares deterministic
output with production Rust oracles. Self-hosting adds no public CLI commands.

## Last Recorded Baseline

The last audited baseline in the removed historical ledger was 2026-07-18:

| Gate | Result |
| --- | --- |
| Lexer corpus, strict tier | 642 / 642 |
| Parser recognition | 642 / 642 |
| Checker FAST | 593 / 632; 39 code mismatches |
| Package contract | Passed for covered families |
| Curated AST | Passed |
| Full AST corpus | Not established |
| Checker FULL | Not established |

These numbers are evidence from that run, not a current CI claim. Current test
results are authoritative when they differ. `selfhost/corpus.txt` is the
checked-in corpus inventory and must change with intentional corpus additions
or removals.

## Freeze policy

Allowed work is limited to:

1. preserving corpus, parity and already-derived regression coverage;
2. correctness fixes when a retained test finds a Core defect;
3. deterministic maintenance needed to keep the harness runnable.

Do not expand the C emitter, portable bootstrap runtime, self-hosted frontend,
canonical self-host IR, or stage1/stage2 pipeline without a future product
decision that explicitly removes this freeze.

Performance work is admitted only when measured self-host wall time blocks a
parity gate. JIT coverage is not a bootstrap correctness requirement.

## Completion Criteria

| Milestone | Required proof |
| --- | --- |
| Reliable parity harness | Exact corpus inventory, strict protocols, frontend and package oracle coverage |
| Self-hosted frontend | One AST and semantic model analyze the compiler's RSS sources |
| Self-hosted lowering | RSS and Rust canonical IR agree on the supported corpus |
| Binary independence | RSS frontend/lowering/backend produce a standalone compiler using a released runtime |
| Source independence | Compiler and minimal runtime rebuild without Cargo or `rustc` |
| Full bootstrap | Clean stage1/stage2 rebuild is reproducible and independently checked |

Line counts and implemented diagnostic families are progress signals, not
completion proofs.

## Validation

| Layer | RSS tool | Oracle/comparison |
| --- | --- | --- |
| Lexer | `selfhost/lexer.rss` | `crate::lexer::lex`; canonical token records |
| Parser | `selfhost/parser.rss` | `parse_source_raw`; verdict and position tier |
| AST | `selfhost/astdump.rss` | Rust surface AST dump; byte-exact |
| Checker | `selfhost/check.rss` | `analyze_source`; code and structured span parity |
| Package | `selfhost/package_contract.rss` | package review; filtered `RS1301` |
| Future lowering | RSS lowerer | canonical normalized IR |
| Future backend | RSS C emitter | VM/AOT behavior and artifact checks |

Useful gates:

```sh
docker compose run --rm dev cargo test -p rsscript-compiler --features selfhost-parity selfhost_parity -- --test-threads=1
docker compose run --rm -e RSS_SELFHOST_TIER=2 dev cargo test -p rsscript-compiler --features selfhost-parity --release --lib selfhost_parity::lexer_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm -e RSS_SELFHOST_PARSE_TIER=1 dev cargo test -p rsscript-compiler --features selfhost-parity --release --lib selfhost_parity::parser_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm dev cargo test -p rsscript-compiler --features selfhost-parity --release --lib selfhost_parity::checker_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm -e RSS_SELFHOST_AST_TIER=2 dev cargo test -p rsscript-compiler --features selfhost-parity --release --lib selfhost_parity::ast_parity_samples -- --exact --test-threads=1 --nocapture
docker compose run --rm dev cargo test -p rsscript-compiler --features selfhost-parity --lib selfhost_parity::package_contract_ -- --nocapture
```

## Token Dump Contract

One token per line:

```text
<line>:<col>:<len>\t<KIND>\t<PAYLOAD>
```

- `line` and `col` identify the token start.
- `len` counts Unicode scalar values, not bytes or graphemes.
- Kinds are `Ident`, `Number`, `String`, `Char`, `InterpolatedString`,
  `MultilineString`, `Keyword`, `Symbol`, `Unknown`, and `Eof`.
- Escaping is deterministic: `\` becomes `\\`; newline, tab, and carriage
  return become `\n`, `\t`, and `\r`.
- Tier 0 compares kind/payload, tier 1 adds line/column, tier 2 adds length.

The Rust lexer is the oracle.

## AST Dump Contract

The canonical dump is:

```text
<two-space indent><TAG>[ <key>=<value>]*[ <PAYLOAD>]
```

Tags and attribute order are fixed, payload is last, and output ends with a
newline. Top-level order is program, features, source-order items, protocols,
protocol implementations, then diagnostic markers. The format covers item,
type, statement, pattern, and expression node families. The oracle is the
surface-preserving Rust parser, not a desugared AST.

Format changes require a versioned migration and byte-exact oracle tests.

## Historical Findings

Self-hosting exposed real defects in mutation writeback, generic lowering,
character literals, operator continuation, positional variant patterns,
freshness flow, call binding, VM/AOT errors, and accidental deep copies. Those
fixes remain protected by tests. Detailed `SH-*` chronology was removed from the
working documentation; Git history is the archive.
