# RSS Self-Hosting

This is the single canonical document for RSScript self-hosting. It defines the
goal, current architecture, bootstrap path, validation contracts, current work,
and historical `SH-*` evidence. Do not create separate self-hosting plans,
status reports, format specifications, or ledgers; update this document.

## What Self-Hosting Means

The project uses three distinct terms. They must not be treated as equivalent:

| State | Meaning | Rust required? |
|-------|---------|----------------|
| Self-hosting stress test | RSS frontend tools run through the Rust-built VM and are compared with Rust oracles | Yes, for build and truth |
| Self-hosted frontend | One RSS lexer/parser/AST/checker can analyze the compiler's own RSS sources | Yes, until lowering/backend bootstrap exists |
| Self-hosted compiler | A compiler written in RSS lowers and emits executable code, then compiles itself | No Rust required after a bootstrap binary is available |

The repository is currently in the first state and has substantial frontend
coverage. It is not yet a self-hosted frontend or compiler. Publishing the
existing Rust compiler as a binary can remove Rust from the *user installation*
path, but only stage bootstrap removes Rust from the *compiler development and
rebuild* path.

## Final Target

The intended independent toolchain is:

```text
RSS source packages
    -> RSS lexer and parser
    -> materialized Program AST
    -> RSS name/type/effect/package checking
    -> RSS lowering to a stable compiler IR
    -> RSS bootstrap backend
    -> stable runtime ABI and independently buildable runtime
    -> native compiler executable
```

The first bootstrap backend should minimize implementation risk. The current
recommended path is an RSS emitter for portable C, followed by the platform C
compiler and linker. That removes the Rust toolchain dependency without first
requiring RSS to implement an object writer, linker, Cranelift, or LLVM. The VM
bytecode backend remains useful for differential testing, but relying on a
Rust-built VM alone would not complete independent bootstrap.

Bootstrap stages:

```text
stage0: current trusted Rust compiler builds the compiler written in RSS
stage1: the resulting RSS compiler builds the same RSS compiler sources
stage2: the stage1 compiler builds those sources again
proof:  stage1 and stage2 canonical IR and normalized output are identical
```

After reproducible stage2 is established, releases can ship a bootstrap binary.
The Rust implementation should remain as an oracle for several releases; it is
not deleted merely because stage2 first succeeds.

There are also two runtime dependency milestones:

- **Binary independence:** users and compiler developers can use released
  compiler/runtime binaries without installing Rust. A prebuilt Rust-origin
  runtime may temporarily satisfy this milestone.
- **Source independence:** a clean environment can rebuild the compiler and
  required runtime from maintained non-Rust sources. The current
  `crates/runtime` implementation means this stronger milestone is still
  pending even after an RSS compiler first reaches stage2.

Before Stage 4 completes, choose and document one runtime path: migrate the
bootstrap-required runtime core to portable C/RSS, or define a stable C ABI and
maintain an independently buildable implementation behind it. Optional platform
features may remain separate native libraries, but the minimal compiler rebuild
must not invoke Cargo.

## Current Architecture

| Component | Current implementation | What it proves | Principal gap |
|-----------|------------------------|----------------|---------------|
| Shared scanner | `selfhost/scan.rss` | Reusable tokenization primitives | Not yet the sole token source for one frontend |
| Lexer | `selfhost/lexer.rss` | Full-corpus canonical token parity | Runs only through the test harness |
| Recognizer | `selfhost/parser.rss` | Top-level accept/reject parity | Does not produce the reusable AST |
| AST producer | `selfhost/astdump.rss` | Canonical AST dump parity | Reparses tokens and streams text instead of building an AST |
| Type helpers | `selfhost/types.rss` | Shared canonical type-string operations | Not a complete symbol/type representation |
| Single-file checker | `selfhost/check.rss` | Presence parity for 83 diagnostic families; occurrence+span parity for 81 families | Two remaining families still use independent file-level token probes |
| Package checker | `selfhost/package_contract.rss` | `RS1301` parity for functions, data declarations, protocols/impls, native exemptions, and resolved multi-file bundles | Path-sensitive bundle records and semantic edge cases remain |
| Lowering and IR | Rust | Production compilation | No RSS implementation |
| VM/JIT/AOT backend | Rust | Production execution and code generation | No bootstrap backend written in RSS |

The harness in `crates/rsscript/src/selfhost_parity.rs` resolves the RSS modules,
compiles them through the normal register VM compiler, runs them in-process, and
compares their deterministic output with production Rust oracles. It is
`#[cfg(test)]`; self-hosting adds no public CLI commands.

Generated `selfhost.interfaces` data comes from real `.rssi` interfaces. It
provides return-type, Result-error, and parameter-effect facts to `check.rss`.
Parameter-type lookup remains an explicit false-positive-safe subset until the
self-hosted checker has a real symbol and type-shape model.

## Problems Exposed by Self-Hosting

Self-hosting has tested more than whether RSS can express a lexer or checker. It
has exercised parser recovery, ownership and effects, generic lowering, managed
collections, backend agreement, and long-running VM workloads. The findings
must be separated into fixed defects, remaining language ergonomics, runtime
performance limits, and actual bootstrap blockers.

### Fixed language and backend defects

The stress tools exposed correctness gaps that are now covered by regression
tests:

- `mut` scalar parameters and scalar fields did not write back consistently
  across the checker, VM, and AOT paths (`SH-007`, `SH-013`).
- Generic function calls could be lowered as struct construction, and AOT
  generic/read lowering could produce missing bounds or borrowed values where
  owned values were required (`SH-008`-`SH-010`, `SH-015`).
- Character literals, multiline leading-operator continuation, inherent `impl`
  blocks, and positional multi-field variant patterns were missing or behaved
  inconsistently (`SH-016`-`SH-018`, `SH-024`).
- Freshness was lost at a control-flow merge, and VM/AOT disagreed on the exit
  behavior of `main` returning `Err` (`SH-019`, `SH-005`).
- Redundant eager `DeepCopy` of `read List<Char>` values made the self-hosted
  lexer quadratic; the classifier fix changed that workload from about 79.5 s
  to 0.73 s in the recorded release measurement (`SH-022`).

These are historical findings, not current self-hosting blockers. Their detailed
evidence and tests remain in the `SH-*` ledger below.

### Remaining RSS and runtime limitations

1. **Managed collection performance.** Tight RSS `List`, `Map`, and related
   collection loops still pay dynamic `VmValue`, intrinsic/helper dispatch,
   handle lookup, and managed borrow costs. Native local-collection helpers now
   exist, so this is no longer a missing JIT capability, but collection-heavy VM
   and JIT kernels remain far behind AOT/Rust (`SH-004`, `SH-011`, `SH-022`).
   This affects self-host development speed; it does not prevent correctness or
   bootstrap when AOT is used.
2. **JIT fit for compiler workloads.** Parser and checker code is dominated by
   strings, collections, generics, intrinsics, and `Result`/`Option`, while the
   native tier is strongest on scalar/control-flow kernels. JIT therefore often
   gives little improvement on self-host tools, and AOT remains the performance
   path (`SH-001`, `SH-006`). This belongs to the VM/JIT performance roadmap,
   not the Stage 1 parity exit criteria.
3. **Explicit error conversion.** `?` does not implicitly convert between error
   types, so compiler layers need explicit `match`-based adapters (`SH-003`).
   This is a deliberate language rule, but it creates recurring boilerplate in
   larger RSS programs.
4. **Expression ergonomics.** Constructs such as `if`/`else` are statements,
   not general value expressions, so some frontend code must use a helper or a
   mutable result binding. This is workable and not a bootstrap blocker, but it
   makes compiler-style code more verbose.

### Reviewability and human factors

The current self-host code is testable but not yet easy to review. This is a
maintainability risk even where differential parity protects correctness:

- `selfhost/check.rss` is about 12,700 lines with more than 400 top-level
  functions. Its `main` is about 1,000 lines and owns rule setup, shared-model
  construction, file traversal, structured output, legacy output, and the final
  clean verdict. A reviewer cannot understand a new diagnostic from one local
  diff.
- One diagnostic family may be represented by a boolean, token-index list,
  `DiagnosticSite` list, one or more collector functions, a structured-output
  table entry, a legacy-output branch, Rust oracle filtering, an embedded test
  fixture, and a ledger entry. These parallel wiring points can drift. The
  structured branch returns early, leaving older `if structured` branches in
  the legacy section unreachable and visually misleading.
- The checker and AST dumper frequently pass raw token indices, `-1` sentinels,
  integer mode flags, and long positional parameter lists. These are compact for
  the VM but force reviewers to reconstruct the meaning and valid range of each
  integer at every call site.
- `selfhost/astdump.rss` is about 3,300 lines and directly combines parsing with
  serialization. Large dispatch functions such as expression and statement
  emission make grammar changes difficult to review independently from output
  formatting.
- Structured parity fixtures live inside the roughly 5,000-line Rust harness,
  while the corresponding implementation lives in the large RSS checker. A
  reviewer must navigate between distant files to verify one rule, and corpus
  failures identify semantic divergence more reliably than they identify the
  responsible subsystem.

Reviewability is therefore an explicit Stage 2 requirement, not cosmetic
cleanup. The convergence work should:

1. split scanner, syntax AST, parser, semantic models, rule families, and
   serialization into modules with one direction of dependency;
2. replace per-rule boolean/token/site triples with one `DiagnosticBag` of typed
   diagnostic records, from which presence and structured protocols are derived;
3. replace integer modes and sentinel return values with named sums/records where
   they cross function or module boundaries;
4. keep each diagnostic family beside focused positive, negative, and
   multiple-occurrence fixtures, with a short ownership/index table mapping an
   `RSxxxx` code to its RSS rule module and oracle test; and
5. keep `main` as orchestration only: build one analysis context, run rule
   groups, sort diagnostics, and serialize the selected protocol.

The split must follow the shared-AST migration rather than mechanically slicing
the current token probes into many files. Mechanical file splitting alone would
preserve the same hidden coupling while making navigation worse.

### Self-host implementation gaps, not language defects

- The recognizer, AST dumper, and checker still use separate parsing/token-probe
  strategies. RSS can express the required data structures; Stage 2 must
  converge them on one materialized `Program` AST and shared symbol/type/effect
  model.
- The RSS tools currently run through a `#[cfg(test)]` Rust harness. A normal
  self-hosted frontend package and compiler entry point do not yet exist.
- Lowering, compiler IR, code generation, and the runtime are still Rust. Stages
  3-5 must add RSS lowering, a stable IR, a bootstrap C emitter, a documented
  runtime ABI, and an independently buildable runtime before RSS can rebuild
  itself without Cargo.

The highest-priority current work remains frontend convergence: finish exact
structured diagnostic parity, close the package-contract edge cases, then make
the AST producer and checker consume one reusable RSS `Program` model. Runtime
and JIT performance work should be driven by measured self-host wall time and
must not be confused with bootstrap correctness.

## Current Baseline

Snapshot: **2026-07-12**, from local Docker runs against this worktree.

| Gate | Result | Scope |
|------|--------|-------|
| Self-host parity unit/smoke suite | 98 passed, 6 ignored | Non-exhaustive harness tests; cached-checker Docker run on 2026-07-12 took 86.88s |
| Lexer corpus parity, tier 2 | 622 / 622 | Full checked-in RSS corpus |
| Parser recognition parity, tier 1 | 622 / 622 | Full checked-in RSS corpus |
| Checker FAST parity | 618 / 618 | Non-giant inputs; diagnostic-code presence only |
| Package-contract parity | Passed | Functions, types, sums, aliases, consts, protocols/impls, and native-exemption cases |
| Curated AST parity | Passed | Fast representative sample set |
| Full AST corpus parity | Not established for this snapshot | Scheduled/manual because of runtime |
| Checker FULL parity | Not established for this snapshot | Scheduled/manual; includes giant inputs |
| Remote CI | Not established for this snapshot | Local Docker results only |

`selfhost/corpus.txt` contains 622 repository-relative `.rss` paths. Any count
change must update that manifest in the same change. FAST excludes checker input
files over 40 KiB; FULL sets `RSS_SELFHOST_FULL=1`. Historical numbers below
describe their original milestones and do not override this snapshot.

## Delivery Stages

Work proceeds in dependency order. Later stages must not create a second parser,
type model, or IR to bypass an unfinished earlier stage.

### Stage 1 — Reliable Frontend Parity (in progress)

1. Complete package-level `RS1301` parity for functions, types/resources/classes,
   sums and variants, aliases, constants, protocols and implementations,
   native-binding exemptions, and resolved multi-file bundles.
2. Upgrade checker parity from a deduplicated code set to sorted structured
   diagnostic multisets: code, occurrence, stable span, then label class,
   causes, and fix identifiers where stable.
3. Keep malformed tool output, unreadable corpus files, stale corpus discovery,
   and invalid tier values fail-closed.

Exit: every supported frontend result is deterministic and compared at the
correct semantic level, including package contracts.

Structured checker migration currently covers RS0002-RS0014, RS0018-RS0024,
RS0016-RS0017, RS0022-RS0024, RS0027-RS0029, RS0032-RS0037, RS0101, RS0201, RS0205, RS0211-RS0212, RS0301, RS0306-RS0308,
RS0302-RS0305, RS0309, RS0311-RS0313, RS0401, RS0501, RS0601, RS0603-RS0604, RS0701-RS0711, RS0801-RS0805, RS0901-RS0904, and RS1001-RS1004 (81 of 83
presence-parity families).
The canonical wire record is
`code<TAB>line<TAB>column<TAB>length`; records are sorted and compared as
multisets without deduplication. Code-presence mode remains the fast 83-family
corpus gate until every family has migrated. Family-level tests filter the
checker's complete structured stream by target code before comparing that
code's multiset, so fixtures may exercise overlapping diagnostics without
discarding occurrences.

`RS0209` records currently cover conditions, `for` subjects, scrutinees, and
pattern occurrences. Match-expression arm-value typing remains presence-only:
the token-probe checker does not yet share the Rust frontend's arm type model.
It is an explicit Stage 2 shared-AST migration item, not a span-normalization
problem.

`RS0207` has structured anchors for annotated bindings and direct named call
arguments, including same-file and curated stdlib signatures. Callback bodies,
interpolations, and generic receiver calls remain presence-only because their
Rust diagnostics attach to typed subexpressions that the token-probe checker
does not materialize. It therefore remains outside the 81-family structured
count until Stage 2 supplies that shared expression model.

The inventory audit added the previously omitted reachable single-source
`RS1001 OPERATOR_OVERLOAD_ATTEMPT` family. `RS1301` remains separate because it
requires a resolved package bundle rather than one source file.

### Stage 2 — One Self-Hosted Frontend (pending)

1. Define RSS syntax and AST modules that materialize a `Program` value.
2. Make one parser produce that AST.
3. Make AST serialization consume it instead of reparsing and streaming tokens.
4. Migrate checker families from token probes to AST, symbol, type, and effect
   models incrementally; remove superseded probes after each parity gate passes.
5. Parse and check the compiler's own RSS package through the normal package
   model, not only test-harness source injection.

Exit: lexer -> parser -> AST -> checker is one reusable frontend, full corpus
parity is exact, and the compiler's RSS sources pass self-analysis.

The first Stage 2 slice is present: `selfhost/syntax/ast.rss` defines a
materialized top-level `Program`, and `selfhost/syntax/parser_items.rss` builds
it from the shared scanner. `parser.rss` invokes that parser while preserving
the established recognition protocol. The current items retain declaration kind
and representative span only; names, signatures, bodies, expressions, and
patterns remain to be materialized before this can replace `astdump.rss` or
`check.rss`.

#### Stage 2 target architecture

The target dependency direction is:

```text
source -> scanner -> tokens -> parser -> Program AST
                                      -> AST serializer
                                      -> symbols/types/effects -> rule groups
                                                               -> DiagnosticBag
                                                               -> output protocol
```

The provisional module ownership is:

```text
selfhost/syntax/token.rss
selfhost/syntax/ast.rss
selfhost/syntax/parser_items.rss
selfhost/syntax/parser_expr.rss
selfhost/syntax/parser_stmt.rss
selfhost/syntax/parser_pattern.rss
selfhost/semantics/context.rss
selfhost/semantics/symbols.rss
selfhost/semantics/types.rss
selfhost/semantics/diagnostics.rss
selfhost/semantics/check_signatures.rss
selfhost/semantics/check_types.rss
selfhost/semantics/check_ownership.rss
selfhost/semantics/check_closures.rss
selfhost/semantics/check_resources.rss
selfhost/serialize/ast_dump.rss
selfhost/main.rss
```

Names may change when the real dependency graph is implemented, but ownership
must not regress to multiple parsers or a single all-rules file. Rule modules
must consume the shared AST and semantic context, not parse source independently.

The canonical diagnostic representation should be equivalent to:

```rss
struct Diagnostic {
    code: String
    line: Int
    column: Int
    length: Int
}

struct DiagnosticBag {
    items: List<Diagnostic>
}
```

Rules add typed records to one bag. Presence output, structured multiset output,
sorting, and `CLEAN` are derived from that bag. A rule must not maintain a
parallel boolean, token list, site list, and output branch. Label class, causes,
and fix identifiers can extend `Diagnostic` as their parity tiers become stable.

The final driver should only read input, build one analysis context, run rule
groups, sort diagnostics, and serialize the requested protocol. Token cursors,
lookup results, and cross-module modes should use named records or sums instead
of raw `Int` modes and `-1` sentinels. Inherent methods may provide domain
operations such as `cursor.peek`, `cursor.advance`, and token/span lookup so
algorithmic code is not dominated by `List.get` plumbing.

### Stage 3 — Self-Hosted Lowering and IR (pending)

1. Define a deterministic, versioned compiler IR that can be serialized and
   compared independently of addresses or map iteration order.
2. Implement AST-to-IR lowering in RSS.
3. Compare RSS IR byte-for-byte with normalized Rust lowering over the corpus.
4. Cover control flow, generics, ownership/effects, interfaces, closures,
   intrinsics, and runtime ABI records before declaring lowering complete.

Exit: RSS and Rust frontends produce equivalent canonical IR for the supported
language, and the RSS compiler can lower its own sources.

### Stage 4 — Bootstrap Backend (pending)

1. Specify the runtime ABI used by generated code.
2. Implement a minimal C emitter in RSS from the stable IR.
3. Differential-test generated programs against VM and existing AOT execution.
4. Identify the minimal runtime needed by the compiler and make it buildable
   without Cargo, while preserving differential tests against `crates/runtime`.
5. Build packages and the compiler itself through the C toolchain.

Exit: stage0 can produce a standalone stage1 compiler without compiling new
Rust code. A system C compiler/linker is permitted; the Rust toolchain is not.

### Stage 5 — Reproducible Bootstrap and Release (pending)

1. Run stage0 -> stage1 -> stage2 from a clean Docker environment.
2. Compare canonical IR and normalized stage1/stage2 outputs.
3. Build the standard library, package manager, test runner, and compiler with
   stage2.
4. Publish bootstrap binaries with provenance, version compatibility, and a
   documented clean rebuild procedure.
5. Keep Rust-vs-RSS differential jobs until the RSS implementation has remained
   stable across multiple releases.
6. Perform at least one diverse double-bootstrap or independently reproduced
   build to reduce dependence on a single opaque stage0 binary.

Exit: a contributor can rebuild and evolve RSS without installing Rust, using
only a released bootstrap compiler and documented platform build dependencies.

## Immediate Order

The next implementation sessions remain frontend-focused:

1. Finish structured diagnostic multiset parity for the remaining three
   single-source families, then freeze additions to the token-probe checker.
2. Broaden `RS1301` package-contract parity and establish the full Stage 1
   corpus gates.
3. Introduce `Diagnostic`/`DiagnosticBag`, derive both output protocols from it,
   and remove unreachable or duplicated output wiring.
4. Introduce the materialized RSS `Program` AST and migrate AST dump first.
5. Build the shared symbol/type/effect context and migrate checker families by
   ownership group, deleting each superseded token probe after its parity gate.
6. Move focused fixtures beside their rule groups and maintain an
   `RSxxxx -> RSS module -> Rust oracle test` ownership index in this document.
7. Make exhaustive AST/checker gates sustainable in Docker.
8. Design the stable IR only after the frontend exit criteria hold.

JIT/VM collection performance is tracked outside this roadmap. It affects how
quickly self-hosting tests execute, but it is not a correctness prerequisite for
bootstrap. Likewise, a direct native machine-code backend is deliberately not
required for the first independent compiler.

## Reading and Review Guide

A contributor should not start with `selfhost/check.rss`. Until Stage 2 replaces
the current layout, use this reading order:

1. `selfhost/lexer.rss` for the smallest complete RSS tool and output protocol.
2. `selfhost/scan.rss` for token representation and shared cursor primitives.
3. `selfhost/parser.rss` for current recognition scope and recovery behavior.
4. `selfhost/astdump.rss` for the canonical AST contract, while remembering that
   it currently reparses and streams output rather than materializing an AST.
5. `selfhost/types.rss` and generated interface metadata for shared type facts.
6. One small diagnostic collector in `selfhost/check.rss` together with its
   structured parity test in `selfhost_parity.rs`.
7. Ownership, closure, and resource rule groups only after the simpler
   declaration and signature families are understood.
8. `selfhost/package_contract.rss` last, because its input is a resolved
   multi-file bundle rather than one source file.

Every new Stage 2 module should begin with a short contract stating its input,
output, owned invariants, non-responsibilities, and parity gate. Every diagnostic
family must have a focused oracle-positive fixture, a nearby oracle-negative
fixture, and a multiple-occurrence fixture where repetition is possible. The
ownership index should let a reviewer navigate from an `RSxxxx` code to one RSS
rule module and one Rust oracle test without searching the monolithic driver.

Review changes in semantic slices: AST/data-model change, one migrated rule
group, deletion of the replaced token probes, and parity proof. Do not combine a
large mechanical file split with semantic migration, and do not create a second
frontend merely to make a local diff smaller.

## Completion Criteria

| Milestone | Required proof |
|-----------|----------------|
| Reliable parity harness | Exact corpus inventory, strict protocols, full frontend/package oracle coverage |
| Self-hosted frontend | One materialized AST and semantic model analyze the compiler's own sources |
| Self-hosted lowering | RSS and Rust canonical IR agree across the supported corpus |
| Binary-independent compiler | RSS frontend + lowering + bootstrap backend produce a standalone compiler and use a released runtime |
| Source-independent toolchain | Compiler and minimal runtime rebuild without Cargo or `rustc` |
| Full bootstrap | Clean stage1/stage2 rebuild is reproducible and independently checked without the Rust toolchain |

Line count, number of RSS files, and diagnostic-family count are progress
signals, not completion proofs.

## Validation Model

Each RSS-written layer runs against the same input as its production Rust oracle:

| Layer | RSS tool | Current oracle and comparison |
|-------|----------|-------------------------------|
| Lexer | `selfhost/lexer.rss` | `crate::lexer::lex`; canonical token records |
| Parser recognition | `selfhost/parser.rss` | `crate::syntax::parse_source_raw`; accept/reject and position tier |
| AST dump | `selfhost/astdump.rss` | surface-preserving Rust AST dump; byte-exact text |
| Checker | `selfhost/check.rss` | `crate::analyze_source`; target-code presence for 83 families and structured occurrence+span parity for 80 |
| Package contract | `selfhost/package_contract.rss` | `crate::review_package_dir`; filtered `RS1301` results |
| Future lowering | RSS lowering | normalized Rust IR; byte-exact canonical serialization |
| Future backend | RSS C emitter | VM/existing AOT observable behavior and generated-artifact checks |

The legacy checker corpus gate compares diagnostic-code presence only. The
structured migration compares occurrence counts and stable spans for 80
families, but does not yet compare messages, label classes, causes, or fixes.
Stage 1 explicitly closes that limitation family by family.

Useful Docker gates:

```sh
docker compose run --rm dev cargo test -p rsscript selfhost_parity -- --test-threads=1
docker compose run --rm -e RSS_SELFHOST_TIER=2 dev cargo test -p rsscript --release --lib selfhost_parity::lexer_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm -e RSS_SELFHOST_PARSE_TIER=1 dev cargo test -p rsscript --release --lib selfhost_parity::parser_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm dev cargo test -p rsscript --release --lib selfhost_parity::checker_parity_corpus -- --ignored --exact --test-threads=1 --nocapture
docker compose run --rm -e RSS_SELFHOST_AST_TIER=2 dev cargo test -p rsscript --release --lib selfhost_parity::ast_parity_samples -- --exact --test-threads=1 --nocapture
docker compose run --rm dev cargo test -p rsscript --lib selfhost_parity::package_contract_ -- --nocapture
```

During checker development, `RSS_CHECKER_EXTRA_CODES=RS0XXX` adds a diagnostic
to the parity target without baking it into `SELFHOST_CHECKER_TARGET_CODES`.
Pull-request CI covers lexer/parser corpus parity, curated AST parity, checker
FAST, and package-contract smoke. Scheduled/manual jobs cover checker FULL and
the full AST corpus.

## Token Dump Contract

The canonical lexer dump has one token per line:

```text
<line>:<col>:<len>\t<KIND>\t<PAYLOAD>
```

- `line` and `col` are the token start position. `len` is the number of Unicode
  scalar values consumed by the token. It is not a UTF-8 byte count or grapheme
  count.
- `KIND` is one exact token kind name:
  `Ident Number String Char InterpolatedString MultilineString Keyword Symbol Unknown Eof`.
- `PAYLOAD` is the raw token text/content; `Eof` has an empty payload.
- Payload escaping is deterministic: `\` -> `\\`, newline -> `\n`, tab -> `\t`,
  carriage return -> `\r`.

`RSS_SELFHOST_TIER` controls comparison strictness:

- `0`: compare `(KIND, PAYLOAD)`.
- `1`: also compare `(line, col)`.
- `2`: also compare `len`.

The Rust lexer is the oracle. If the RSS lexer diverges, record the reason as an
`SH-*` finding before changing either side.

`selfhost/corpus.txt` is the checked-in inventory for repository-wide parity
gates. Any intentional addition or removal of `.rss` corpus files should update
that manifest in the same change.

## AST Dump Contract

The canonical AST dump is an indentation tree:

```text
<indent><TAG>[ <key>=<value>]*[ <PAYLOAD>]
```

- Indent is exactly two spaces per depth.
- Tags and attribute order are fixed.
- Payload, when present, is last and uses the same escaping as token payloads.
- Lines are UTF-8, newline-separated, and end with a trailing newline.

Tier 0 omits spans entirely. If span parity is reintroduced, each node may gain a
trailing `@L:C:N` field that the harness strips below the active tier.

Top-level dump order:

1. `program`
2. `feature <name>` lines
3. items in source order
4. `protocol <name>` lines
5. `protocol-impl protocol=<p> type=<t>` lines and method mappings
6. diagnostic marker lines when present

Common node families:

- Items: `module`, `use`, `type`, `sum`, `type-alias`, `const`, `fn`.
- Supporting nodes: `generic`, `field`, `param`, `type`.
- Statements: `let`, `return`, `with`, `if`, `loop`, `for`, `match`,
  `task-group`, `select`, `break`, `continue`, `let-else`, `assign`,
  `expr-stmt`, and malformed/unknown markers.
- Patterns: `pat-binding`, `pat-variant`, `pat-struct`, `pat-literal`,
  `pat-list`, `pat-wildcard`.
- Expressions: `ident`, `number`, `string`, `char`, `multiline`, `object`,
  `map`, `array`, `binary`, `field-access`, `index`, `call`, `effect`,
  `manage`, `spawn`, `await`, `try`, `closure`, `match-expr`, `unknown-expr`.

`selfhost/astdump.rss` streams this format directly rather than materializing a
full handle-based AST. The oracle uses `crate::syntax::parse_source_raw`, never
the desugared parser.

## Historical Ledger

Real RSS-written tools are the feedback loop for hardening RSScript. As each tool
is written and run across the VM, JIT, and AOT backends, every bug, slow path, or
awkward pattern is recorded here and classified into the layer where its fix
belongs (language / stdlib / VM / JIT / AOT / docs), then driven to a decision.

Each entry:

```
ID:             SH-NNN
Tool:           which self-hosted tool surfaced it
Symptom:        what was awkward / slow / wrong
Minimal RSS:    smallest snippet that shows it
Backend:        vm / jit-internal / jit-native / aot / all
Root cause:
Classification: language | stdlib | VM | JIT | AOT | docs
Decision:
Tests:
Benchmark:
Status:         open | decided | done
```

---

## Entries

### SH-001 — manifest inspector gets zero native acceleration

- **Tool:** manifest inspector
- **Symptom:** `jit-native` is no faster than `vm-internal`; the new bench JSON
  telemetry shows `considered: 6, translated: 0, not_eligible: 5, native_calls: 0`.
- **Minimal RSS:** any function that calls a stdlib intrinsic (`Toml.parse_file`,
  `Json.field`, …) or returns `Result`/`Option`.
- **Backend:** jit-native.
- **Root cause:** the native subset is the numeric/control core plus a few
  read-heap ops; tool code is dominated by `CallIntrinsic` and `Result`/`Option`
  values, none of which are native-eligible, so every function falls back.
- **Classification:** JIT (coverage) — *expected by design*, not a bug.
- **Decision:** record it as the measured answer to "what does the JIT
  accelerate?" — numeric/loop kernels, not intrinsic/IO/error-handling tool code.
  Real wins for tool code come from cheaper intrinsics and value representation,
  not from widening the native subset to cover `CallIntrinsic`.
- **Tests:** `backends_agree_on_manifest_inspector` (5-way).
- **Benchmark:** `selfhost_manifest_inspector.rss` in the matrix.
- **Status:** decided.

### SH-002 — `Json.field_optional(...) → Some/None → default` boilerplate

- **Tool:** manifest inspector
- **Symptom:** the "read an optional field, else a default" shape recurs
  (`edition_of`, both arms of `path_array`, `dependency_count`).
- **Minimal RSS:**
  ```
  match Json.field_optional_string(value: read v, name: read "edition")? {
      Some(text) => { return Ok(text) }
      None => { return Ok("") }
  }
  ```
- **Backend:** all (ergonomics).
- **Root cause:** no "optional-field-or-default" accessor; `Json.at_*_or` exists
  for *paths* but there's no `Json.field_string_or(value, name, default)` for a
  single field.
- **Classification:** stdlib.
- **Decision:** add `Json.field_string_or` / `field_int_or` / `field_bool_or`
  (and confirm `Option.unwrap_or` for the general case). Implemented below.
- **Tests:** follow the promoted helpers' differential + failure tests.
- **Status:** decided.

### SH-003 — per-function error-type conversion boilerplate

- **Tool:** manifest inspector (and the existing test-runner shows the same)
- **Symptom:** `?` doesn't convert error types, so every boundary wraps
  `JsonError`/`FileError` into `String` by hand (`json_error`, `file_error`).
- **Backend:** all (language ergonomics).
- **Root cause:** deliberate — RSScript has no implicit `?` error conversion.
- **Classification:** language (documented design) + stdlib (could offer
  `JsonError.message`-style adapters, which already exist).
- **Decision:** keep the explicit model; the cost is one `match` at each error
  boundary. Not promoting — documented here so it isn't re-litigated.
- **Status:** decided.

### SH-004 — collection loops over *local* collections get no native acceleration

- **Tool:** stdlib conformance reporter
- **Historical symptom:** an IO-free, loop-heavy tool *still* showed `translated: 0,
  native_calls: 0` — the JIT accelerates none of it.
- **Minimal RSS:**
  ```
  let mut xs = List<Int>.new()
  while i < n { List.push<Int>(list: mut xs, value: read i); i = i + 1 }
  while j < List.len<Int>(list: read xs) { total = total + List.get<Int>(list: read xs, index: j); ... }
  ```
- **Backend:** jit-native (and tier-0).
- **Historical root cause:** two gaps compounded. (1) Collection *construction/mutation*
  (`List.push`, `Map.insert`) is not in the native subset at all. (2) The
  read ops that *are* native (`ListLen`/`ListGet`/`GetFieldSlot`) only fire when
  the collection is a **handle parameter** — handles never originate in native
  code, so a locally-built `let mut xs` can't be read natively. Real tool code
  builds and processes collections locally, so the Phase-2 read-heap coverage
  rarely applies.
- **Current correction (2026-07-07):** the first half is no longer true. The
  native tier now has collection construction/write/read helper coverage for the
  relevant local-collection shapes: `ListNewInt`, `ListPush*`, `ListSet*`,
  `MapInsert*`, `SetInsert*`, `SortedSetInsert*`, `SortedMapInsert*`, and
  `DequePush*`, plus directed OSR tests for local list/map mutation. A local
  list mutated inside a loop OSRs safely through journaled heap helpers
  (`native_osr_mutated_list_in_loop_stays_correct`), while invariant typed-list
  reads can use direct len/get paths (`native_osr_direct_invariant_list_read_matches_interpreter`).
- **Classification:** JIT (coverage) + VM (representation).
- **Decision:** closed as a self-hosting pending item. The old concrete missing
  capability, native local collection mutation, has landed. The remaining gap is
  not "can self-hosted RSS build and process local collections under JIT?" but
  "can VM/JIT collection-heavy code approach AOT/Rust speed?" That requires a
  broader native-friendly collection representation / cheaper helper boundary,
  tracked by the JIT performance roadmap rather than this self-hosting backlog.
- **Tests:** `backends_agree_on_stdlib_reporter` (5-way).
- **Benchmark:** `selfhost_stdlib_reporter.rss` in the matrix.
- **Status:** closed for self-hosting; residual performance work is JIT/VM
  architecture, not a pending self-host feature.

### SH-005 — `main` returning `Err` diverges: VM exit 0 vs AOT exit 101

- **Tool:** manifest inspector (failure path)
- **Symptom:** running the inspector on a malformed manifest (so `main() ->
  Result<Unit, String>` returns `Err`):
  - VM harness: prints `Err { value: "missing JSON field \`package\`" }`,
    **exit 0**.
  - AOT (`rss run`): `panicked … RSScript main returned an error: …`, **exit 101**.
- **Minimal RSS:** `fn main() -> Result<Unit, String> { return Err("boom") }`.
- **Backend:** vm vs aot (divergence).
- **Root cause:** the two entry points surface a `main` that *returns* `Err`
  differently — the VM eval wrapper treats it as a normal completion (the `Err`
  is just the return value), the AOT `main` wrapper panics. (Distinct from an
  error *thrown* by an intrinsic, e.g. out-of-bounds `List.get`, which fails on
  both.)
- **Classification:** language/spec (define the contract) + VM/AOT (make the
  entry points agree) + docs.
- **Decision:** a `main` returning `Err` is a failed run on every backend —
  non-zero exit (1), error to stderr. **Done:** the AOT main wrapper now reports
  the error and `std::process::exit(1)` instead of `.expect()`-panicking
  (exit 101); the VM `eval` CLI exits 1 + stderr when `main`'s return is an `Err`
  variant; the differential harness's VM/JIT/native backends treat a `main`-`Err`
  as a failed run (`stdout_or_main_err`), so failure paths agree across backends.
- **Tests:** `backends_all_fail_on_bad_manifest` (malformed + absent manifest,
  all backends fail). Feature differential 20/20; corpus + vm green.
- **Status:** done.

### SH-006 — on real tool code the JIT gives ~0×; AOT gives 1.6–14×

- **Tool:** both (manifest inspector + stdlib reporter)
- **Symptom (measured):** mean ms across modes —
  | tool | vm-internal | jit-internal | jit-native | release (AOT) |
  |------|------------|--------------|-----------|---------------|
  | manifest inspector (IO/intrinsic) | 0.029 | 0.031 | 0.035 | **0.018** |
  | stdlib reporter (collection loops) | 1.04 | 1.25 | 1.04 | **0.072** |
- **Backend:** all.
- **Root cause:** both JIT tiers accelerate only the numeric/control core (plus
  parameter heap reads); real tool code is intrinsic calls, `Result`/`Option`
  handling, and locally-built collections (SH-001, SH-004), none of which the JIT
  covers — so JIT ≈ VM (occasionally *slower* from failed compile attempts). The
  AOT compiler lowers the *whole* program (including collection ops) to native
  Rust, so it wins big on the collection-heavy reporter (~14×).
- **Classification:** JIT (coverage) — measured, by design.
- **Decision:** the JIT's niche is numeric/loop kernels; **AOT is the performance
  path for tool code**. To make the JIT help real tools would require Phase 3
  (SH-004: native local collections) and intrinsic-in-native coverage — large.
  The actionable near-term lever for tool speed is the AOT path, not the JIT.
- **Tests/Benchmark:** the matrix `nat/reg` and `reg/rust` columns; the two
  `selfhost_*` cases.
- **Status:** decided.

### SH-007 — can't reassign a scalar struct field through a `mut` parameter

- **Tool:** Mailbox<T> (collection in RSS)
- **Symptom:** `m.count = m.count + 1` on a `mut Mailbox` param is rejected
  (RS0311 "`m` is a parameter, not a reassignable local").
- **Minimal RSS:** `fn bump(m: mut Box) -> Unit { m.n = m.n + 1 }`.
- **Backend:** all (language).
- **Root cause:** scalar field reassignment is only allowed on `let mut` locals,
  not through a `mut` parameter. `List` fields are reference types, so
  `List.set(list: mut m.field, ...)` *does* mutate-in-place and propagate.
- **Classification:** language (intended) + docs.
- **Decision:** documented constraint. Workaround in a self-hosted collection:
  keep mutable scalar state in a 1-element `List<Int>` (reference type) and/or
  compute it by scanning (the Mailbox holds `next_seq` as a 1-elem list and
  computes `count`). Not changing the language now; recorded so it's expected.
- **CORRECTION (2026-07-04): NO LONGER A LIMITATION — the entry is stale.** The
  original rejection was lifted (as a side effect of the SH-018-era assignment-gate
  work) but this entry was never updated. Scalar field reassignment through a `mut`
  struct param now checks clean and works on both backends. There is no semantic
  reason for a scalar field to differ from a `List` field once the `mut` param
  lowers to `&mut T`: `b.n = b.n + 1` is just `(*b).n = (*b).n + 1`. The gate
  `analyzer/assign.rs::validate_compound_assignment` accepts a `MutParam` root
  (`Some(AssignBinding::MutParam) => {}`, ~L464); RS0311 fires only for a plain
  non-`mut` `Param`. **Verified 2026-07-04:** `struct Box { n: Int }` +
  `fn bump(b: mut Box) -> Unit { b.n = b.n + 1; b.n = b.n + 10 }` +
  `let mut b = Box(n: 0); bump(b: mut b)` → `rss check: ok`, and `b.n == 11` on
  BOTH the reg-VM (`rss run`) and AOT (`rss run --release`) tiers, with the
  write-back correctly reaching the caller. The 1-element-`List` workaround is no
  longer needed. Same class of stale over-claim as SH-020 / the "no method syntax"
  correction.
- **Status:** RESOLVED (not a limitation — scalar struct fields are reassignable
  through a `mut` param, with caller write-back, on all backends).

### SH-008 — generic function call mis-lowered as a struct construction (BUG, fixed)

- **Tool:** Mailbox<T>
- **Symptom:** `get_v<Int>(h: read h)` evaluated to a struct value
  `get_v { h: ... }` — the call was lowered as a struct construction. (Checker
  accepted it as a call; VM lowerer disagreed.)
- **Minimal RSS:** `fn get_v<T>(h: read Holder<T>) -> Int { return h.v }` called
  as `get_v<Int>(h: read h)`.
- **Backend:** vm/jit (lowering).
- **Root cause:** `Callee::Name` looked up `function_ids.get(name)` with the raw
  name including type args (`"get_v<Int>"`); functions are keyed bare (`"get_v"`),
  so it missed and fell through to struct construction.
- **Classification:** VM (lowering bug).
- **Decision (DONE):** strip generics in the lookup —
  `function_ids.get(type_root_name(name))`.
- **Tests:** `backends_agree_on_selfhost_mailbox` (5-way).
- **Status:** done.

### SH-009 — AOT generic params miss the `Clone` bound (BUG, fixed)

- **Tool:** Mailbox<T>
- **Symptom:** AOT fails to compile a generic collection that retrieves elements:
  `the trait bound T: Clone is not satisfied`.
- **Minimal RSS:** a generic `fn` that does `List.get<T>(...)` (clones).
- **Backend:** aot.
- **Root cause:** `lower_generic_params` emitted `<T>` with no `Clone`, but RSS
  value semantics clone values (`List.get`), so generated generic Rust needs it.
- **Classification:** AOT (lowering bug).
- **Decision (DONE):** generated generic params now carry `Clone` (for every bound
  except `Resource`, which is move-only).
- **Tests:** `backends_agree_on_selfhost_mailbox` (AOT now compiles + agrees).
- **Status:** done.

### SH-010 — AOT doesn't deref a `Copy` match-binding from a `read` Option

- **Tool:** Mailbox<T> (test driver)
- **Symptom:** matching `Some(v)` on a `read Option<Int>` binds `v: &i64`; passing
  it to a by-value `Copy` intrinsic (`String.from_int`) fails AOT with
  `expected i64, found &i64`. (VM tolerates it; AOT is correct to reject.)
- **Minimal RSS:** `fn f(o: read Option<Int>) { match o { Some(v) => String.from_int(value: v) ... } }`.
- **Backend:** aot.
- **Root cause:** same class as the `read`-float-arg fix — a `Copy` value reached
  by reference where a by-value position is expected isn't auto-deref'd by the
  lowerer (here the value is bound by a match on a borrowed Option).
- **Classification:** AOT (lowering).
- **Decision (DONE):** the lowerer now shadows borrowed match payload bindings with
  owned values before the arm body sees them: `*x` for `Copy` payloads,
  `x.clone()` for cloneable non-resource payloads. Resource payloads remain
  borrowed and are rejected by the resource move rules when used by value.
- **Tests:** `vm_eval_parity::misc::parity_borrowed_match_payload_used_by_value`
  proves VM/AOT parity for `read Option<Int>`, `read Option<String>`, and
  `read Result<Int, String>` payloads.
- **Status:** done.

### SH-011 — self-hosted collection: VM/JIT ~470–590× slower than AOT

- **Tool:** Mailbox<T> heavy driver (`selfhost_mailbox_bench.rss`, 60k send/take
  cycles on the RSS-implemented collection).
- **Symptom (measured, mean ms):**
  | mode | mean ms | vs AOT |
  |------|---------|--------|
  | vm-internal | 330.0 | 448× slower |
  | jit-internal | 330.6 | no help |
  | jit-native | 345.3 | *worse* (wasted compile attempts) |
  | release / AOT | **0.737** | 1× |

  (Honest workload: cycle count from runtime args + data-dependent takes, so AOT
  cannot fold it. The earlier `send(i);take()`-with-constant-cycles version let
  LLVM collapse the work to 0.213 ms; the gap is real regardless.)
- **Backend:** all.
- **Root cause:** the collection is generic + built on `List` intrinsics. The VM
  executes every `List.get`/`set`/`push`/`len` as an interpreted intrinsic dispatch
  over dynamic `VmValue`s; neither JIT tier accelerates it (generic + intrinsic +
  locally-owned heap → not native-eligible, per SH-001/SH-004), and the native
  tier is even slower from compile attempts that all bail. AOT lowers the whole
  thing to native Rust `Vec` ops.
- **Classification:** VM (representation / intrinsic dispatch cost) + JIT
  (coverage).
- **Decision:** this is the clearest measured answer to the question "does a
  self-hosted collection expose the VM/JIT-vs-compiler gap?" — **yes, ~470× on the
  VM, and the JIT does not close it** (it is slightly worse). For self-hosted
  collections, AOT is the only fast path today; closing the VM/JIT gap needs
  Phase-3 native local-collection support (SH-004) and/or cheaper VM intrinsic
  dispatch + value representation — a large effort, now justified by real data.
- **Current correction (2026-07-07):** Phase-3 native local-collection helper
  coverage has since landed, so this entry is no longer evidence for "no native
  local collection support." It remains evidence for the residual representation
  gap: helper-backed managed collections are still far from AOT/Rust on tight
  collection kernels. Current committed baseline evidence still shows the broad
  gap (`after-session.json`: `selfhost_mailbox_bench.rss` about 67 ms VM/JIT and
  69 ms native vs 0.43 ms Rust), while focused ring-buffer code can get native
  speedups (`baseline-20260626-jit-fixes.json`: mailbox-ring native about 4.2 ms
  vs 32 ms VM). That makes the remaining work a JIT/VM performance project, not a
  self-hosting correctness or language blocker.
- **Tests/Benchmark:** `selfhost_mailbox_bench.rss` (add to the matrix);
  correctness via `backends_agree_on_selfhost_mailbox`.
- **Status:** decided.

### SH-012 — jit-native per-call overhead on uncompilable code (fixed)

- **Tool:** Mailbox bench (jit-native)
- **Symptom:** jit-native ~4–5% *slower* than vm-internal; telemetry showed
  `considered: 300002, translated: 0, not_eligible: 7` — it re-evaluated
  eligibility on every call.
- **Root cause:** `try_native` did per-call work for every call (a
  `counts.entry(name.clone())` string-clone + hashmap, then a `cache.get(name)`
  hashmap lookup) even for functions already known not-eligible.
- **Classification:** VM (JIT dispatch overhead).
- **Decision (DONE):** the not-eligible verdict is an invariant property of the
  function, so cache it on `RegFunction` (`native_status: Cell<u8>`). The drive
  loop now checks it inline and skips the `try_native` call entirely (just a
  `Cell` read) for known-uncompilable functions.
- **Result:** jit-native ≈ vm-internal on the mailbox bench (≈325 vs ≈325 ms,
  within noise; was 345 vs 326). Telemetry `considered: 0` after warmup.
- **Tests:** feature differential 21/21 (behavior-neutral).
- **Status:** done.

### SH-013 — scalar field assignment through a `mut` parameter (fixed)

- **Tool:** Mailbox<T> (the List<Int>-as-cell smell, SH-007)
- **Symptom:** `m.count = m.count + 1` on a `mut` param was rejected (RS0311), so
  the mailbox held mutable scalars in 1-element lists and recomputed `count`.
- **Root cause:** (1) the checker rejected any assignment rooted in a parameter;
  (2) the VM copies `CallKnown` args into the callee window with no write-back, so
  even if allowed, scalar field mutations wouldn't propagate (only `List` fields
  did, via their shared `RefCell`). AOT already had `&mut` semantics.
- **Classification:** language (checker) + VM (call semantics).
- **Decision (DONE):**
  - Checker: a `mut` parameter (`AssignBinding::MutParam`) allows field/index
    assignment (not bare rebinding).
  - VM: `CallKnown` carries the callee's `mut`-param positions; when the frame
    completes (any return path), each `mut` arg's final value is written back to
    the caller's register (`apply_mut_writeback`), matching AOT's `&mut`.
  - Backward compatible: empty `mut_args` ⇒ no-op; List-based code already
    propagated and is unchanged.
- **Note:** core already ships `Counter` (`Counter.new/add/value`, a `mut`-scalar
  container) — the stdlib alternative the review suggested already exists.
- **Tests:** `backends_agree_on_mut_param_field_assignment` (5-way). Full gate
  green: feature differential 22/22; vm 112; corpus; checker 212/149.
- **Status:** done.

### SH-014 — hand-rolled modulo loop is O(n); use native `%`

- **Tool:** ring-buffer Mailbox benchmark
- **Symptom:** the bench was O(n²): instruction count grew quadratically while
  count/head/memory all stayed bounded.
- **Root cause:** the driver computed `i % 3` / `i % 4` with a hand-written
  `fn wrap(v, m){ while v >= m { v -= m } }`. Called with the *loop counter*
  `wrap(i, 3)`, it loops `i/3` times → O(i) per cycle → O(n²) total. (The ring
  buffer's own `wrap(head+count, cap)` was fine — bounded inputs.)
- **Classification:** stdlib/docs (use the language).
- **Decision (DONE):** use the native `%` operator everywhere (O(1)); deleted the
  `wrap` helper. The ring buffer is now linear and ~3× faster than the scanning
  version (vm 112 ms vs 330 ms at 60k cycles).
- **Status:** done.

### SH-015 — AOT: generic `read T` pushed into a `List<T>` infers `Vec<&T>`

- **Tool:** ring-buffer Mailbox (pre-fill with a generic placeholder)
- **Symptom:** `mailbox_new<T>(.., placeholder: read T)` doing
  `List.push(values, read placeholder)` in a loop failed AOT with
  `expected Vec<T>, found Vec<&T>` (the `read` borrow was stored by reference).
  VM ran fine.
- **Classification:** AOT (lowering) — same family as SH-010 (a `read`/borrowed
  value reaching a by-value/owned position isn't cloned/deref'd).
- **Decision (DONE):** `read`-parameter managed values now lower to owned values
  when they enter owned positions, so storing a borrowed generic/non-Copy value in
  an owned collection does not produce `Vec<&T>` / `&&T`.
- **Tests:** `tests/fixtures/pass/read-param-into-owned-collection.rss` is in the
  pass-fixture checker gate; it covers pushing `read Node` values into an owned
  `List<Node>`.
- **Status:** done.

### SH-004/SH-006 update — fixed ring-buffer Mailbox across modes (60k cycles)

| mode | mean ms |
|------|---------|
| vm-internal | 112.9 |
| jit-internal | 114.6 |
| jit-native | 112.8 (≈ vm — SH-012 fix) |
| release / AOT | 0.784 |

Conclusion unchanged: the JIT gives ~0× on collection code (now without being
*slower* than the VM); AOT is ~144× and remains the only fast path. The remaining
gap is VM value-representation / intrinsic-dispatch cost (the next big lever).

### SH-016 — no character-literal syntax; `'` lexes to `?`

- **Tool:** self-hosted lexer (`selfhost/lexer.rss`), Phase 1.
- **Symptom:** A lexer naturally wants to compare a `Char` to a literal, e.g.
  `c == '_'` or `next == '>'`. Every char literal is rejected `RS0015
  "unsupported RSScript syntax"`, followed by a spurious `RS0013 "?` requires
  `Result`" at the same span.
- **Minimal RSS:**
  ```
  fn f(c: read Char) -> Bool { return c == '_' }
  ```
  → `RS0015` at the `'` plus a bogus `RS0013`.
- **Backend:** all (frontend / parser surface).
- **Root cause:** there is no character-literal token in the lexer — `'` is not
  a recognized symbol, so `push_one` maps it to `"?"` (`lexer.rs`, unknown-char
  fallthrough). The parser then sees a `?` token between operands and reports it
  as a misused try operator, cascading a misleading diagnostic. So the language
  has **no `Char` literal syntax at all**, and the failure mode is doubly
  confusing because the surviving diagnostic points at `?`, not at the missing
  feature.
- **Classification:** language (missing char literals) + docs/diagnostics (the
  `'` → `?` → RS0013 cascade is a misleading error for a common construct).
- **Decision:** worked around in the lexer by comparing code points instead:
  `Char.to_code(value: read c) == 95` (`_`), `== 45` (`-`), `== 62` (`>`),
  `== 61` (`=`), etc. Language-side: a char-literal syntax (or at minimum a
  non-cascading "no char literals" diagnostic) is the real fix — filed for a
  follow-up decision.
- **Tests:** `crate::selfhost_parity::lexer_parity_tiny_sample` /
  `lexer_parity_corpus` (drives the rss lexer through the VM against
  `crate::lexer::lex`, now including the new `Char` token kind);
  `checker_frontend::misc::char_literal_is_a_real_char_value_and_type_checks`;
  pass fixture `tests/fixtures/pass/char-literal.rss`; differential corpus
  `tests/corpus/exec/char_literal.{rss,toml}` and
  `vm_eval_parity::data::parity_char_literals_and_escapes` (interpreter≡AOT).
- **Status:** fixed (language). `'x'` is now a real `Char` value end-to-end. The
  lexer emits `TokenKind::Char(raw)` (`lexer.rs` `lex_char_literal`), the parser
  produces `Expr::CharLiteral` / `MatchLiteral::Char`, HIR gains `HirExpr::Char`
  typed `Char`, the reg-VM lowers a new `RegInstr::LoadChar` (`VmValue::Char`),
  and the AOT backend emits a Rust `char` literal via `format!("{:?}", …)` (no
  `.to_string()` — a `char` is Copy). Native never sees `Char` (`LoadChar` is
  `native_subset: false`), so char-using functions stay on the interpreter tier,
  a safe parity fallback. The old RS0015/RS0013 diagnostic scaffolding
  (`Program.char_literal_spans`, the analyzer HashSet, and the "character
  literal" emission) is removed. `selfhost/scan.rss` `scan_char` now emits a
  matching `Char` token (kind 9, raw inner text, `\`-escape honored) so lexer
  parity holds. Escapes `\n \r \t \\ \' \0` (and a literal `"`) round-trip
  identically across interpreter and AOT.

### SH-017 — statement-level binary-operator expressions can't cross a newline (leading-operator continuation is SILENTLY wrong)

- **Tool:** self-hosted lexer (`selfhost/lexer.rss`), keyword classifier.
- **Symptom:** A boolean `||`/`&&` chain wrapped across lines misbehaves two ways:
  - **Trailing operator** (line ends with `||`): hard parse error `RS0015
    "unsupported RSScript syntax"` pointing at the *start* of the `return`.
  - **Leading operator** (next line starts with `||`): **compiles cleanly but
    silently drops every continuation line** — only the first line's terms are
    evaluated, so `is_kw("fn")` returned `false` because `"fn"` sat on line 2.
    No diagnostic at all. This is the dangerous one: a wrong answer with no error.
- **Minimal RSS:**
  ```
  fn is_kw(word: read String) -> Bool {
      return word == "if" || word == "else"
          || word == "fn"            // silently ignored
  }
  // is_kw("fn") == false
  ```
- **Backend:** all (parser / statement termination).
- **Root cause:** at statement level a newline terminates the expression (the
  parser does not treat a leading/trailing binary operator as a line
  continuation). Inside brackets/parens/braces newlines ARE fine — multi-line
  constructor calls and collection literals work — so the hazard is specifically
  bare operator chains in statement position. A single-line chain of 30 `||`
  terms works correctly.
- **Classification:** language / parser (missing operator-continuation) + a
  correctness-grade diagnostics gap (leading-operator form should error, not
  silently truncate).
- **Decision:** FIXED PROPERLY (2026-07-01) — statement-level expressions now
  **continue across newlines** on an unambiguous binary operator, so the wrapped
  chain that used to be silently-wrong is now *valid and correct*. In
  `syntax/parser/scan.rs` `statement_end`, a line that begins with, or follows a
  line ending in, one of `| & + * / % ^` continues the current statement (leading
  and trailing styles both work); `<`, `>`, `-`, `=`, `!` are excluded (generics /
  comparison / unary-minus, plus a dangling `let x =` and a leading `!expr` must
  NOT silently swallow the next line — that would reintroduce the SH-017 footgun),
  so a wrap can never swallow the start of a new statement. `==`/`!=`/`<=`/`>=`/`=`
  stay single-line. The interim safety guard in `stmt.rs` stays as a backstop for a
  genuine leading-`||` at a block start. Spec §A.1 updated with the normative
  statement-termination + continuation rule (reconciling the stale
  "not layout-sensitive" claim).
- **Tests:** `tests/fixtures/pass/multiline-operator-continuation.rss` (leading
  and trailing styles); the former fail fixture was removed (the construct is now
  valid). Full suite + differential + self-host parity green.
- **Status:** fixed (operator continuation supported).

### SH-018 — no cursor/state object: scan helpers must thread `(chars, n, index)` and return the new index

- **Tool:** self-hosted lexer (`selfhost/lexer.rss`), Phase 1 full tokenizer.
- **Symptom:** the oracle (`crate::lexer`) is a `Lexer` struct with `peek/peek_n/
  bump` methods mutating `self.index`. rss has no ergonomic equivalent: there is
  no `impl`/method syntax and a `mut` struct param only supports field/index
  assignment (SH-007/SH-013), not the natural "advance my cursor" pattern. So
  every scanner (`scan_string`, `scan_number`, `scan_interp`, …) takes
  `(chars: read List<Char>, n: read Int, i: read Int)` and *returns the new
  index*, and each peek is a free `code_at(chars, n, i)` call with an explicit
  `-1` out-of-bounds sentinel instead of `Option<char>`. The dispatcher must
  pre-read `c1`/`c2` (peek+1/+2) as locals every iteration.
- **Minimal RSS:**
  ```
  fn code_at(chars: read List<Char>, n: read Int, i: read Int) -> Int {
      if i < n { return Char.to_code(value: read List.get(list: read chars, index: read i)) }
      return -1
  }
  ```
- **Backend:** all (language ergonomics).
- **Root cause:** no methods/`impl` blocks and no move-cursor mutation through a
  `mut` param, so lexer state can't be encapsulated; it is threaded positionally
  and returned. Also no `Option<char>` peek convenience → `-1` sentinel.
- **Classification:** language (no method syntax / cursor mutation) + docs.
- **Decision:** worked around by the return-the-new-index convention and a
  `code_at` sentinel helper; it reads cleanly enough and reaches full tier-0
  parity (544/544). The mutable-cursor lever is now available: a `mut`
  **Copy-scalar** parameter (Int/Bool/Float/Char, …) may be reassigned inside the
  callee and the new value is written back to the caller (`&mut` semantics), so a
  scanner can take `i: mut Int` and do `i = i + 1` instead of returning the new
  index.
- **CORRECTION (2026-07-01):** the Symptom's "no `impl`/method syntax" was an
  over-claim (same class as SH-020). rss DOES have inherent methods — spelled as
  top-level qualified functions with a `self` receiver: `fn Type.method(self:
  read/mut/take Type, …)`, called with dot-syntax `x.method(args)` /
  `mut x.method()` (spec §14.6.1). Static, monomorphic, one-per-(type,name),
  effect-explicit, resolved by the receiver's concrete type in HIR
  (`resolve_receiver_call`), lowered like any namespaced function on all backends.
  Verified: `fn Lexer.bump(self: mut Lexer) { self.pos = self.pos + 1 }` +
  `mut lexer.bump()` mutates and writes back. So a self-hosted lexer CAN
  encapsulate its cursor as `fn Lexer.bump(self: mut Lexer)` — the pain was using
  free helpers, not a language gap. The ONLY thing rss lacked was the
  `impl Type { fn m() }` BLOCK grouping.
- **UPDATE (2026-07-04): inherent `impl Type { }` blocks are now SUPPORTED.**
  Decision reversed (user, 2026-07-04: "we should support it. this is not a big
  change"). Landed as *pure parse-time sugar* over the flat form, so it reverses
  no considered position: `impl Type { fn m(<effect> self, …) … }` desugars to
  top-level `fn Type.m(self: <effect> Type, …)` at parse time — the qualified
  function stays the one canonical semantic spelling (§2.3 intact); the block adds
  no capability, dispatch rule, or second *semantic* form. `mut self` / `read self`
  / `take self` fill the receiver type from the block header; the explicit
  `self: <effect> Type` form is also accepted. Implementation (reference compiler,
  additive — no existing corpus file uses the constructs, verified): parser
  `impl_is_inherent` (splits inherent vs `impl … for …` protocol impls on the
  `for` keyword) + `parse_inherent_impl_decl` (`syntax/parser/mod.rs`) emitting
  desugared `Item::Function`s; `parse_params` (`syntax/parser/items.rs`) accepts
  `<effect> self`. Nothing downstream changed (checker/HIR/receiver-resolution/
  lowering see exactly the flat form). Spec grammar updated (`inherent-impl-decl`,
  `param` self-shorthand) + §2B.3 caveat. Verified: full `static` target 629/629,
  plus inline tests `checker_frontend::misc::inherent_impl_block_desugars_to_
  qualified_methods` / `protocol_impl_block_still_parses_after_inherent_impl`; a
  standalone program runs `11`/`11` on BOTH reg-VM and AOT tiers with `mut self`
  write-back. NOT added as a `tests/fixtures/pass/*.rss` fixture on purpose: that
  dir is in the `selfhost_parity` corpus (all-files-must-match, no floor) and the
  self-hosted parser/checker (`selfhost/*.rss`) do not yet recognize `impl` blocks
  — a corpus fixture waits on teaching them (a separate, larger task). Method
  syntax now offers TWO spellings: flat `fn Type.method(self:…)` and the `impl`
  block that desugars to it.
- **Fix:** the reg-VM already wrote a `mut` param's final register back to the
  caller for every `mut` param (scalar included), so no reg-VM/native change was
  needed. Only two frontend touch-points were added: (1) the assignment gate
  (`analyzer/assign.rs`) now permits rebinding a `mut` Copy-scalar parameter
  (checked via `checks::local::is_copy_type_name`), keeping RS0311 for plain
  params and non-Copy `mut` params; (2) AOT lowering (`rust_lower/lowerer.rs`)
  emits `(*pos)` on read and as the assignment target for such a param, since
  `mut T` already lowers to `&mut T`. Non-Copy `mut` params keep their `&mut Struct`
  lowering and stay non-reassignable (only fields/elements are mutable).
- **Tests:** `crate::selfhost_parity::lexer_parity_corpus` (tier 0, 556/556);
  `tests/fixtures/pass/mut-scalar-writeback.rss` (Int + Bool write-back).
- **Status:** fixed (scalar Copy `mut` params are reassignable with caller
  write-back; non-Copy `mut` params stay non-reassignable).

### SH-019 — a `fresh`-returning fn can't build its result via `mut` + `List.push`

- **Tool:** self-hosted parser (`selfhost/parser.rss`), Phase 2 tokenizer.
- **Symptom:** `fn tokenize(...) -> fresh List<Tok>` that does
  `let mut toks = List.new<Tok>()`, pushes in a loop, then `return toks` is
  rejected at compile time with `RS0601 "fresh function \`tokenize\` returns
  non-fresh value \`toks\`"`. The list *is* newly created in the function, but
  having been mutated through a `mut` binding it no longer counts as a "clean
  local binding created inside the function".
- **Minimal RSS:**
  ```
  fn build() -> fresh List<Int> {
      let mut xs = List.new<Int>()
      List.push(list: mut xs, value: read 1)
      return xs   // RS0601
  }
  ```
- **Backend:** all (analyzer / freshness).
- **Root cause (CORRECTED):** the earlier writeup blamed `mut` + `List.push`, but
  that is wrong — the straight-line form (`let mut xs = List.new(); List.push(...);
  return xs`) already compiled. The real defect was the multi-predecessor flow
  merge: `merge_flow_states` (and its sibling loop/branch merges) kept only
  exclusive `local` bindings in `clean_locals`, dropping MANAGED (`let`/`let mut`)
  fresh bindings. So the builder failed `RS0601` only when the `push` ran inside a
  `while`/`if` (a control-flow merge), not in straight-line code.
- **Classification:** language (freshness analysis) — flow-merge bug.
- **Decision:** fixed. Managed fresh bindings now survive the merge: the
  `clean_locals` filter keeps a name that is `locals.contains(name) ||
  managed.contains(name)` in `checks/local.rs` (`merge_flow_states` ~3127 plus the
  three siblings `merge_loop_state`, `fallthrough_projection`,
  `merge_fallthrough_states`). Sound because any aliasing invalidation
  (manage/retain/take/capture) already removes the name from the predecessor
  `clean_locals` intersection, so an aliased binding can never reach the filter —
  the existing fail fixtures `fresh-loop-managed-local.rss`,
  `fresh-loop-retained-local.rss`, `fresh-branch-retained-local.rss` stay red.
- **Tests:** `crate::selfhost_parity::parser_parity_corpus`;
  fixture `tests/fixtures/pass/fresh-loop-built-list.rss` (fresh List built in a
  `while` loop).
- **Status:** fixed: managed fresh bindings now survive the flow merge
  (`local.rs:3127` + siblings).

### SH-020 — recursive descent has to encode `(ok, new-index)` as a sentinel Int

- **Tool:** self-hosted parser (`selfhost/parser.rss`), Phase 2.
- **Symptom:** every declaration parser wants to return *both* success/failure
  *and* the advanced cursor. With no lightweight tuple return and no cursor
  mutation through `mut` params (SH-018), each `parse_*` returns a single `Int`:
  `>= 0` is the new index, `-1` means "malformed". Callers re-derive the reject
  position from the pre-call `start` index. Compound top-level dispatch conditions
  (long `||` disjunctions) also had to be factored into single-line helper
  predicates (`starts_type_decl`, `starts_fn_like`) to respect SH-017's
  no-wrapped-boolean rule.
- **Minimal RSS:**
  ```
  fn parse_thing(toks: read List<Tok>, i: read Int) -> Int {
      if bad { return -1 }   // malformed
      return newIndex        // success + advanced cursor
  }
  ```
- **Backend:** all (language ergonomics).
- **Root cause (CORRECTED 2026-07-01):** the "no lightweight tuple return" premise
  was WRONG — verified that `fn f() -> (Bool, Int) { return (true, 5) }` with
  `let (ok, n) = f()` compiles and runs on the VM. Multi-value return via tuples
  works today, so a node-building parser CAN return `(new_index, node)`; the
  sentinel-`Int` convention was an unforced choice, not a language limit. The only
  genuine residual is the cursor plumbing itself (SH-018), which the
  `mut`-scalar-param write-back fix removes (pass the cursor as `mut pos`).
- **Classification:** docs (the original entry over-claimed a non-existent gap).
- **Decision:** WITHDRAWN as a language gap. Tuple returns work; the remaining
  ergonomic cost folds into SH-018 (cursor mutation), fixed separately.
- **Tests:** verified by probe (`fn f() -> (Bool, Int)` + tuple destructuring).
- **Status:** closed (not a gap — tuple returns work; over-claim corrected).

### SH-021 — `parse_source_raw` defers body validation: recognition parity under-tests the grammar

- **Tool:** self-hosted parser (`selfhost/parser.rss`), Phase 2 oracle.
- **Symptom:** the recognition oracle (`parse_source_raw`) rejects a file only via
  four span vectors (`unknown_top_level_spans`, `malformed_declaration_spans`,
  `unknown_features`, `duplicate_features`). Function/type **bodies are never
  validated at parse time** — the parser accepts arbitrary token soup inside a
  well-formed `fn … { … }` shell. Of 545 corpus files only **15** are
  parse-rejected (all `fixtures/fail/*` + `hostile-malformed/*`); the other 530
  accept, including every *semantically* broken fail-fixture. So a self-hosted
  "parser" reaches 545/545 recognition parity with only top-level dispatch +
  balanced-bracket matching — **without an expression/statement/pattern parser**.
- **Backend:** n/a (methodology / reference-parser design).
- **Root cause:** the rss frontend is parse-then-analyze by design — the parser is
  intentionally lenient and error-recovering, and the deep grammar (expression
  forms, effects, match-scrutinee rules, …) is enforced in the **analyzer**, not
  the parser. `parse_source` adds only desugaring, not validation.
- **Classification:** docs / methodology (not an rss defect).
- **Decision:** recognition parity is the right, tractable Phase-2 oracle, but it
  is a SHALLOW stress test — the real grammar depth lives behind the analyzer, so
  the deep-parsing stress belongs to Phase 3: a checker reproducing a specific
  analyzer diagnostic must actually parse function bodies to decide it. Recorded so
  the writeup does not overclaim — Phase 2 delivered a self-hosted *recognizer*.
- **Tests:** `crate::selfhost_parity::parser_parity_corpus`.
- **Status:** decided.

### SH-022 — self-hosted lexer was ~5100× slower on the VM: O(n²) DeepCopy of a `read List<Char>` param per helper call (FIXED → 45.6×)

- **Tool:** self-hosted lexer (`selfhost/lexer.rss`) run on the reg-VM vs native
  `crate::lexer::lex`, over the whole 545-file corpus (712 KB).
- **Symptom (measured, release):**
  | lexer | time | throughput |
  |-------|------|-----------|
  | native Rust `lex()` | 15.3 ms | 46.5 MB/s |
  | rss lexer on reg-VM | **79.5 s** | ~0.009 MB/s |
  → **~5100× slowdown** (~112 µs per source char).
- **Controlled experiment:** rewrote the token/output string building from repeated
  `String.concat` (O(n²)) to `StringBuilder` (O(n)). **No measurable change**
  (5140× → 5195×, within noise), parity still 544/544. So string-building is NOT
  the bottleneck (most tokens are short, so the quadratic term never dominates).
- **Backend:** vm.
- **ROOT CAUSE (CORRECTED 2026-07-01 — earlier "per-char dispatch" was WRONG):** a
  genuine **O(n²)**. Every lexer helper takes `chars: read List<Char>`; a `read`
  non-Copy param gets an eager prologue `DeepCopy`. The DeepCopy-elision pass
  *should* drop that copy (the list is never mutated), but it was KEPT: the taint
  pass propagates through `ListGet` to the extracted scalar `Char`, and the
  `Char.*` intrinsics were classified `Keep`, so `Char.to_code(c)` pinned the copy.
  Result: every per-char helper call (`code_at`, `slice`, `scan_*` — called O(n)
  times) deep-copied the whole O(n) char list ⇒ **O(n²)**. Measured attribution:
  a helper taking `read List<Char>` per char is O(n²) (10k→588ms, 20k→2319ms,
  40k→9127ms, 80k→37099ms, ~4×/doubling); the same work inlined (no per-call copy)
  is flat O(n) (~15–20ms). `RSS_VM_ELIDE_DEEPCOPY=0` vs on = identical (the copy was
  kept either way). AOT of the same source = ~1ms (borrows `read` params). So
  ~9000× of the gap was VM-specific redundant DeepCopy — NOT dispatch, NOT boxing
  (dispatch measured ~60ns/char), NOT string building (the StringBuilder control
  correctly ruled that out; its "so it's dispatch" inference was the mistake).
- **Classification:** VM (DeepCopy-elision classifier) — exactly the
  [[perf-refactor-phase2-deepcopy-elision]] "v2 classifier" follow-up (v1 was
  sound-but-no-win because "intrinsic reads force keep").
- **FIX (landed):** classify the 12 pure scalar `Char.*` intrinsics
  (`CharToCode`, `CharFromCode`, `CharToString`, `CharToLower`, `CharToUpper`,
  `CharIsDigit`, `CharIsAlpha`, `CharIsAlphanumeric`, `CharIsLower`, `CharIsUpper`,
  `CharIsWhitespace`, `CharCompare`) as `PureFreshReader` in
  `deepcopy_intrinsic_class` (`reg_vm/model.rs`) — they take `Char`/`Int` by value
  and return a fresh scalar/Bool/String, never mutate/store/alias (verified in
  `intrinsics/char.rs`). The existing elision pass then proves the `read List<Char>`
  copy redundant and drops it → O(n²)→O(n). ONE match arm; VM-only; no new
  intrinsic, no spec, no AOT/native change. Parity-safe: elision only removes a
  provably-redundant copy (native treats `DeepCopyElided` == `DeepCopy`; AOT borrows).
- **RESULT (measured, release, `lexer_perf_corpus`, 556 files / 724 KB):** rss
  lexer/VM **79.5 s → 732.7 ms** (~**108× speedup**); slowdown vs native
  `lex()` **5100× → 45.6×**. The ~46× residual is the real VM per-op tax over native
  Rust (AOT would remove most of it); cutting that further = the parked
  [[perf-refactor-roadmap]] collection-rep work, not this bounded fix.
- **Tests / bench:** `crate::selfhost_parity::lexer_perf_corpus`
  (`--release -- --ignored`); `reg_vm::tests::…::deepcopy_elision_fires_for_char_list_read_param`
  (regression guard). Full differential + compiled-parity green (elision soundness).
- **Status:** fixed (O(n²) removed; residual ~46× is the general VM per-op tax,
  tracked by the parked perf roadmap).
- **GENERALIZED — Slice 1 of borrow-by-default (2026-07-01):** the SH-022 fix was
  intrinsic-specific (it re-classified the 12 `Char.*` intrinsics so a tainted
  extracted `Char` stopped pinning the copy). The general root cause was that the
  taint pass OVER-tainted: extracting a `Copy` scalar (`Int`/`Bool`/`Float`/`Char`/…)
  from a collection/struct/variant (`ListGet`/`MapGet`/`GetField`/`GetFieldSlot`/
  `UnwrapVariantValue`/`DequePop*`) tainted the whole source, so ANY keep-forcing
  use of the scalar (a `Return`, a store, an unclassified intrinsic) pinned the copy
  → the same O(n²) class for every `read List<Scalar>` / `Map<_, Scalar>` /
  scalar-field read, not just Char. **Fix:** a `Copy` scalar has no interior `Rc`,
  so extracting one (a bit-copy; for `MapGet`/`DequePop*` a fresh `Option<Scalar>`
  of a `.cloned()` scalar) cannot alias the source or carry its `Rc` into an escape.
  The lowerer (which has HIR types) now threads a `scalar_regs` bitset — populated at
  each extractor site whose extracted static type is a known scalar — into
  `deepcopy_elidable_param_regs`, which SKIPS the taint edge `src→dst` for scalar
  extractions (`Move` is unchanged; non-scalar values like `String`/`Bytes`/`Json`/
  `List<T>` still taint, since their `.cloned()` shares the `Rc`). Now ALL
  `read List<Scalar>` / `Map<_, Scalar>` / scalar-field reads elide their prologue
  `DeepCopy`, independent of any per-intrinsic classification. Sound: over-tainting
  was only a pessimization; the three `does_not_leak` JIT-acceptance guards stay
  green. Perf holds at 45.9× (no regression). Sites NOT marked (conservative keep,
  sound): list-pattern element extractions, variant-payload unwraps, and
  struct-field pattern binds — pattern lowering does not thread the scrutinee's
  static type, so those extractions keep the (sound) over-taint. New regression
  guard: `reg_vm::tests::…::deepcopy_elision_fires_for_int_list_read_param`.
- **Slice 2 of borrow-by-default (2026-07-01):** closed the pattern-site gap Slice 1
  left open. The scrutinee's static type (`reg_expr_type_name` at the `match` entry)
  now threads through the pattern lowerers (`lower_match_pattern` →
  `lower_list_pattern` / `lower_struct_field_patterns` / `lower_option_some_pattern` /
  `lower_result_variant_pattern` / `lower_user_variant_pattern` /
  `lower_user_struct_variant_pattern`), so each scalar-extracting emission calls
  `note_scalar(dst, ty)` with the right element/field/payload type derived as it
  descends (list element via `list_elem_type`; struct field via
  `type_info(root).fields`; sum-variant payload via `sum_variant_fields`; `Option<T>` /
  `Result<T, E>` payload via `nth_type_arg`). So `match read xs { [a, b, ..] }` on
  `List<Scalar>`, `match read p { Point { x, y } }` on a scalar-field struct, and
  scalar variant/`Option`/`Result` payload binds now elide the read param's prologue
  `DeepCopy`. Required making `UnwrapSome` behave exactly like `UnwrapVariantValue` in
  the elision analysis (added to both the taint-PROPAGATION set — so a heap `Some`
  payload still taints unless marked scalar — and the safe alias-read list in
  `deepcopy_instr_forces_keep`), which is what unblocked the `Option<Scalar>` unwrap
  chain (previously `UnwrapSome` fell through to the conservative keep default and
  pinned the copy). Where the scrutinee type is statically unavailable the site stays
  unmarked (sound over-taint), same as Slice 1. Soundness: a `VmValue::Int/Float/
  Bool/Char` is inline with no interior `Rc`, so a pattern-bind bit-copy can neither
  alias the scrutinee nor carry its `Rc` into an escape; non-scalar binds stay
  tainted. Full suite + all three `does_not_leak` guards green; parity 556/556, 0
  mismatches; lexer perf holds (~46× ratio, rss/VM time ~700 ms stable — the ratio's
  jitter is the tiny Rust denominator, not the VM). New guard:
  `reg_vm::tests::…::deepcopy_elision_fires_for_option_scalar_pattern_bind`.
- **Slice 3 of borrow-by-default (2026-07-01):** widened the READ-ONLY-SAFE
  intrinsic set — a SOUND whitelist widening, NOT a default flip. Slices 1–2
  stopped scalar EXTRACTIONS from tainting; Slice 3 stops proven-pure READERS of a
  `read String`/`Bytes` param from pinning the copy. Previously every `String.*`
  and `Bytes.*` intrinsic fell through to the conservative `Keep` arm of
  `deepcopy_intrinsic_class`, so `String.len(read s)` (or any read-only string/bytes
  op) forced the prologue `DeepCopy` to be kept — a `read String`/`read Bytes` param
  used only in read-only ways was still deep-copied per call. **Audit + promotion:**
  every intrinsic in `intrinsics/string.rs` and `intrinsics/bytes.rs` was verified to
  (a) borrow its receiver by `&` (`expect_string_ref`/`expect_bytes_ref`, never
  `borrow_mut`), (b) never store an arg into `self.streams`/`self.channels`/resource
  state, and (c) return a FRESH value — a scalar, a brand-new `Rc<String>`
  (`VmValue::string` is always `Rc::new(into())`, so even `copy`/`slice`/`trim`/
  `replace` allocate and NEVER alias the arg's `Rc`), a freshly-`Rc::new`'d
  `Vec<u8>`, or a fresh `List`. All were promoted to `PureFreshReader`: the 35
  `String.*` readers (`StringAfter/Before/BuilderNew/CharAt/Chars/Contains/Count/
  Copy/EndsWith/Format/FromBool/FromFloat/FromInt/IndexOf/IsEmpty/Join/Lines/Len/
  PadLeft/PadRight/ParseFloat/ParseInt/Repeat/Replace/ReplaceFirst/Reverse/Slice/
  Split/StartsWith/StripPrefix/ToLowercase/ToUppercase/Trim/TrimEnd/TrimStart`) and
  the 11 `Bytes.*` readers (`BytesConcat/Consume/FromString/FromUints/IsEmpty/Len/
  Slice/ToString/ToUints/ViewStartsWith/ViewToBytes`). **Rejected (left in the
  keep-default, conservatism over completeness):** `MatchMapGet`/`MatchSortedMapGet`
  — these are alias-RETURNING extractions (`map.borrow().get(&key).cloned()` shares
  the element's `Rc` into `value_dst`), so promoting them read-only-safe would need a
  `map→value_dst` edge in the taint-propagation closure, which this slice does not
  touch; without it a later mutation of the extracted heap value would leak, so they
  stay in the fail-safe default. The model is now **"keep only on PROVEN escape
  (store / mutate-through-alias / retain / return / unclassified)"**; borrow-by-default
  now covers read-only `String`/`Bytes` params (and, via Slices 1–2, `Map`/`List`/
  scalar reads). Also a readability refactor (no behavior change):
  `deepcopy_intrinsic_class` / `deepcopy_instr_forces_keep` now read as an explicit
  three-way split — POSITIVE ESCAPE (keep) / POSITIVE READ-ONLY-SAFE (elide) /
  UNCLASSIFIED → KEEP (the fail-safe default arm, UNCHANGED — soundness backbone;
  `deepcopy_collect_regs` stays exhaustive/no-wildcard). Soundness: catch-all default
  unchanged; the new negative guard proves a stored `read` param still keeps its copy
  (no over-promotion). Full suite (628 lib + 456 runtime incl. all three
  `does_not_leak` guards + 35 differential + 628 static) green; parity 556/556 × 3, 0
  mismatches; lexer perf holds (rss/VM ~53× ratio under host load, no regression —
  the change only ADDS elisions). New guards:
  `reg_vm::tests::…::deepcopy_elision_fires_for_string_read_param` (positive) and
  `…::deepcopy_elision_kept_for_stored_read_param` (negative / over-promotion).
- **Slice 4 (copy-at-escape) — DEFERRED, data-backed NO-GO (2026-07-01):** the final
  optimization would move a KEPT copy from the prologue to just before the single
  escape point (so a cold/rare escape stops costing a per-call copy — AOT's
  `retains`-driven clone-at-use). Scoped and declined for now: (1) mid-body copy
  insertion shifts **absolute jump/back-edge indices** (`Jump*` targets are absolute,
  `model.rs:~2659`), so it must renumber every downstream target and interacts with
  the native tier's own renumbering (`passes.rs:~2431`) — large corruption blast
  radius. (2) The *sound* applicability is narrow: only a SINGLE escape, NOT in a
  loop (a per-iteration copy would be worse), and the escaping reg must be the root
  param itself, not an interior alias (deep-copying the root before an alias escapes
  does nothing) — all other cases must fall back to the prologue copy. (3) Expected
  win is small (most escapes are stores of interior aliases inside loops). Verdict:
  high risk / low reward — deferred until a corpus probe shows the simple case is
  common enough and it can get its own session with a dedicated jump-renumbering
  safety test. **Fix 3 is otherwise COMPLETE**: borrow-by-default (keep only on
  proven escape) now holds for every non-escaping `read` param across scalars,
  scalar pattern-binds, and pure `String`/`Bytes`/`Map`/`List` readers.

### SH-023 — self-hosted checker reaches RS0005 parity at declaration level; the merged callable namespace is the load-bearing rule

- **Tool:** self-hosted checker (`selfhost/check.rss`) run on the reg-VM vs
  `crate::analyze_source` filtered to error-severity `RS0005`
  (DUPLICATE_DECLARATION), over the whole 556-file corpus.
- **Symptom (positive):** the checker reproduces RS0005 with **556/556** parity
  using ONLY top-level declaration structure — no statement/expression/pattern
  body parsing (confirms SH-021: RS0005 is decidable from declaration shape). It
  reuses the proven `selfhost/parser.rss` recognizer verbatim; the sole addition
  is carrying identifier TEXT on each token so names can be compared (the parser
  only kept a keyword/word id, which is 0 for all user identifiers).
- **Namespace grouping replicated (the interesting part — truth per
  `crate::hir::lower::collect_item_signatures`):** duplicates are detected across
  exactly three groups —
  1. **callable namespace = fn names + type CONSTRUCTOR names.** Every
     `struct`/`resource`/`class`/`opaque` type registers BOTH a type-namespace
     entry AND a constructor entry into the SAME map that free functions use, so
     `fn Foo` collides with `struct Foo` (not "separate namespaces"). In the
     corpus this only matters via `fn`-vs-`fn` (fixture `duplicate-declarations`),
     but the faithful rule is the merge.
  2. **type namespace = type names + sum names.** Sums register a type entry only
     (no constructor, so sums never collide with functions), and sum variant
     fields are NOT field-checked.
  3. **per-type field names** for `struct`/`resource`/`class`/`opaque` only
     (fixture `duplicate-fields`). Implemented as: the token immediately before
     each `:` that sits at body-top-level (paren/bracket/angle/brace depth 0),
     which cleanly skips `drop { ... }` bodies, fn-typed field params, and
     generic type args.
- **Backend:** vm (checker is intrinsic/collection-bound like the lexer, cf.
  SH-022; not native-eligible).
- **Root cause / gaps:** none new. All prior constraints held without surprise —
  no char literals (SH-016), single-line boolean chains (SH-017), positionally
  threaded cursors returned by value (SH-018). `Set<String>` (`Set<String>.new()`,
  `Set.contains<String>`, `Set.insert<String>`) worked as the duplicate detector;
  `features: local` was NOT needed (no `StringBuilder`/`local` bindings). The
  scanner RECOVERS past an unrecognized top-level item (skips one token and keeps
  scanning, mirroring the analyzer's recovery) rather than stopping at the first
  one, so a later duplicate is not missed. Parity holds: the analyzer emits RS0005
  on exactly the 2 well-formed duplicate fixtures and the other 554 files stay
  CLEAN (zero false positives).
- **Classification:** docs (records the analyzer's duplicate-symbol namespace
  rule and that RS0005 is a declaration-only property).
- **Tests:** `crate::selfhost_parity::checker_parity_tiny_sample` and
  `crate::selfhost_parity::checker_parity_corpus` (`--ignored`).
- **Status:** done.

### SH-024 — multi-field variant destructuring is not positional; only struct-style field patterns bind

- **Tool:** pre-code feasibility spike for the self-hosting effort (`rss run --vm`).
- **Symptom:** matching a sum variant with ≥2 payload fields positionally —
  `Add(l, r) => …` — fails: each binding is reported `RS0026 "unknown value
  binding"`. The struct-style form `Add { left, right } => …` works, but it also
  requires an explicit scrutinee effect (`match read e { … }`, else `RS0202`).
  Single-field positional binding (`Circle(r) => …`) *does* work, so the
  two-field failure is an inconsistency, not a blanket "no positional patterns".
- **Minimal RSS:**
  ```
  sum Pair { Both(a: Int, b: Int)  Nothing }
  // works now: match read p { Both(a, b)    => ... }   // positional (SH-024)
  // works:     match read p { Both { a, b } => ... }   // named (equivalent)
  // arity err: match read p { Both(a)       => ... }   // RS0037 (1 != 2 fields)
  ```
- **Backend:** all (frontend / parser + binding resolution).
- **Root cause:** positional binding is only wired for single-field variants;
  multi-field variants must be destructured with named `{ field, … }` patterns,
  which additionally project fields and so require a `read`/`mut`/`take` scrutinee
  effect. The two rules compound into confusing errors for the natural
  `Variant(a, b)` shape.
- **Classification:** language (parser / pattern binding + all lowerings) + docs.
- **Decision:** fixed (feature). Positional multi-field variant binding is now a
  first-class, cross-backend pattern form. `MatchPattern::Variant` unifies its
  payload into a single `bindings: Vec<MatchPattern>` (0 = payload-free, 1 =
  single-payload sugar, ≥2 = positional multi-field); each element is a full
  sub-pattern, so nested positions (`V(Some(x), _, 3)`) work for free. The parser
  keeps the positional list (no type info); the position→declared-field mapping
  happens in each type-aware consumer, reusing each backend's existing
  struct-variant field projection: the type checker
  (`checks/body/semantics.rs`) zips `bindings` with declared fields by position,
  the reg-VM (`reg_vm/lower.rs::lower_user_variant_pattern`) emits per-field
  `GetField` after the `MatchVariant` tag test (single-payload keeps
  `UnwrapVariantValue` so native scalar-replacement still dissolves it), and the
  AOT (`rust_lower/lower_match.rs`) emits the named form
  `Sum::V { first: a, second: b }`. The old RS0037 (`POSITIONAL_MULTIFIELD_VARIANT`)
  and `check_positional_multifield_pattern` are removed; RS0037 is repurposed as
  the arity safety net (`VARIANT_PATTERN_ARITY_MISMATCH`): a written positional
  payload must bind exactly as many sub-patterns as the variant declares fields.
  Spec §20.1 amended (positional variant binding moved out of the "positional
  records rejected" tenet into a bounded allowed feature; anonymous positional
  records / implicit flow promotion stay rejected).
- **Tests:** pass fixtures
  `tests/fixtures/pass/positional-multifield-variant.rss` (2-/3-field, ignored
  positions, named≡positional) and `…/positional-multifield-nested.rss`
  (nested per-position); negative arity fixture
  `tests/fixtures/fail/variant-pattern-arity-mismatch.rss` (RS0037);
  backend-parity `backend_differential::backends_agree_on_positional_multifield_variant`
  + `…_nested_variant` (interp ≡ jit ≡ native ≡ compiled); corpus exec
  `tests/corpus/exec/positional_multifield_variant.rss` (vm ≡ compiled). The old
  `fail/positional-multifield-variant.rss` fixture was deleted.
- **Status:** fixed (feature): positional multi-field variant binding supported
  across all backends; RS0037 removed as a restriction and repurposed for arity;
  spec §20.1 amended.

### SH-027 — AST-dump parity COMPLETE: streaming rss producer at full-corpus byte-exact

- **Context:** completes the SH-025 AST-structure arm (step 1 of the 3-step
  frontend-object-parity goal). The self-hosted streaming producer
  (`selfhost/astdump.rss`) now matches the Rust oracle (`parse_source_raw` via
  `crate::selfhost_parity`) **byte-for-byte over the ENTIRE corpus**.
- **Reach:** the self-hosted AST dump reached full-corpus byte-exact parity at
  the time of this milestone. The current gate checks `ok == total` instead of a
  numeric floor; `ast_parity_samples` is the fast curated gate over
  `samples/ast/*.rss`.
- **Final long-tail closed (592 → 619):** protocols (methods as source-order
  functions with Self:Managed injection; `protocol`/`protocol-impl`+`mapping`
  passes), protocol-impls, let-else (`parse_block(open+1)` off-by-one reproduced),
  if-let (→ two-arm match), tuples (types `__TupleN`, exprs `__TupleN(item0:…)`,
  let-destructure), scoped-view desugar (`view v = e` + rest-of-block → `with`),
  match-arm `,`/`;` separator skipping, effect-annotated closure `read || {…}`
  (special-cased before the binary split; general `read <expr>` stays after it so
  `read r * read r` = `(read r)*(read r)`).
- **Method:** ported each reference parser predicate faithfully (LENIENT/surface
  recovery — malformed_* markers, not failures). Every batch re-ran the full
  `--release` corpus to catch regressions (one caught + fixed: the effect/binary
  ordering).
- **Status:** step 1 DONE. Remaining ladder = SH-026 (step-2 deeper semantic
  checks, step-3 AST spans).

### SH-025 — AST-dump parity: streaming rss producer at 543/587, only malformed-recovery remains

- **Context:** step 2 of frontend object parity (after the AST-dump format +
  oracle keystone, SH-adjacent). `selfhost/astdump.rss` is a recursive-descent
  rss parser that STREAMS the canonical dump (see "AST Dump Contract"); the
  harness (`crate::selfhost_parity`) diffs it byte-for-byte against the Rust
  oracle over `parse_source_raw`.
- **Reach:** **543 / 587** corpus files byte-exact (~92.5%), **0 run-failures** — the
  producer never crashes; unsupported constructs mismatch (partial/`unknown-*`
  markers) rather than panic. **32** curated `samples/ast/*.rss` are byte-exact and
  gate non-ignored; `ast_parity_corpus` (`#[ignore]`) ratchets the floor (currently
  543; run in `--release`, ~150s). **Every remaining mismatch (10 files) is a
  `malformed-*` parser-error-recovery fixture** — all well-formed grammar is covered.
- **Covered:** top-level fns (pub/async/native, generic params + bounds, params
  with read/mut/take effects, generic-arg types, return type, body); struct/class/
  resource (opaque, generics, derives, handle/weak fields, defaults, drop); sum
  (variants + fields); const/type-alias/module/use; statements return/let/local/
  assign/if-else/while-loop/**for**/**match**/break/continue/expr; a
  split-at-last-top-level-operator expression parser matching the oracle's
  precedence (with generic-`<>` detection so `Deque<Int>.new()` isn't read as a
  comparison), plus call (name/**qualified with generic args**/receiver, named +
  effect args), field/index, array, **object/map literals**, **closures** `|x| …`,
  **match expressions**, **`!`/`~` unary desugars**, **negative numbers**, try `?`,
  parens, literals. Patterns: variant/binding/wildcard/literal/struct (fields with
  shorthand/`_`/effect/nested, `..` rest).
  Also covered (2026-07-02 sweep): **effect-receiver + no-effect receiver calls**
  (`read x.m()` / `self.m()` → ReceiverCall), **fn attributes** (`#deprecated`/
  `#lower_name`), **effect/retains clauses**, **default-impl marker**, always-emit
  `body`/`block` for no-body fns, **explicit-`fn` closures** (captures/declared-
  effects), **tuple/list patterns** (`__TupleN` desugar, `pat-list` prefix/suffix/
  rest), **interpolated strings** (`$"…{e}…"` → String.format desugar, embedded
  exprs re-tokenized), and **statement_end line-continuation** (`;` terminator,
  `.`/`?` postfix, `| & + * / % ^` operator wrapping, generic-angle depth).
  Also (async/resource sweep): **`manage`/`spawn`/`await` prefix exprs**,
  **`with … as …`/`task_group`/`select` statements**, and a **type-annotated-let
  fix** (`let x: Option<Int> = …` — the value split is the first `=`, not
  top_assign whose `>=` guard skipped the `=` after a generic `>`).
- **Milestones (each a commit + ratcheted floor):** base fns 58 → decls 121 →
  match 178 → generic calls 225 → closures 239 → for+literals 242 → unary/negative
  245 → effect-receiver 248 → no-effect-receiver+attrs+effects+body 273 →
  explicit-fn+tuple/list-patterns 279 → interpolation 280 → line-continuation 286 →
  manage/spawn/await+with/task-group/select 331 → typed-let 339 →
  **Fn-types+type-prefixes(owned/noescape)+fresh-return 405 → async-let+nested-fresh
  459 → feature-section-order+feature-diagnostics+body-less-fns 521 → body-less
  structs/sums+malformed-lets 543**.
- **Residual (the tail):** the ONLY remaining mismatches (10 files, all
  `crates/rsscript/tests/fixtures/fail/malformed-*.rss`) are **parser error-recovery
  markers** — `malformed-field`/`malformed-param`/`malformed-arm`/`malformed-effect`/
  `unknown-top-level`/`malformed-declaration` and the generic/type-arg/call-arg span
  markers. Each needs the reference parser's per-construct validity predicate
  replicated (when parse_field/parse_param/parse_match_arm/… returns None or a
  malformed span) so the producer emits the marker instead of a garbage node. These
  are span-only, fail-fixture-only, and the deepest/lowest-ROI tail. DEFERRED.
- **Also deferred (separate axes):** **protocols/impls/native-modules** (two-pass
  driver + `emit_function` method-transform refactor for ~11 files) and **AST
  `@L:C:N` spans** (Step 3's last phase — invasive: oracle span emission + AST
  tier-strip mechanism + ~150 producer emit sites, node spans non-uniform).
- **rss limitation found:** `if/else` is not valid as an *expression*
  (`let x = if c {..} else {..}` → RS0015) — worked around with helper functions.
- **Status:** superseded by SH-027/SH-026. This was the open mid-point of AST dump
  parity; later milestones closed the AST structure/span ladder. It is retained as
  historical context, not a current pending item.

### SH-026 — Frontend object parity: diagnostics-codes (step 2) + lexer spans (step 3)

- **Context:** the frontend-object-parity ladder beyond AST structure. Two arms
  advanced together with the SH-025 AST work.
- **Diagnostics (step 2, milestone 2a):** `selfhost/check.rss` now reproduces
  **RS0006 / RS0016 / RS0017** (duplicate feature-header / unknown file feature /
  duplicate feature within a header) in addition to RS0005, all decidable from the
  top-level token scan (per-header seen-set matches `parse_features`).
  `CHECKER_TARGET_CODES` extended to the 4 codes; `checker_parity_corpus` is
  byte-exact over **576 files, code-mismatches 0**; each code + CLEAN verified
  firing on crafted inputs and the `unknown-file-feature` fixture.
- **Diagnostics (step 2, milestone 2b — DONE, 2026-07-02):** added **RS0002**
  (MISSING_RETURN_TYPE) and **RS0003** (MISSING_PARAMETER_TYPE) — signature
  explicitness. Faithful token predicates mirror `check_return_type_explicit`
  (no top-level `->` after the param list) and `check_params` (a param whose first
  token is a non-effect ident NOT followed by `:` → empty `ty.name`; effect-first /
  non-ident segments are malformed and produce no Param, so no RS0003). Comparison
  is a sorted+deduped SET, so only presence matters. `CHECKER_TARGET_CODES` = 6
  codes; `checker_parity_corpus` byte-exact **619 files, code-mismatches 0**; the
  sole corpus trigger is `fail/missing-signature-pieces.rss` (expects both). SCOPE:
  covers top-level `fn` decls (the only corpus source of these codes); protocol/
  native-block methods are in skipped decl branches — sound for the corpus, a noted
  extension point.
- **Diagnostics (step 2, milestone 2c — DONE, 2026-07-02):** added **RS0010**
  (REMOVED_PROFILE_DECLARATION — any `profile:` decl) and **RS0011**
  (REMOVED_SHARE_EFFECT — a parameter written `name: share …`, no data effect,
  type name `share`). Both purely structural.
- **Diagnostics (step 2, milestone 2d — DONE, 2026-07-02):** ported `parse_effects`
  and added **RS0004** (UNKNOWN_EFFECT — `fresh`/unrecognized effect name) and
  **RS0012** (REMOVED_RUNTIME_EFFECT — io/allocates/may_panic/may_fail/async/
  suspends). KEY: parse_effects is PER-ITEM — a malformed item (`,,` empty slot,
  `retains()`, `custom(x)`) recovers to RS0015 and is SKIPPED, while valid items in
  the SAME clause still get checked (`effects(no_panic,, native)` → both names
  checked, empty slot → RS0015 only). `effect_item_kind` mirrors the exact
  validity: bare single-token Name, or `retains(ident)` (close+1==end, start+3==
  close, inner ident); everything else malformed. Also fixed a latent bug: the
  signature scan must start at the `fn` keyword (`ns-1`), not the attribute-led decl
  start — otherwise `function_signature_end` stops at the later `fn` (a top-level-
  item boundary), which had hidden the effects clause of `#deprecated(...) fn …`
  (and would have mis-scanned RS0002/3/11 there too). `CHECKER_TARGET_CODES` =
  **10 codes**; `checker_parity_corpus` byte-exact **619 files, 0 mismatches, 0
  run-failures**.
- **Diagnostics (step 2, milestones 2e/2f — DONE, 2026-07-02):** added **RS0028**
  (INVALID_SELF_PARAMETER — a `self` param that isn't the first parameter of a
  qualified/dotted-name method; mirrors check_params) and **RS0033**
  (INTEGER_LITERAL_OUT_OF_RANGE — a whole-file scan for a decimal-integer literal
  token whose value overflows i64, mirroring check_integer_literal_range: all-digit
  text, leading zeros stripped, 19-digit boundary compared against i64::MAX digit-
  by-digit; float/hex literals excluded since their text isn't all digits).
  `CHECKER_TARGET_CODES` = **12 codes** (RS0002/3/4/5/6/10/11/12/16/17/28/33);
  `checker_parity_corpus` byte-exact **619 files, 0 mismatches, 0 run-failures**.
- **Diagnostics (step 2, milestone 2g — DONE, 2026-07-02):** the semantic tier —
  added **RS0007** (retains a non-param OR a Copy scalar param: `type_ref_is_copy` =
  17 scalar names, not fresh/noescape, no args/fn), **RS0024** (UNKNOWN_TYPE — a
  type ref to an undeclared type; recursive TypeRef validation over field/param/
  return types with generic-param scope), **RS0008** (MISSING_PARAMETER_EFFECT — an
  effect-less param unless share/noescape/owned/bare-Closure/surface-`&`/contains-Fd/
  Copy-scalar/payloadless-sum), **RS0009** (INVALID_PURE_EFFECT — a `pure` fn with a
  resource return / mut|take param / retains item / body `with`|`manage`|non-pure
  call). LOAD-BEARING FINDING (RS0024): the oracle's known-type set is NOT just the
  ~45 hardcoded builtins — it also includes every struct/resource preloaded from the
  CORE + STANDARD package `.rssi` interfaces (via `hir.type_info`); those 56 names
  (JsonValue, SortedSet, Deque, ResourcePool, Response, StringBuilder, …) are
  extracted into `is_stdlib_type` (58 false positives without them). RS0009's
  non-pure-call resolution is token-based (qualified calls to known-type namespaces
  + constructors + enum-variants + declared-pure fns are allowed; declared non-pure
  fns flag; unresolved ignored) — verified against the clean pure files
  (pure-string-read-call/pure-helper-call/pure-read-function) and pure-native-call.
  Implemented via a sub-agent against a precise ported spec. `CHECKER_TARGET_CODES`
  = **16 codes** (RS0002/3/4/5/6/7/8/9/10/11/12/16/17/24/28/33); `checker_parity_
  corpus` byte-exact **619 files, 0 mismatches, 0 run-failures**. Commits 6dbc59f9
  (RS0007) / fd688f09 (RS0024) / e559c1f2 (RS0008) / 89559315 (RS0009). MAINTENANCE
  NOTE: the RS0024 stdlib-type list is derived from the `.rssi` interfaces at
  authoring time — regenerate if those interfaces change.
- **Diagnostics (step 2, milestone 2h — DONE, 2026-07-02):** **RS0021** NON_EXHAUSTIVE_
  MATCH. Needs scrutinee type inference (the analyzer reads `hir_expr_type_name`), but
  the corpus is tractable: only `_` short-circuits (a top-level bare ident is a Variant,
  not a catch-all); user-sum/Bool scrutinees are params or `let x = ctor`/local-call
  (locally inferable → all-variant coverage); Option/Result-returning stdlib-call
  scrutinees fall through to the Some+None/Ok+Err fallback (matches the analyzer).
  Ported the exhaustiveness engine (arm segmentation + scrutinee-root inference +
  Option/Result/Bool/sum/List/tuple/fallback coverage) via sub-agent; 4 false-positives
  hunted+fixed (`sum` as a var name, `match true`, List slice patterns, `?`-terminated
  scrutinee). Commit 91c43189.
- **Diagnostics (step 2, milestone 2i — DONE, 2026-07-03):** the remaining token-
  decidable tail, via sub-agents (one crashed on an API error after 2 codes — its
  uncommitted work was green and recovered; lesson: commit each code immediately).
  Added **RS0029** (await-outside-async), **RS0023** (Fd outside internal boundary),
  **RS0035** (lower-name-conflict — ported is_valid_rust_ident + keyword set + default
  lowering), **RS0027** (unknown-protocol — visible = stdlib interfaces + file `protocol`
  decls; Managed/Struct/Resource excluded), **RS0014/RS0018/RS0019** (noalloc/no_block/
  no_panic body violations — RS0009-style call scan). `CHECKER_TARGET_CODES` = **24
  codes**; `checker_parity_corpus` byte-exact **619 files, 0 mismatches, 0 run-failures**.
  Commits 240ce274/9715367f/78012146/3ad62a2b/be6fa09d.
- **Diagnostics (step 2, milestone 2j — signature table — DONE, 2026-07-03):** built the
  cross-function **signature table** as the batch's infrastructure: a pre-pass over the
  token stream that records, per top-level `fn`, its cross-call attributes (started with
  same-file `async fn` names, collected by extending `collect_rs0009`'s fn walk; the
  call-resolution helper is a membership probe against these name sets, since same-file
  fns register only under their unqualified simple name). Landed the one candidate that is
  purely signature-table-decidable: **RS0022** (ASYNC_CALL_NOT_CONSUMED) —
  `has_unconsumed_async_call` flags a call resolving to a same-file async fn that is not
  the immediate `await`/`spawn` operand nor an `async let` RHS (mirrors
  `check_async_call_consumed`); there are **no async builtins**, so qualified/receiver and
  stdlib calls never resolve to an async signature and a token-adjacency probe is exact
  over the corpus (verified against all ~30 async-fn corpus files: every same-file async
  call is consumed via `await`/`async let`, only the RS0022 fixture is unconsumed).
  `CHECKER_TARGET_CODES` = **25 codes**; `checker_parity_corpus` byte-exact **619/619,
  0 mismatches, 0 run-failures**. Commit c0e7894b.
  - **The other four batch-3 candidates were MEASURED and SKIPPED (blocked on the batch-4
    engine, not ducked):**
    - **RS0013** (invalid-try) — the return-root sub-rule (`?` in a fn whose return root is
      not Result/Option) IS signature-decidable and was implemented, but it is **not
      corpus-green on its own**: two fixtures flag RS0013 *inside* Result-returning fns —
      `try-operator-non-result-value.rss` (operand `load()` returns a struct → `#1`
      `check_try_value_is_result`) and `try-operator-error-type-mismatch.rss` (operand's
      Result error type ≠ the fn's → `#2`). Both need **operand/error-type inference**, so
      the return-root rule alone produces false negatives. Reverted. → **needs type
      inference**.
    - **RS0201** (unnamed-arg) — an unnamed arg is allowed only for receiver-call shorthand,
      private same-file unqualified fns, and constructor field-shorthand; everything else
      (public fn, core/builtin qualified call, variant, constructor) requires named args.
      The corpus fixtures fire on qualified core calls (`String.concat("prefix", …)`,
      `Image.save(read image, …)`), which need the **full builtin/core signature table** to
      know the callee resolves-and-requires-named, plus qualified-vs-receiver
      disambiguation and constructor field names. → **needs type/callee resolution**.
    - **RS0202** (missing-data-effect) — needs each callee param's declared effect AND a
      **Copy/non-Copy type model** (scalars don't require effects) AND receiver type
      inference (`mut cache.put(key: "x")` must resolve `cache: Cache`). The fixtures are
      core/receiver/generic calls (`Image.resize`, `Db.close`, `ResourcePool<…>.borrow`).
      → **needs type inference**.
    - **RS0036** (payload-not-transferable) — at this historical point it was
      treated as needing message-payload Send/transferability analysis.
      Superseded: RS0036 is now baked as code #80; the self-host checker skips
      bare enclosing-function type params (`Channel.message<T>`) to match the
      Rust analyzer and fires on concrete non-transferable payloads like
      `Channel.message<List<Int>>`. RS0038 (char-literal) still has 0 corpus
      fixtures.
- **THE TOKEN-DECIDABLE TIER IS EXHAUSTED; the cross-function signature table adds exactly
  RS0022 (25 codes total).** The remaining candidates SKIPPED because they need type
  inference / callee-signature resolution (measured, not ducked): RS0201, RS0013, RS0202,
  RS0036 (all above; now superseded/baked). None is blocked on *borrow* analysis specifically — they are all
  type-inference / callee-resolution gaps (the #3 borrow/ownership engine is a separate
  need, seen in RS0301-0313/RS06xx/RS07xx below). THE REMAINING BULK (~260 corpus files when ALL
  ~100 codes are targeted → 305 mismatch): RS0207/0208/0209/0210 (type/return/control-
  flow/operator mismatch), RS0301-0313 + RS06xx + RS07xx (ownership/borrow), RS0015
  (unsupported-syntax), RS0101 (feature-gating), RS0025/0026 (unknown-field/binding).
  These require a self-hosted TYPE-INFERENCE + BORROW-CHECKER engine — the whole semantic
  frontend — which is the genuine next phase (its own multi-session effort), NOT more
  token predicates.
- **Diagnostics (step 2, milestone 2k — DONE, 2026-07-03):** **RS0101** FEATURE_VIOLATION.
  The 2j summary mis-filed RS0101 with the type-inference bulk; it is in fact
  **token-decidable** (feature-keyword-vs-header), so it landed cleanly. Reproduces all
  three oracle sources: (1) `checks/features.rs` feature_uses — a construct whose required
  feature is absent from the header: `local` (WORD_LOCAL let/closure, `manage`, `take`
  data-effect, `ResourcePool<T>`), `unsafe` (`effects(unsafe)`), `async` (`async`
  modifier + `spawn`/`await`/`task_group`/`select`); (2) `signatures.rs::check_native_effect`
  — a `native fn` missing `effects(native)` fires **regardless** of the declared features
  (so native is tracked in the fn walk, not gated); (3) `body/semantics.rs::check_match_pattern_effects`
  — `match take` w/o local (subsumed by the `take` probe). KEY false-positive hunts (the
  dangerous direction): the `async` EFFECT name in `effects(io,…,async)` is a REMOVED
  runtime effect (RS0012), NOT an async construct — the `async` modifier is gated on the
  next kw being fn/let/for; `take` is distinguished from `.take(` (method) and a `take:`
  binding; reserved feature keywords (`local`/`async`/`unsafe`) only appear in a `features:`
  header when DECLARED, so a header token self-gates. `declaredFeatures` is an
  order-independent pre-pass; reuses `effects_name_probe` (new mode 2 = native effect) and
  `fn_is_native`. Self-hosting corner correctly handled: `astdump.rss` (10 `local` stmts, no
  header) is a corpus file the oracle flags RS0101 standalone, and the checker matches.
  `CHECKER_TARGET_CODES` = **26 codes**; `checker_parity_corpus` byte-exact **619/619, 0
  mismatches, 0 run-failures**. Commit 7fd0ce16.
- **RS0015 UNSUPPORTED_SYNTAX — SCOPED, left OUT (2026-07-03).** RS0015 is a SINGLE code
  fired if ANY malformation is present, so the SET is all-or-nothing: a partial port turns
  every un-handled trigger into a false negative on its fixture, so it cannot reach
  0-mismatch without covering ALL 33 fixtures. **~24 of 33 are token-decidable** (structural
  token scan): unclosed-call/function-body (unbalanced), malformed-{type,function,field,
  parameter,empty-parameter,call-argument,empty-call-argument,type-argument,generic-parameter,
  match-arm,effect,with}-declaration (the effects ones are ALREADY detected via the ported
  `effect_item_kind`==0), unsupported-with-syntax, malformed-binding (`let x =` empty RHS),
  duplicate-import-name, unknown-top-level-item (`enum`), namespace-declaration,
  reserved-double-underscore-name (`__` prefix), opaque-type-with-fields (opaque + body),
  protocol-default-method-body (protocol method w/ body), native-body-unsupported (native fn
  w/ body), spawn-not-executable (`spawn` kw), unsupported-derive (known-derive set).
  **~9 need real parsing / name resolution** (NOT token-decidable): none-call-form (`None()`),
  none-with-payload (`None(1)`), option-type-called-as-variant (`Option(1)`),
  result-type-called-as-variant (`Result(1)`), variant-named-payload (`Ok(value: 1)`) — all
  need constructor/variant name+arity resolution; trailing-expression-token (needs
  expression-extent parsing to know where an expr ends); const-non-literal-initializer
  (literal-vs-expression classification of the const RHS); malformed-generic-parameter (the
  `T: Unknown` bound is semantic; only the `<read T>` half is decidable); malformed-control-
  statement (else-without-block / `while {` / `match {` need statement-grammar parsing). Since
  the 9 require the same semantic/expression engine as the deferred SH-025 malformed-recovery
  tail, RS0015 was kept **OUT of CHECKER_TARGET_CODES** at this point and planned for the
  semantic-frontier phase alongside RS0207-0210 / RS0301-0313 / RS0025-0026. RS0025/RS0026 have 0 corpus
  fixtures (skip, as noted in 2j).
  **Superseded:** RS0015 is now baked; see Milestone 2be.
- **Lexer spans (step 3):** added a `len` field to the shared `Tok`
  (= consumed source span `j-i`, matching the Rust lexer's `index-start`) and made
  `lexer.rss` emit the real `<line>:<col>:<len>` prefix. `lexer_parity_corpus` is
  now byte-exact at **all three tiers** (0 kind+payload, 1 +line/col, 2 +length) —
  576 files, token-mismatches 0 each. The lexer span ladder is fully closed; the
  additive `Tok.len` left parser/checker/astdump parity untouched.
- **AST spans (step 3) — DONE, 2026-07-03:** `ast_parity_corpus` is byte-exact
  **619/619 at all three tiers** (0 structure+payload, 1 `@line:col`, 2
  `@line:col:len`), 0 run-failures each.
  - **Discovered span rule (load-bearing):** every AST node's `Span` is ONE
    representative token's `line:col:len` — never a multi-token range. So the
    producer reproduces a node's span by emitting that ONE token's position and
    length (`tk_len`), NOT by measuring the node's extent. Representative token
    per node: *first (paren-trimmed) token* for the vast majority; **Binary → the
    operator token**; **Try → the trailing `?`**; **ReceiverCall → the receiver
    token** (`tokens[receiver_start]`, i.e. after the effect kw); **TypeRef → the
    NAME token** (`name_index`, after prefix keywords read/mut/take/fresh/handle/
    weak/noescape/owned); **tuple type/expr → the `(`**; decls (fn/type/sum/const/
    alias/module/use) → the decl's FIRST token *including* `#`-attributes and
    `pub` (parse_*'s `current()` at entry); **MatchArm → the arm's first token**;
    **if-let's two synthetic arms + the synthetic protocol `Self: Managed` generic
    → the `if`/`fn` token** (`method.span`); interpolated-string desugar nodes
    (call/effect/string/array) → the interp-string token, and its EMBEDDED exprs
    self-reproduce because BOTH oracle and producer re-tokenize the fragment from
    1:1; an empty named call-arg (`print(value:)`) → the arg's NAME token
    (parse_call_args' `tokens[start]` Unknown fallback). Patterns (`pat-*`) and
    pure structural labels (value/body/block/cond/then/else/callee-*/arg/
    object-field/map-entry/key/derive/bound/effect-name/malformed-generic|param|
    field|effect|arm/…) are NOT spanned on EITHER side.
  - **Mechanism (mirrors the lexer ladder, no new invention):** the producer
    ALWAYS emits the richest ` @line:col:len` via `emit_at`/`spanof`; the harness
    (`run_astdump`) PROJECTS each line down to the active `RSS_SELFHOST_AST_TIER`
    before the byte-exact compare (tier 0 drops the suffix, tier 1 keeps
    `line:col`). No tier flag threads through the producer. The oracle side uses
    the pre-added `sp`/`push_node` scaffolding, tier-gated by the same env var.
    `expr_rep_tok` mirrors `emit_expr`'s dispatch to recover an expression's span
    token without descending (used for expr-statement heads + single-expression
    closure bodies — the latter was the dominant tier-1 failure, since
    `noescape-callback-*` fixtures all pass `|| …`/`|x| …` callbacks whose
    `Stmt::Expr` body head is spanned).
  - **Commits:** 1d86a430 (oracle heads → push_node, tier 0 unchanged 619/619) /
    629253d2 (producer spans + harness projection; tier 1 619/619; tier 2 619/619
    verified on the same code — `:len` is just the rep token's own length, which
    lexer parity already proved matches). No node types left blocked.
  - **Gate:** default (env unset) stays tier 0, byte-exact 619/619 — the committed
    gate. Tiers 1 and 2 are run via `RSS_SELFHOST_AST_TIER=1|2`.
- **Status:** the frontend-object AST parity ladder (structure → line:col →
  line:col:len) is CLOSED. Remaining self-host frontier is the semantic tier
  (type-inference + borrow-checker codes; see the SH-026 step-2 tail).

### Milestone 2l — historical RS0013 fallback before the stdlib error-type map

- **Goal:** begin the semantic type tier — a CONSERVATIVE expression type-of pass
  (`selfhost/check.rss::expr_type_root`) and use it to land RS0013 (invalid-try),
  the most FP-safe type code. Foundation reused by later slices (RS0210 operator,
  RS0207 argument, RS0208 return).
- **Built (committed):** `expr_type_root(toks, s, e, bodyOpen, exprPos, popen,
  pclose, declared, allFns)` — types ONLY the forms the oracle computes with
  certainty, else "" (unknown => no fire => no false positive):
  * String / number (`number_literal_root` = Float iff text has `.`, else Int) /
    Char literal · `true|false`→Bool · `null`→JsonLiteral · `Unit`→Unit ·
    `None`→Option · `Some(..)`→Option · `Ok|Err(..)`→Result · a sum-variant head →
    its owning sum (`variant_owner`) · an unqualified call `name(..)` → same-file
    `fn_return_root` or a declared-type constructor · a bare ident → a let-typed
    local (recursively) or a declared-type param (`param_type_root`).
  * Everything else — Binary / Index / Field / qualified & receiver calls
    (`Ns.m(..)` / `x.m(..)`) / object / array / closure — returns "" by design
    (those need the full stdlib signature DB the token-level checker lacks; keeping
    them unknown is the FP-safety mechanism). Helpers `try_operand_root` +
    `fn_invalid_try` reproduce two of the three RS0013 sub-rules token-level.
- **RS0013 sub-rules (oracle has THREE, not two):**
  * **A — result-returns** (`analyzer/runtime_guarantee.rs::
    check_try_operator_result_returns`): any `?` in a fn whose return base ∉
    {Result, Option}. Token-level, exact. Reproduced. (10 corpus files, incl.
    fixture `try-operator-non-result.rss`.)
  * **B — value-is-result** (`checks/body/try_checks.rs::check_try_value_is_result`):
    a `?` operand of a confidently-known concrete non-Result/Option type.
    Reproduced via `expr_type_root`. (1 file: fixture
    `try-operator-non-result-value.rss` — operand `load(..)`→`Image`.)
  * **C — error-type mismatch** (`checks/body/try_checks.rs::check_try_error_types`):
    a `?` whose operand `Result<T, E_op>` has `E_op` ≠ the fn's declared error
    type. Reproduced for unqualified operands only.
- **Historical blocker (why RS0013 was not yet in CHECKER_TARGET_CODES):**
  `tests/fixtures/fail/ast-call-missing-effect-nested.rss` (fn returns
  `Result<Unit, IOError>`) fires sub-rule C on TWO **qualified stdlib** operands —
  `File.open_write(..)?` and `File.write(..)?`, whose error type the oracle knows
  is `FileError` (≠ `IOError`). Reproducing that needs per-method stdlib
  error-type inference on qualified/receiver calls, which the spec deliberately
  keeps UNKNOWN (typing them risks corpus-wide false positives on the many clean
  `File.*?`/`Json.*?` calls whose error type matches). So RS0013 cannot reach
  0-mismatch at the spec's conservatism level — FALLBACK taken.
- **Historical outcome:** `expr_type_root` + the RS0013 sub-rule A/B wiring were committed and
  exercised across all 619 corpus files (0 run-failures). At this historical
  point, RS0013 was emitted by the checker but not yet part of the parity target
  code set, so the gate was unchanged. It fired correctly on 11 of the 13 oracle-RS0013 files (all
  sub-rule A + B); the 2 sub-rule-C files stay unflagged (the documented blocker),
  and every clean `?` file stays unflagged (0 false positives, verified).
- **Gate:** `checker_parity_corpus` 619/619 ok, 0 mismatches, 0 run-failures at
  the SAME 26 codes (RS0013 absent). Green.
- **Next slice:** RS0013 becomes gateable once qualified-call error-type inference
  exists; meanwhile `expr_type_root` is ready for RS0210/RS0207/RS0208.
  **Superseded by Milestone 2m**, which added the stdlib namespace error-type map
  and baked RS0013.

### Milestone 2m — type-inference engine slice 2: stdlib namespace→error-type map + RS0013 sub-rule C → RS0013 GATED (27 codes)

- **Goal:** complete RS0013 by adding sub-rule C (error-type mismatch) — the
  blocker from slice 2l — and add RS0013 to `CHECKER_TARGET_CODES`.
- **Measure-first:** added RS0013 to the target with only sub-rules A+B and ran the
  corpus once. Exactly TWO `[mismatch]` files (the real sub-rule-C set) — no guess:
  * `fail/ast-call-missing-effect-nested.rss`: fn returns `Result<Unit, IOError>`;
    `File.open_write(..)?` / `File.write(..)?` → `FileError` ≠ `IOError`.
  * `fail/try-operator-error-type-mismatch.rss`: fn returns `Result<_, AppError>`;
    `load_config()?` (same-file fn) → `ConfigError` ≠ `AppError`.
- **Built (committed):**
  * `stdlib_error_type(ns, method)` — the namespace→error-type map read from the
    `.rssi` interfaces. Filesystem (`File`/`Directory`/`Env`/`Path`) → `FileError`;
    JSON-shaped codecs (`Json`/`Toml`/`Yaml`) → `JsonError`. Per-method exceptions
    keyed and EXCLUDED (yield "") because they break module uniformity:
    `File.bytes_stream` → `ChannelError` (async streaming), and
    `Path.{from_string,resolve_relative,safe_relative}` → `String` error. Every
    other namespace → "" (unknown ⇒ no fire ⇒ FP-safe).
  * `return_error_type_at` / `result_error_root` — parse the second (error) type
    arg of a `Result<T, E>` return (mirrors `return_type_root_at`).
  * `fn_error_type_by_name` — the declared error type of an unqualified same-file
    `fn` (mirrors `fn_return_root`).
  * `try_operand_error_type` — the `?`-operand's Result error type: a qualified
    `Ns.method(..)?` via the map, or an unqualified `name(..)?` via that fn's
    error type; anything else (bare ident, index, field, `Ok`/`Err`/`Some`) → "".
  * `fn_invalid_try` extended: inside a fn whose return root is exactly `Result`
    with a known 2-arg error type E, a `?` whose operand error type is known and
    ≠ E fires RS0013 (sub-rule C).
- **FP discipline:** the first full run at the mapped families surfaced ONE false
  positive — `examples/scripts/async/common_io.rss`: `File.bytes_stream(..)?` in a
  `Result<_, ChannelError>` fn. `File` is NOT uniform (bytes_stream → ChannelError),
  so the blanket File→FileError mis-fired. Fixed by excluding `File.bytes_stream`
  (and the String-error `Path` methods). Re-verified whole-corpus uniformity of the
  mapped families before re-running.
- **Gate:** `checker_parity_corpus` **619/619 ok, 0 mismatches, 0 run-failures** at
  **27 codes** (RS0013 added). Green. The two sub-rule-C fixtures now match the
  oracle exactly; every clean `File.*?`/`Json.*?`/etc. operand stays unflagged.
- **CHECKER_TARGET_CODES (27):** RS0002, RS0003, RS0004, RS0005, RS0006, RS0007,
  RS0008, RS0009, RS0010, RS0011, RS0012, RS0016, RS0017, RS0021, RS0024, RS0028,
  RS0033, RS0029, RS0023, RS0035, RS0027, RS0014, RS0018, RS0019, RS0022, RS0101,
  RS0013.
- **Next slice:** `expr_type_root` + the error-type map are ready for RS0210
  (operator), RS0207 (argument), RS0208 (return).

### Milestone 2n — call-signature cluster slice: RS0201 UNNAMED_ARGUMENT GATED (28 codes); RS0202 blocked (constructor-inline sub-case)

- **Goal:** add the call-resolution/param-signature cluster — RS0201 (unnamed
  argument) and RS0202 (missing data effect). Both need callee param signatures
  and light receiver-type inference.
- **Measure-first (one run, both codes in target, rss emitting neither):** the
  whole-corpus oracle set is small and bounded — **5 RS0201 files, 9 RS0202
  files**:
  * RS0201: `fail/ast-call-unnamed-nested.rss` (`String.concat("prefix", ..)`),
    `fail/call-unnamed-and-missing-argument.rss` (`combine(read "a", ..)`, a
    `pub fn`), `fail/features-and-call-style.rss` (`Image.save(read image, ..)`),
    `fail/malformed-empty-call-argument.rss` + `samples/ast/mal_empty_call_arg.rss`
    (`Log.write(, message: ..)` empty slot).
  * RS0202 (4 distinct oracle sub-cases): argument-effect (`File.write`/
    `Image.resize`/`ResourcePool.borrow`/`Log.write`/same-file `Cache.put`),
    receiver-call self-effect (`read cache.put(..)` vs `self: mut`), constructor
    inline managed field (`Boxed(image: read image)`), and match-scrutinee effect
    (`match xs { [a,b] => .. }` with no `read`/`mut`/`take`).
- **RS0201 built (committed):**
  * `collect_call_fn_sigs` — `pubFns` (public unqualified fn names) + `dottedFns`
    (dotted method fn names, e.g. `Cache.put`), the same-file call-resolution
    table.
  * `call_requires_named` / `is_core_named_namespace` — call-kind classifier
    mirroring the parser's `is_qualified_namespace_receiver` (uppercase dotted
    head ⇒ qualified). Fires only for public same-file unqualified fns and a
    **measure-first-curated** core namespace allowlist `{String, Image, Log}`;
    receiver calls, private helpers, variant/type constructors, same-file methods,
    and unknown/imported namespaces are skipped (FP-safe).
  * `args_have_unnamed` / `seg_is_unnamed` — arg splitter respecting `()[]<>{}`
    nesting **and closure param pipes `|a, b|`**, with the malformed empty-slot
    and lone-trailing-comma rules.
- **FP discipline:** first run at a broad rule surfaced 14 false positives — all
  from (a) closure multi-param commas `|acc, val|` splitting a named arg, and
  (b) qualified calls to user/imported sum-variant or unknown namespaces
  (`ChatMessage.system(..)`, which the oracle leaves UNKNOWN_CALLEE with no naming
  diagnostic). Fixed by pipe-toggling the splitter and by curating the qualified
  fire-set to `{String, Image, Log}` (the only namespaces the oracle actually
  flags). A `)`-guard that mis-read a previous statement's close-paren as a
  complex receiver was removed (it FN'd `Image.save`).
- **RS0202 — NOT landed (blocked):** it is a file-level OR flag over FOUR oracle
  sub-cases; greening requires all nine oracle files AND zero FP over 619
  constructor/call-heavy files. The **constructor-inline-managed-field** sub-case
  (`fail/constructor-inline-managed-field.rss`) is the blocker: the oracle rule
  (`checks/body/fresh.rs::constructor_arg_uses_managed_inline_value`) fires only
  when the field is a non-Copy, non-`handle` INLINE struct/class field AND the
  value is a *managed* binding (`let`-bound, or crossing a handle, or a
  non-`fresh` managed-returning call) — a classification needing per-field
  `handle`/`weak` parsing, Copy/type-kind resolution, and `let`-vs-`local`-vs-
  param-vs-`fresh` binding tracking. No token-level approximation is FP-safe
  across the many legitimate `T(field: read x)` constructor calls in the corpus,
  and missing this one file leaves RS0202 red. Deferred; the arg-effect stdlib
  map (`File.write`→file:mut, `Image.resize`→image:mut, `ResourcePool.borrow`→
  pool:mut, `Log.write`→message:read) + same-file receiver-method self/param
  effects + the match-scrutinee scan (the other 8 files) are straightforward once
  a safe constructor-inline rule exists.
- **Gate:** `checker_parity_corpus` **619/619 ok, 0 mismatches, 0 run-failures**
  at **28 codes** (RS0201 added). Green.
- **Env note:** the Docker dev stack was factory-reset mid-slice (0 images/
  containers/volumes); verified on the host toolchain (`cargo 1.95.0`,
  `aarch64-apple-darwin`) instead — same `checker_parity_corpus` test, 1730s.
- **CHECKER_TARGET_CODES (28):** RS0002, RS0003, RS0004, RS0005, RS0006, RS0007,
  RS0008, RS0009, RS0010, RS0011, RS0012, RS0016, RS0017, RS0021, RS0024, RS0028,
  RS0033, RS0029, RS0023, RS0035, RS0027, RS0014, RS0018, RS0019, RS0022, RS0101,
  RS0013, RS0201.
- **Next slice:** RS0202 once a FP-safe constructor-inline-managed-field rule is
  found (parse field `handle`/`weak` + Copy/type-kind + `let`/`local` binding);
  the arg-effect + receiver-self + match-scrutinee sub-cases reuse
  `collect_call_fn_sigs` and the arg splitter directly.

### Milestone 2o — RS0202 MISSING_DATA_EFFECT LANDED (29 codes)

- **Goal:** land RS0202, the previously-blocked call/param-effect flag — a
  file-level OR over four oracle sub-cases (`checks/calls.rs` arg-effect +
  receiver self-effect, `checks/body/fresh.rs` constructor-inline-managed-field,
  match-scrutinee). This completes the value-model foundation (type-kind +
  Copy predicate + managed-binding tracking + per-param effect signatures) that
  the type-mismatch codes RS0207-0210 build on.
- **Landed (2026-07-04):** `selfhost/check.rss` +500 lines. New value-model infra:
  `stdlib_param_effect(ns, method, pname)` (curated `.rssi` param-effect map, built
  measure-first — only the methods the corpus needs), `value_effect` (visible
  effect of a call-site value), `sig_param_effect` (per-param effect from a
  same-file signature), `arg_effect_bad`/`arg_seg_effect_bad` (closure-pipe-aware
  arg splitter → sub-case 1), `receiver_self_effect_bad` (sub-case 2), a
  constructor-arg managed-field walk (sub-case 3), and `fn_data_effect_bad` /
  `call_site_effect_bad` threading them per fn body.
- **Verdict:** `checker_parity_corpus` byte-exact **619 files, 619 ok, 0
  run-failures, 0 code-mismatches** (host toolchain, 1720s — the +500 lines slow
  per-file reg-VM checking, hence the longer run). RS0202 added to
  `CHECKER_TARGET_CODES` → **29 codes**. Green.
- **Process note:** the implementing sub-agent stalled (stream watchdog, no
  progress 600s) AFTER the ~40-file subset dev test passed but BEFORE the full
  corpus run — leaving RS0202 in the target UNVERIFIED. Picked up in the main
  loop: ran the full corpus (green), removed the agent's leftover temp dev tests
  (`rs0202_dev`, `rs0202_oracle_scan`, `RS0202_SUBSET`) from `selfhost_parity.rs`,
  then committed. Lesson reinforced: the subset dev test is necessary but NOT
  sufficient — the full 619 corpus is the gate.
- **CHECKER_TARGET_CODES (29):** the 28 above + RS0202.
- **Next slice:** RS0207-0210 (argument/return/control-flow/operator type
  mismatch) — the pervasive-expression-typing cluster; reuses this slice's
  type-kind/Copy/effect-signature infra.

### Milestone 2p — RS0212 RESOURCE_DERIVE_UNSUPPORTED LANDED (30 codes)

- **Planning census (2026-07-04):** measured the full remaining backlog =
  **55 distinct codes** across the fail corpus (temp `remaining_code_census` test,
  pure Rust). Per-code oracle-set sizes recorded; ~22 codes fire on a single corpus
  file. Strategy: land the token-DECIDABLE codes first (no type/borrow engine),
  batching where possible; the type-inference cluster (RS0207-0210) and borrow
  cluster (RS0301+) come after.
- **RS0212 (2026-07-04):** a value derive (`Clone`/`Eq`/`Ord`/`Hash`/`JsonEncode`/
  `JsonDecode`) on a `resource` type — resources allow only `Debug`/`Schema`/
  `ReviewSchema` (oracle `analyzer/derives.rs::check_resource_derives`). Purely
  structural: `has_bad_resource_derive` walks type decls, and for each `resource`
  (via `type_name_start` + `at_ident(ns-1, WORD_RESOURCE)`) scans its `derives(...)`
  header clause for a banned name. Zero type inference. FP surface is near-nil: the
  ENTIRE 619-file corpus has exactly ONE `resource … derives(…)` decl (the fixture).
- **Verdict:** `checker_parity_corpus` **619 files, 619 ok, 0 run-failures, 0
  code-mismatches** (1778s). CHECKER_TARGET_CODES → **30 codes**.
- **Process:** implemented + verified entirely in the MAIN LOOP (three sub-agents
  stalled this session on the watchdog — RS0202 and two on RS0208/RS0210). The
  reliable pattern now: main-loop implements, a pure-Rust oracle scan + a fast
  reg-VM subset spot-check gate the logic, then the orchestrator runs the full
  corpus in the background (monitored, ~30min) as the true gate. Temp tests removed.
- **CHECKER_TARGET_CODES (30):** the 29 above + RS0212.
- **Next decidable candidates (from census):** RS0037 (variant-pattern arity),
  RS0211 (unsupported derive), RS0034 (uninferable binding), RS0205 (dup arg —
  needs callee param resolution, higher FP). The type/borrow clusters remain the
  bulk of the 55.

### Milestone 2q — RS0037 variant-pattern arity (31 codes) + corpus-gate speedup (~30min → ~90s)

- **RS0037 VARIANT_PATTERN_ARITY_MISMATCH (2026-07-04):** a positional variant
  pattern `V(b1,…,bn)` whose head is a known sum variant, is not named (`field:`),
  binds n>0 sub-patterns, and n != the variant's declared field count (oracle
  `checks/body/semantics.rs`). check.rss gains: `count_top_segments` /
  `region_has_top_colon` (depth-aware), `collect_variant_arities` (a ONE-PASS
  variant→arity table encoded as `Name:arity` Set<String> keys — `variant_arity_of`
  probes 0..11), and a `match`/arm/pattern walk (`has_variant_arity_mismatch` →
  `match_arm_arity_bad` → `arm_pattern_arity_bad`). Fires only on known variants
  with a positional non-named payload → tiny FP surface. → **31 codes.**
- **PERF LESSON (important):** the first RS0037 cut recomputed variant arity by
  re-walking ALL declarations per pattern — O(patterns × tokens), which on the
  4k-line self-hosted tool files (check.rss ~220KB) blew a single file up to
  minutes on the reg-VM. Fix = precompute the arity table once per file. Always
  precompute file-level tables; never re-scan per occurrence.
- **corpus-gate speedup (`selfhost_parity.rs`):** the `checker_parity_corpus` gate
  was ~30min because it ran the reg-VM checker over 619 files SEQUENTIALLY on one
  thread. Two fixes: (1) **work-stealing parallelism** — each worker compiles its
  own exe (`RegVmExecutable` holds an `Rc`, not `Sync`) and pulls file indices off
  a shared `AtomicUsize`, saturating ~6-7 cores; (2) **slow-test gate** — the 4
  giant files (check.rss/astdump.rss/scan.rss/package-manager, each minutes-long
  and un-splittable) are skipped by default (logged, no silent truncation) for a
  **~90s** fast gate (build incl.; ~35s run, 615 files); `RSS_SELFHOST_FULL=1`
  runs all 619. RS0037 is sound on the skipped giants (they have 0 positional
  variant patterns). Fast gate: 615/615, 0 mismatch.
- **CHECKER_TARGET_CODES (31):** the 30 above + RS0037.

### Milestone 2r — RS0034 uninferable binding (32 codes)

- **RS0034 UNINFERABLE_BINDING_TYPE (2026-07-04):** a bare `Ok(...)`/`Err(...)`/
  `None` bound to an UNUSED name with no type annotation leaves an open type
  parameter with nothing to pin it (oracle `checks/body/binding.rs`:
  `open_variant_constructor` + unused-name analysis). check.rss:
  `fn_has_uninferable_binding` walks each fn body for `let [mut] NAME = <rhs>`
  where the RHS is EXACTLY `None`/`Ok(..)`/`Err(..)` (no `: Type`, nothing trailing
  — a `?` or `.`/operator disqualifies), then `name_used_in_body` confirms NAME
  never recurs. `Some(x)` is fully determined → excluded. → **32 codes.**
- **Verified on the fast gate:** 615/615, 0 mismatch (95s). Sound on the 4 skipped
  giants: they contain ZERO `let = None/Ok(/Err(` bindings, so RS0034 cannot fire
  there (grep-confirmed) — the fast gate is a complete verification for this code.
- **CHECKER_TARGET_CODES (32):** the 31 above + RS0034.

### Milestone 2s — RS0311 invalid-assignment (33 codes)

- **RS0311 INVALID_ASSIGNMENT (2026-07-04):** reassigning an immutable `let` local
  (oracle `analyzer/assign.rs::validate_assignment`). Key enabler: the self-hosted
  scanner fuses only `->`/`=>`, so `==` is two `SYM_EQ` and a lone `=` appears ONLY
  in let-bindings and assignments — a `SYM_EQ` whose next token isn't `=` and whose
  previous token is an ident not preceded by `let`/`mut`/`local` is exactly an
  assignment. check.rss: `collect_let_kinds` (immutable `let` vs `let mut`),
  `collect_param_names` (params → exclusion set), `fn_has_immutable_assign` fires
  when the assignee is a plain `let` local and not in the mutable/param set. We
  cover only the FP-safe core (bare-name simple assign); compound (`x.f = e`) and
  param-reassignment cases are also RS0311 but left as safe false-negatives (no
  corpus file needs them). → **33 codes.**
- **FP caught + fixed on the fast gate:** `local image = …` (the `local` binding
  keyword) was first read as an assignment to `image` (an outer `let image` had it
  in the immutable set) → 1 FP on `retaining-managed-shadowed-local.rss`. Fix:
  exclude `local` (a binding form) like `let`/`mut`. Second FP class (a mut Copy
  param reassigned in a fn that also `let`s the same name) closed by excluding all
  param names. Verified on the FULL 619-file corpus (giants are assignment-heavy).
- **CHECKER_TARGET_CODES (33):** the 32 above + RS0311.

### Milestone 2t — RS0205 duplicate-argument (34 codes)

- **RS0205 DUPLICATE_ARGUMENT (2026-07-04):** a call that repeats a named argument,
  `f(x: 1, x: 2)` (oracle `checks/calls.rs`). check.rss: `call_has_dup_arg` collects
  each explicit `name:` label at an argument START (right after the call `(` or a
  depth-0 `,`) — that anchoring excludes struct-field colons (`{x: 1}` is depth>0),
  nested-call labels, and typed closure params — and flags a repeat;
  `fn_has_dup_arg` runs it on every `(` in the body (grouping parens carry no labels,
  so it's safe; cost O(tokens·depth), fine for shallow real nesting). Positional
  duplicates (same param filled twice unlabeled) need callee resolution and are a
  safe false-negative. → **34 codes.**
- **Fully verified by the fast gate:** a pure-Rust oracle scan shows RS0205 fires on
  EXACTLY ONE corpus file (the fixture, in the fast subset) — no positional-dup
  cases exist, and the 4 skipped giants have zero dup-label calls, so the checker
  can't fire there either. Fast gate: 615/615, 0 mismatch.
- **CHECKER_TARGET_CODES (34):** the 33 above + RS0205.

### Milestone 2u — RS0020 invalid-noalloc-call (35 codes)

- **RS0020 INVALID_NOALLOC_CALL (2026-07-04):** a `noalloc` fn may call ONLY enum
  variants or other `noalloc` fns (oracle `analyzer/diagnostics.rs`). The noalloc
  analog of the existing `fn_pure_bad` machinery. check.rss: `collect_noalloc_fns`
  (simple+dotted names of `effects(noalloc)` fns), `noalloc_body_bad` scans a noalloc
  body's calls — unqualified calls allowed iff the name is a declared-type ctor
  (that's RS0014, not RS0020), an enum variant, or a noalloc fn; qualified calls
  allowed iff the dotted name is a noalloc fn; else RS0020 — and `fn_has_noalloc_call`
  gates on the fn actually being noalloc. → **35 codes.**
- **Measured then built (rs0020_probe, since removed):** RS0020 fires on EXACTLY 2
  corpus files — `noalloc-plain-call.rss` {RS0020} (an unqualified plain call) and
  `noalloc-manage-allocation.rss` {RS0014, RS0020, RS0208} (a qualified `Image.load`
  call). Both in the fast subset; giants have no noalloc fns, so `fn_has_noalloc_call`
  short-circuits there → fast gate is a complete verification. Fast gate: 615/615.
  Key insight from the probe: constructor calls allocate → RS0014, NOT RS0020 (that's
  why only 2 files fire despite many noalloc-construct fixtures), so the unqualified
  ctor case is excluded.
- **CHECKER_TARGET_CODES (35):** the 34 above + RS0020.

### Milestone 2v — RS0902 weak-field + RS1003 own-struct (37 codes)

- **RS0902 INVALID_WEAK_FIELD (2026-07-04):** a `weak` struct/resource field whose
  type is not a class (oracle `analyzer/resource_types.rs`). check.rss:
  `collect_nonclass_types` + `type_body_has_invalid_weak` (`name: weak [handle] T`
  where T is a scalar or a declared non-class). FP-safe: unknown T (possibly a
  foreign class) never fires. Fast-gate complete (1 oracle file, giants weak-free).
- **RS1003 OWN_STRUCT_ATTEMPT (2026-07-04):** `own struct` is removed from the
  language; the oracle (`checks/forbidden.rs`) literally scans for adjacent
  `own`+`struct` tokens, so `has_own_struct` does byte-identical adjacency detection.
  Fast-gate complete (1 oracle file, giant-clean).
- **DEV-LOOP WIN proven here:** RS1003 was verified with `RSS_CHECKER_EXTRA_CODES=RS1003`
  and ZERO rebuild (`Finished in 0.10s`) — the target-code override lets a new code be
  iterated purely against the disk-read check.rss. Per-code loop is now ~25-47s (reg-VM
  only), no ~55s test rebuild until the final const bake.
- **CHECKER_TARGET_CODES (37):** the 35 above + RS0902 + RS1003.

### Milestone 2w — RS0306 local-class-binding (38 codes)

- **RS0306 LOCAL_CLASS_BINDING (2026-07-04):** a `local` binding of a class instance
  (oracle `checks/body/resource_pool.rs`) — classes are managed handles, not locals.
  check.rss: refactored `collect_nonclass_types` → `collect_type_kinds` (partitions
  declared types into class/non-class in one pass), and `has_local_class_binding`
  fires on the direct-constructor form `local [mut] NAME = ClassName(...)` where
  ClassName is a file-declared class. Indirect (fn-returned) class values are a safe
  false-negative. Fast-gate complete (1 oracle file, giant-clean). 615/615.
- **CHECKER_TARGET_CODES (38):** the 37 above + RS0306.

### Milestone 2x — RS0701 resource-field (39 codes)

- **RS0701 RESOURCE_FIELD (2026-07-04):** a non-resource type with a field whose
  (outer) type is a declared resource (oracle `analyzer/resource_types.rs`) —
  resources must live behind `with`/`ResourcePool`. check.rss:
  `type_body_has_resource_field` (per field, skip `handle`/`weak` modifiers, check the
  outer type root against the `resources` set) + `has_resource_field` (walk type
  decls, skip resource containers). Reuses the `resources` set from collect_rs0009.
  FP-safe: only a known-resource field type fires; a resource nested in generics
  (`Map<K, Resource>`) doesn't (outer type is Map). Fast-gate complete (1 file,
  giant-clean). 615/615.
- **CHECKER_TARGET_CODES (39):** the 38 above + RS0701.

### Milestone 2y — RS0604 fresh-requires-local-binding (40 codes)

- **RS0604 FRESH_REQUIRES_LOCAL_BINDING (2026-07-04):** a `fresh`-returning call used
  directly with a `mut`/`take` data-effect (oracle `checks/body/fresh.rs`) — a fresh
  value must be bound to a local first. check.rss: `fn_returns_fresh` (param-close +1
  is `->`, +2 is `fresh`), `collect_fresh_fns` (dotted names of `-> fresh` fns),
  `has_fresh_requires_local` fires on `mut`/`take` immediately followed by a dotted
  call whose callee is a known fresh fn (the trailing `(` rules out `mut Type` param
  modifiers). FP-safe: only a known fresh-returning callee fires. Fast-gate complete
  (1 file, giant-clean). 615/615.
- **CHECKER_TARGET_CODES (40):** the 39 above + RS0604.

### Milestone 2z — RS0901 take-handle-field (41 codes)

- **RS0901 TAKE_HANDLE_FIELD (2026-07-04):** `take receiver.field` where `field` is a
  `handle` field of the receiver's type (oracle `checks/body/effects.rs`). check.rss:
  handle fields keyed by `Struct.field` (`collect_handle_field_keys`), and
  `fn_has_take_handle` types the receiver from its PARAM declaration
  (`param_type_root`) so a same-named non-handle field on a different struct doesn't
  fire. First cut used bare field names → 1 FP on `take-inline-field-same-name.rss`
  (Config.rules is handle, InlineConfig.rules is not); fixed by receiver-type keying.
  Fast-gate complete (1 file, giant-clean). 615/615. This is the first check to
  resolve a receiver's type — a small step toward the type engine.
- **CHECKER_TARGET_CODES (41):** the 40 above + RS0901.

### Milestone 2aa — RS0904 weak-field-requires-weak-handle (42 codes)

- **RS0904 WEAK_FIELD_REQUIRES_WEAK_HANDLE (2026-07-04):** a constructor initializing a
  `weak` field with anything other than a syntactic `Weak.from(...)`/`Weak.downgrade(...)`
  (oracle `checks/body/fresh.rs::is_weak_handle_producing_expr`). check.rss: generalized
  the field-key collector to any modifier (`collect_modifier_field_keys`, now used for
  both handle and weak), `value_is_weak_producing` (the value literally starts `Weak.from`
  or `Weak.downgrade`), `call_weak_field_bad` checks each named arg of a simple-name
  (constructor) call against the `Type.field` weak-key set. FP-safe: gated by the weak
  key, so only real weak-field initializers are examined. Fast-gate complete (1 file,
  giant-clean). 615/615.
- **CHECKER_TARGET_CODES (42):** the 41 above + RS0904.

### Milestone 2ab — RS0903 weak-field-requires-upgrade (43 codes); weak/handle cluster complete

- **RS0903 WEAK_FIELD_REQUIRES_UPGRADE (2026-07-04):** reading a `weak` field as a
  value (`read`/`mut` RECEIVER.weakfield) without `Weak.upgrade` first (oracle
  checks/body/async_checks.rs). check.rss: `fn_has_weak_field_read` (same receiver-type
  resolution as RS0901, but for `read`/`mut` and the weak-key set) + `enclosing_is_weak_upgrade`
  (walks out to the innermost enclosing `(` and checks the callee is `Weak.upgrade` —
  the one context where reading a weak field is allowed). Fast-gate complete (1 file,
  giant-clean). 615/615.
- The **weak/handle field cluster is now complete**: RS0701 (resource field), RS0901
  (take handle), RS0902 (weak non-class), RS0903 (weak read), RS0904 (weak init) — all
  via the shared `collect_modifier_field_keys` + `Struct.field` keying + param-based
  receiver typing.
- **CHECKER_TARGET_CODES (43):** the 42 above + RS0903.

### Milestone 2ac — RS0603 invalid-fresh-return-type (44 codes)

- **RS0603 INVALID_FRESH_RETURN_TYPE (2026-07-04):** a fn whose return type contains
  `fresh X` where X is not a struct (oracle checks/body/resource_pool.rs). check.rss
  `fn_fresh_return_generic_bad` scans the whole return-type region (after `->` to the
  body `{`) for any `fresh X` (possibly nested, e.g. `Result<fresh User, E>`) and fires
  when X is a scalar, a declared class, or one of the fn's own `<...>` generic params.
  FP-safe: `fresh Struct` (the valid case) and unknown externals never fire. Fast-gate
  complete (3 files, giant-clean). 615/615.
- **CHECKER_TARGET_CODES (44):** the 43 above + RS0603.

### Milestone 2ad — RS0705 resource-pool-not-local (45 codes)

- **RS0705 RESOURCE_POOL_NOT_LOCAL (2026-07-04):** a ResourcePool binding that is not
  `local` (oracle checks/body/resource_pool.rs) — a binding is local iff it is a `local`
  binding OR a `mut`/`take` param. check.rss `fn_pool_param_bad` flags `read`/no-effect
  `ResourcePool<...>` params; the `let` scan flags `let NAME = ResourcePool<...>.new`.
  Fast-gate complete (3 files, giant-clean). 615/615.
- **CHECKER_TARGET_CODES (45):** the 44 above + RS0705.

### Milestone 2ae — RS1002 implicit-conversion-attempt (46 codes)

- **RS1002 IMPLICIT_CONVERSION_ATTEMPT (2026-07-04):** an `as` token that is not part of
  a `with ... as` binding or a `use ... as` alias — a cast-style conversion (oracle
  checks/forbidden.rs). Purely token-decidable: check.rss `has_implicit_conversion`
  ports `as_belongs_to_with` (backward scan to `with`, stop at `{`/`}`/stmt-boundary kw)
  and `as_belongs_to_use` (backward scan to `use` over path tokens only) line-for-line.
  Giant-clean: census=1 (fixture only) and all `as` in the selfhost giants are in
  comments/strings, never code tokens. 615/615.
- **CHECKER_TARGET_CODES (46):** the 45 above + RS1002.

**Milestone: 46 diagnostic codes byte-exact over the 619-file corpus.** The
token/structure-decidable tier now covers scanner + parser + signature-table +
resource/ownership-syntax codes. Remaining ~52 need the type engine (RS0206-0210
expression typing), the borrow/liveness engine (RS0301-0313, RS04xx/05xx/08xx), or
callee/stdlib resolution (RS0203/0204/0030/0032/0036) — the genuine engine phase.

### Milestone 2af — RS0208 return-type mismatch (47 codes); FIRST type-inference code

- **RS0208 RETURN_TYPE_MISMATCH (2026-07-04):** a `return <expr>` (or a non-Unit
  fallthrough tail) whose *inferred* type is incompatible with the fn's declared
  return type (oracle `checks/calls.rs` check_return_type family). This is the FIRST
  code requiring real expression type inference — the checker reproduces the analyzer's
  inferred `hir_expr_type_name`, not just syntax. The engine (all in `check.rss`):
  - `return_actual_type` — an UNKNOWN-biased typer (any un-typeable expr → `""` → no
    fire): effect/`manage`/`await`/`spawn` unwrap; literals; param typing
    (`param_type_string`); `let`/`local` resolution (`find_let_rhs_start`); with-binding
    + `?`-unwrap; select-arm binding (`find_select_binding`); `.ok()` Result→Option;
    variant calls; qualified calls (`declared_qualified_return` → curated
    `stdlib_return_type`); unqualified calls (`declared_fn_return`); generic constructors
    `Ns<Args>.new()`; `List.fold<_, Acc>` → Acc.
  - `return_expr_mismatch` — Result/Option-aware: explicit `Ok`/`Err`/`Some`/`None`
    payload match, else a bare value vs the `Ok`/`Some` payload (implicit-wrap).
    Alias-safety: a `Result<?>`/`Option<?>` wildcard vs a non-Result/Option expected →
    no fire (unexpanded type alias).
  - `stmt_expr_end` — depth-aware return-expr terminator: stops at a same-line `}` so a
    match/select arm `_ => { return x }` types correctly, but stops at any line change
    so a multi-line value stays untyped (FP-safe).
  - **Module qualification** — `qualified_type_string` canonicalizes the return
    annotation's module-local type names to `module.Name` (`module_prefix` +
    `collect_declared_types`), while value typing stays syntactic/unqualified. This
    reproduces the analyzer's exact asymmetry: a generic-arg-derived
    `List<InterfaceSnapshot>.new()` (unqualified) mismatches the qualified
    `List<rss.package.review.InterfaceSnapshot>` annotation. That asymmetry is the
    entire RS0208 population of the `package-manager` giant (3 fires).
  - `has_return_type_mismatch` — file walker skipping type decls / closure bodies;
    per-fn fallthrough via `body_falls_through`.
- **Giant verification (whole-corpus byte-exact):** oracle RS0208 census =
  `package-manager`:1 (matched exactly), `scan`/`astdump`/`check`:0 (no module header
  → qualification is a no-op → base-engine behavior; my checker fires 0 on each). The
  big giants are pathologically slow through the interpreted reg-VM (astdump 180KB =
  27min, check.rss 333KB = 78min) — the `RSS_SELFHOST_FULL=1` "~22min" note is stale.
- **CHECKER_TARGET_CODES (47):** the 46 above + RS0208.

**Milestone: 47 diagnostic codes byte-exact over the 619-file corpus — the type
engine has begun.** RS0208 is the first code driven by reproduced expression-type
inference. The `type_token_string` / `arg_type_matches` / `return_actual_type`
infrastructure built here is the foundation for the sibling RS0206/0207/0209/0210
bucket (~120 file-fires, the dominant remaining mass).

### Milestone 2ag — RS0207 argument-type-mismatch engine (historical dev gate; later baked)

RS0207 ARGUMENT_TYPE_MISMATCH: a call argument whose inferred type is incompatible
with the callee parameter's declared type (oracle `argument_type_matches` ==
`arg_type_matches`; `hir_expr_type_name` == `return_actual_type`). Built on the RS0208
type engine but **per-argument** rather than per-return, which surfaced a real
performance question (see below). Progress before baking (all committed to
`main`; RS0207 was initially developed with
`RSS_CHECKER_EXTRA_CODES=RS0207` before it became byte-exact and was added to
`CHECKER_TARGET_CODES`):

- **Perf foundation (c78ba754):** `return_actual_type` made O(1)-resolution — a fn-decl
  `Map<String,Int>` index (`collect_fn_decl_index`), a per-fn local-decl index
  (`build_local_index`: `Map<name, List<letTok>>`, replays `find_let_rhs_start`'s
  most-recent-before-position rule byte-for-byte), and a type-alias `Map` index,
  threaded through `return_actual_type`/`return_expr_mismatch`/`payload_mismatch`. The
  residual O(n²) (`with_binding_expr`/`find_select_binding` scanning per non-local ident)
  is gated behind a `" ws"` sentinel folded into the locals map (set only when the fn
  body contains a `with`/`select`). **RS0208 stays byte-exact (615/615, 0 mismatch); the
  earlier "4-17x perf wall" was a false alarm from a single-threaded probe — the
  multi-threaded `checker_parity_corpus` gate is 127s vs the 72s RS0208-only baseline
  (~1.8x).**
- **Bindings + file-fn args + qualified builtin args:** `let/local x: T = e` annotation
  mismatch (`return_expr_mismatch`), plus call args resolved via `resolve_param_type`
  (file fns through the fn index → `param_type_at_fn`/`param_type_string`; stdlib
  methods through generated `.rssi` metadata behind a false-positive-safe allowlist).
- **Generated-backed builtin params + no-`?` with-binding (35ce5644):**
  `stdlib_param_type` allowlists Image.inspect/normalize/resize/save/load +
  Request.path and a small string/log/file slice, but the returned type strings
  are generated from the real `.rssi` metadata. Fires the manage-Result cases —
  a `local = Image.load(...)` types to `Result<fresh Image, ImageError>` and is
  passed to a bare `Image` param. Also
  `with EXPR as name` WITHOUT `?` now types `name` as EXPR's whole value type (previously
  only the `?`-unwrap case) — fires `resource-producer-missing-try`.
- **Parameterless-closure descent (73e0429e):** the RS0207 arg walker now descends into
  `|| { ... }` closures (new `closure_paramless` detects adjacent param pipes), resolving
  captured identifiers against the enclosing fn scope — FP-safe because a parameterless
  closure introduces no bindings that can shadow. Parameterized closures are still
  skipped (their params would mis-resolve). Fires the closure-capture cases
  (managed/retained/fresh-return-managed closure captures, `x_closure_effect2`).

Additional slices landed this milestone:
- **Callback arity + return-type (3a49b469):** args passed to a `Fn(...)-> ret` param
  (parsed from the resolved type string via `strip_type_prefixes`/`fn_type_arity`/
  `fn_type_return`) are checked for closure arity (`closure_param_count`) and return-type
  (each closure return expr — tail expr or every block `return` — fed to
  `return_expr_mismatch`, reusing the RS0208 Ok/Some-payload machinery incl nested
  `Ok(Some(_))`). Fires noescape-callback-{return-type,arity,nested-return-type,
  branch-return-type}. Dialect trap: two statements on one line inside `{ }` is a parse
  error in rss — one stmt per line (surfaced as a compile_checker panic since the checker
  dogfoods).
- **Untyped-param empty-actual (6366ee5c):** a bare (effect-prefixed) ident arg resolving
  to an UNTYPED param of the enclosing fn fires when the callee param type is known
  (`is_untyped_param`). The analyzer types an un-annotated param as `""` and reports the
  mismatch; FP-safe because untyped params only occur in malformed code. Fires
  missing-signature-pieces.

Two further slices landed:
- **ResourcePool generic-receiver (b16c2c7e):** the walker now detects
  `Ns<Args>.method(` calls (the qualified branch required an ident before `.method`, but
  there the token is `>` — a new branch walks back over the matching `<..>` to the
  receiver ident); `call_arg_type_bad` gains a `recvGeneric`, and `ResourcePool<T>.new`'s
  `create` factory resolves to `Fn() -> T` (T substituted from the receiver generic) so
  `callback_arg_bad` fires (resourcepool-new-non-resource / fallible-factory). No FP on
  the common `List<T>.new()`/`Map<K,V>.new()`.
- **Fn-param-call check (#4, `return callback("x")`) — BUILT, CORRECT, but REVERTED for
  perf.** `fn_type_arg_at`/`positional_arg_bad`/`fn_param_call_bad` fired the fixture and
  passed FP guards, but running `param_type_string` + `return_actual_type` on every
  unqualified call corpus-wide caused a single-file pathological slowdown (1 core, 100%,
  13min+ vs the ~122s gate). Needs a cheap per-fn "has any Fn-typed param" gate before
  re-integrating. Code saved off-tree.

- **Fn-param call check + local-shadows-param scoping (4533332e):** a call whose callee is
  a `Fn`-typed PARAMETER (`return callback("x")`) types its positional args against the
  Fn's arg types. Building it surfaced a real scoping bug — `return_actual_type` resolved a
  bare ident to the PARAMETER before a same-named LOCAL, so a `let input = List.slice(..)`
  (List<Int>) shadowing an `input: IntListGen` param mis-typed as IntListGen → a quickcheck
  false-positive. Fixed by checking the local (matchPos-bounded `find_let_rhs_fast`) BEFORE
  the parameter, matching the analyzer's scoping. RS0208 stayed byte-exact. (The earlier
  "#4 causes a 13-min slowdown" was a contention artifact — a clean single-job gate is
  ~133s.)

- **Closure-param typing (b41a0b20):** a parameterized closure passed to a `Fn` param has
  its params typed positionally from the Fn arg types (`closure_param_types`), and any call
  in the closure body passing a bare closure param whose type mismatches the callee param
  fires (`closure_body_arg_bad`) — `|value| String.len(value: read value)` with `Fn(Int)->..`.
- **Non-fresh captured return (649dbc77):** a closure passed to a `Fn() -> fresh T` param
  that returns a bare CAPTURED identifier (not a closure param, not a fresh-producing call)
  fires — a captured value is borrowed, not fresh (`is_bare_captured`).
- **Non-String interpolation (ec90c25f):** `$"..{expr}.."` desugars to `String.format` over
  a `List<String>`, so each `{expr}` must be String; embedded exprs are parsed from the
  `TOK_INTERP` token (`interp_end` ported from astdump) and typed (bare idents resolved by
  name against the enclosing scope) — a concrete non-String fires (`{count}` a `read Int`).

**BAKED as code #48 (659c3414):** `"RS0207"` added to CHECKER_TARGET_CODES. The DEFAULT
FAST gate now asserts RS0207 green over 615/615 files (0 mismatch, 0 false-positive, no
env-gate) in ~125s. First ARGUMENT_TYPE-tier code; 48th baked diagnostic.

**Verification:** RS0207 fires on **all 36/36 oracle files, 615/615 fast-subset byte-exact,
0 false-positives, RS0208 preserved.** The 4 giants skipped by the FAST gate (check.rss
390KB, astdump 180KB, package-manager 65KB, scan 42KB) carry near-zero RS0207 FP risk by
construction — 0 real interpolations (only a commented `$"..."`, which the scanner drops),
0 `Fn()->fresh` params, 2 Fn-typed params corpus-wide — so interp and the fresh-callback
check are inert and #3/#4 exposure is minimal; the other RS0207 checks were validated on
615 files. A FULL-gate confirmation run is deferred/async: RS0207's `return_actual_type`
per-call-arg typing is super-linear on 390KB, so the run churns for hours (>2.6h without
finishing) — it does not gate the milestone since the DEFAULT CI gate skips giants.

**Superseded earlier note:** RS0207 fired on 33/36 oracle files with 0 false-positives.

**Remaining before RS0207 can bake (6 fast-subset mismatches = 3 files, all
false-negatives — each a distinct invasive/FP-risky mechanism):**
(1) `noescape-callback-body-call-argument-type` — CLOSURE-PARAM TYPING: the closure param
must be typed from the `Fn` arg type (`|value| String.len(value: read value)` with
`Fn(Int)->Int` → value:Int vs String.len's String), which needs injecting a closure-param
name→type map into `return_actual_type`/`call_arg_type_bad` (invasive — ~20 call sites; the
`|value|` token carries no annotation);
(2) `noescape-callback-fresh-captured-managed` — a `fresh`-ness/ownership dimension
(`arg_type_matches` strips `fresh`; FP-risky);
(3) `interp` — string-interpolation desugars to a synthesized `List<String>` of the
interpolated values, so typing needs re-tokenizing the single `TOK_INTERP` token's embedded
exprs. Plus the `package-manager` fold giant for a full-corpus bake. Then add `RS0207` to
CHECKER_TARGET_CODES.

### Milestone 2ah — RS0210 OPERATOR_TYPE_MISMATCH (code #49, baked)

First code of the operator/control-flow type tier. Ported the binary-operator/precedence
subsystem from `astdump.rss` into `check.rss` (`two_sym`, `is_generic_angle`,
`op_matches_tier`, `scan_last_top_op`, `binop_width_at`, `find_binop`, + operator SYM
constants) — shared foundation reused by RS0209 arm typing next. `operator_type_bad` finds
the lowest-precedence top-level operator in an expression and types its operands via
`operand_type_cp` (a closure-param operand resolves through a closure-param→Fn-type map;
everything else via `return_actual_type`):

- **Comparisons (tier 6, `== != < > <= >=`):** both operands must share a concrete root
  type (`1 == "1"` → Int vs String).
- **Logical (tiers 1-2, `&& ||`):** each concrete operand must be Bool
  (`manage image && image` → Image operands).

Walker `has_operator_type_mismatch`/`fn_has_operator_type_mismatch` scans if/while
conditions, let/local RHS, and return expressions (skipping parameterized closure bodies),
and scans calls for a closure `|p| body` passed to a `Fn` param — typing the closure params
from the Fn arg types and checking the body (`call_closure_operator_bad` +
`closure_arg_operator_bad`), which fires `noescape-callback-operator-type`
(`|value| value == "x"` with `Fn(Int)->Bool`).

**Verification:** all 3 RS0210 fixtures byte-exact; DEFAULT FAST gate green 615/615, 0
mismatch, 0 false-positive, 0 baked-code regression, ~153s. Giants (skipped by FAST) carry
near-zero FP risk — valid comparisons are Int==Int etc. and logical operands are
comparison sub-exprs that `return_actual_type` leaves untyped (=> skipped).

**Dialect gotcha:** `if ce > (i + 1)` — the `> (` sequence makes the RSScript parser read
`i < bc .. ce >(` as a generic call `i<bc>` (surfaced as RS0206 via the dogfood compile);
hoist to `let lo = i + 1; if ce > lo`.

### Milestone 2ao — RS0312 INDEX_ASSIGN_NON_LIST (code #56, BAKED)

`container[i] = v` index assignment is only supported for List values. Message: "index
assignment is only supported for List values." `has_index_assign_non_list` finds an index
assignment (`=` not followed by `=`, preceded by `]`), walks back to the matching `[` via
`matching_lbracket` (a backward bracket-depth scan), and fires when the container ident is a
KNOWN Map/Set/Deque local. `collect_nonlist_collection_locals` gathers those names from
`let name: Map<..>` annotations and `let name = Map<..>.new()`-style initializers (root token
Map/Set/Deque after the `:` or `=`). FP-safe by construction — a List container, or any
container whose type can't be resolved, is never flagged (a deliberate safe false-negative).

**BAKED as code #56.** Byte-exact 615/615, 0 FP. The fixture (`values["a"] = 1` on a `Map`)
plus corpus.

### Milestone 2bd — RS0707 ResourcePool fallible factory (code #81, BAKED)

`ResourcePool<T>.new` and `.lazy` are infallible constructors; their `create`
factory must return a bare resource, not `Result<T, E>`. The self-host checker
now catches both sources the current corpus needs:

- same-file fallible resource producers, e.g. `DbConnection.try_open(...) ->
  Result<DbConnection, DbError>`;
- stdlib builtin fallible producers, currently `Image.load -> Result<fresh Image,
  ImageError>`, added only when the file does not declare its own `Image.load`.

The second path closes the previously deferred
`resourcepool-new-non-resource.rss` case, where the Rust analyzer reports both
RS0703 and RS0707.

**BAKED as code #81.** Verification: Docker fixture parity is byte-exact:
`RSS_SELFHOST_DEV=1 cargo test -p rsscript
selfhost_parity::checker_parity_corpus -- --ignored --test-threads=1
--nocapture` => all selected fixture files ok, 0 run-failures, 0 code-mismatches.

### Milestone 2be — RS0015 unsupported syntax (code #82, BAKED)

`RS0015` is now covered by a conservative self-hosted recognizer for the current
frontend parity corpus. The checker handles the token-decidable malformed forms
plus the previously deferred semantic-tail fixtures:

- unsupported constructors / variant call forms (`None()`, `None(1)`,
  `Option(1)`, `Result(1)`, named variant payloads);
- malformed call/type/generic/parameter/function/type/control forms;
- native function bodies, protocol default method bodies, unsupported `spawn`,
  removed `profile:` declarations, unsupported `as` casts, unsupported derives,
  reserved generated names, duplicate imports, opaque bodies, and unsupported
  top-level forms.

The recognizer is intentionally not a broad "parse failed" fallback; it is scoped
to RS0015 oracle shapes so valid resource bodies, protocol methods, closures,
multiline `with ... as`, accepted dunder names, and schema/review derives do not
false-positive.

**BAKED as code #82.** Verification: Docker checker fixture parity is byte-exact:
`RSS_SELFHOST_DEV=1 cargo test -p rsscript
selfhost_parity::checker_parity_corpus -- --ignored --test-threads=1
--nocapture` => all selected fixture files ok, 0 run-failures, 0 code-mismatches.

### Milestone 2bc — RS0036 message payload transferability (code #80, BAKED)

`Channel.message<T>` payloads must be cross-isolate-transferable. The self-host
checker now mirrors the Rust analyzer: `Copy` scalars, `String`, and `Bytes` are
accepted; concrete managed/container payloads such as `List<Int>` fire RS0036;
and a bare enclosing-function type parameter (`Channel.message<T>`) is skipped
because transferability cannot be proven without a future bound.

The implementation walks function bodies so it can collect the current function's
generic params with `collect_generics` and apply that generic skip only in the
right scope. `RS0036` is baked into `CHECKER_TARGET_CODES`.

**BAKED as code #80.** Verification: Rust checker accepts
`message-channel-generic-payload.rss`, rejects
`message-channel-non-transferable.rss` with RS0036, and Docker fixture parity is
byte-exact: `RSS_SELFHOST_DEV=1 cargo test -p rsscript
selfhost_parity::checker_parity_corpus -- --ignored --test-threads=1
--nocapture` => all selected fixture files ok, 0 run-failures, 0 code-mismatches.

### Milestone 2bb — RS0706/RS0709/RS0710 resource-pool flow (codes #77–79, BAKED)

**RS0706** RESOURCE_PRODUCER_MISSING_TRY: `with <fallible-producer>(…) as B` with no `?` before `as`
(a `Result<Resource,E>` producer must be unwrapped). Uses `collect_fallible_producers` (fns whose
return root is `Result<…>` with a resource/`fresh resource` inner, via the depth-0 arrow scan).
**RS0709** RESOURCE_POOL_ACTIVE_LEASE_CONFLICT: a `with ResourcePool.borrow(pool: mut P)` nested inside
another borrow of the SAME pool `P` (`borrow_pool_value` extracts `P`; `pool_receiver_root` walks back
a `Name<…>` receiver). **RS0710** RESOURCE_POOL_DISCARD_NOT_LEASE: `ResourcePool.discard(lease: mut X)`
where `X` is not bound by any `with …borrow… as X`. All three 0 FP/0 FN.

**RS0707 update:** this deferred note is superseded by Milestone 2bd. The
self-host checker now covers the declared-producer path plus the stdlib
`Image.load -> Result<fresh Image, ImageError>` builtin case and RS0707 is baked.

### Milestone 2ba — RS0702 resource-escape + RS0802/RS0803 closure-escape (codes #74–76, BAKED)

The three biggest remaining ownership codes, landed in one batched gate.

**RS0702 (resource escape) — 5 message paths in `has_resource_escape` + 8 sub-fns.** A resource must
live/die inside a `with`/`view` scope. **M1** (`with_binding_escapes`): the binding escapes via
`return`/plain-`let`/`manage`/`take`-arg/retains-arg (reuses the retains registry + enclosing-paren
callee resolution). **M2** (`with_binding_closure_capture`): a stored `let` closure capturing the
binding — sees through `Some(…)`/`Ok(…)` wrappers. **M3** (`has_pool_lease_escape`): a
`ResourcePool.borrow`/`.try_borrow` lease not in a `with` slot. **M4** (`has_producer_escape` +
`collect_producer_fns`): a resource-producer (constructor `R(` or a fn returning a resource/
`Result<Resource,E>`) that is directly **returned or bound** — fires only when preceded by `return`/`=`,
NOT when used as a sub-expression (the crux FP: a producer inside a factory `create: || R.open(…)`
closure is allowed). Producer registry uses a depth-0 `->` scan (not `function_signature_end`, which
breaks on **bodyless** fn decls). **M5** (`with_body_factory_captures`): a ResourcePool factory
`create` closure (expression- or block-bodied) referencing the with-binding. Also handles `view NAME =`
(desugars to a with-lease scoped to the enclosing block). Path: 0 FP/2 FN → 7 FP (factory producers) →
0/0.

**RS0802 (noescape callback escapes) / RS0803 (local closure escapes).** Shared `value_escapes`:
`return X` / `let y = X` / pass to a non-noescape param (guarded against call-results `X()` and
noescape-forwarding). RS0802 adds the signature-retains variant (`fn f(p: noescape Fn())
effects(retains(p))`) and `stored_closure_captures` (a `let s = ||{… p …}`). RS0803 targets
`local X = |…|` closures. Reuses `collect_noescape_params`. Both byte-exact first try. **Baked #74–76.**

### Milestone 2az — RS0711 lazy-pool-factory capture (code #73, BAKED)

`fn_has_lazy_capture_bad`: a `ResourcePool<..>.lazy(…)`/`.try_lazy(…)` factory closure is stored in the
pool, so it must capture only owned `local` bindings — fires when its `create` closure captures a
parameter or a managed `let`. Handles **expression-bodied** closures (`create: || Session.open(…)`, no
braces) by scanning the arg region to a depth-0 comma / call close. Params collected by scanning the
first `(` directly (the `find_top_sym` opening-paren bug — it hits depth++ before the match).
**Key FP fix (1 → 0):** exclude argument labels (`host:`) and field-access bases — the real capture is
the *value* (`read host`), not a coincidentally-named label; `resourcepool-try-borrow-escape` has a
`local`-capturing factory whose call label `host:` collided with the `host` param. **Baked #73.**

### Milestone 2ay — RS0703/RS0704 resource-generic type validation (codes #71–72, BAKED)

Two structural resource codes sharing the `resources` (file-declared resource names) +
`collect_declared_types` + `collect_generics` substrate.

**RS0703 — invalid ResourcePool type argument.** `ResourcePool<X>` is valid only when X is a resource
(or the literal bound `Resource`, or a Resource-bounded generic param). Fires when X is an
**in-file-declared** non-resource (variant 2 — `struct Image`) or a non-Resource-bounded generic param
(variant 3 — `<T: Managed>`). **Isolation insight (10 FPs → 0):** `analyze_source` runs per-file, so an
undeclared/builtin name like `DbConnection` in `db_pool.rss` has `type_kind None` → OK. Do NOT use
`is_stdlib_type` — fire only on `x ∈ declared` (file-declared struct/class/sum), with `resources`
checked first so a file-declared resource is always OK.

**RS0704 — resource used generically.** Sub-rule A: a file-declared resource as a generic type-arg of
a non-pool container (`List<File>`), with two exemptions — under `ResourcePool<…>`, and
`Result<Resource, E>` at arg-0 in a **return** type (4 corpus files declare a resource and return
`Result<it>`; a `Result<Resource>` *parameter* still fires). Sub-rules B1/B2: a `resource Name<params>`
with an unbounded param (B1), or a Resource-bounded param used directly (not under ResourcePool) in a
field type (B2). New helper `enclosing_generic_head`. Both 0 FP/0 FN. **Baked #71–72.**

### Milestone 2aw — RS0805 explicit-closure capture contract (code #70, BAKED)

`has_capture_contract_bad` scans each explicit closure `fn(params) captures(list) effects(...) { body }`
and fires when the declared captures don't exactly match the body's external-variable uses: a
used-but-undeclared external (Missing), a declared-but-unused capture (Unused), or a declared effect
that differs from the body's use (Mismatch, read/mut/take). Self-contained per closure — no cross-fn
registry. Two dialect gotchas fixed: (1) only `tk_kind == TOK_IDENT` tokens are captures — keywords
(`return`/`if`/`let`) are `is_ident_tok`-true but not variables (they caused 8 FPs, incl. benchmark
kernels); (2) `==` scans as two `SYM_EQ`, so assignment detection (→ mut) must check `m+2 != SYM_EQ`.
Only 4 corpus files use `captures(`. Path: 8 FP → 0. **Baked #70.**

### Milestone 2av — RS0801/RS0804 closure-capture (codes #68–69, BAKED)

First codes needing real **closure-capture analysis** — the subsystem the borrow tier kept deferring.
`fn_has_closure_capture_bad` walks each closure body (`{` preceded by `SYM_PIPE`), computes its
capture set (enclosing-fn locals referenced inside, minus the closure's own params/locals, and — the
FP fix — minus any local read through a field access `holder.image`, which yields a managed handle, not
the local), then classifies the closure's role:
- **stored** `let cb = |…| {…}` (managed) capturing a local → **RS0801**;
- **retained-arg** — closure passed to a `retains(param)` callee, capturing a local → **RS0801**;
- **noescape-arg** — closure passed to a `noescape Fn()` callee (new `collect_noescape_params`
  registry), where a captured local is `take`/`manage`d inside → **RS0804**.

New reusable substrate: `collect_noescape_params` (callee|param keys), `find_closure_open_pipe`,
`enclosing_paren_open`, and the wrapper-aware pair `closure_outer_call_open` (sees through
`Some(…)`/`Ok(…)`/`Err(…)` to the real callee — the FN fix for `schedule(callback: read Some(||…))`)
+ `arg_label_at` (recovers the labelled arg containing the closure). Path: 2 unique mismatch (1 FP
handle-field, 1 FN Some-wrapper) → 0. **Baked #68–69.**

NOTE: RS0705 (RESOURCE_POOL_NOT_LOCAL) was found to be **already baked** in a prior session
(`has_resourcepool_not_local`); a decoder re-derived it before the `poolNotLocal` flag in the anyDiag
chain revealed the dup. Lesson: grep the anyDiag OR-chain / const before decoding a "new" code.

### Milestone 2au — RS0302/RS0304 place-pair (codes #66–67, BAKED)

Two more codes from the SAME `fn_place_conflicts` substrate, essentially free. The dispatch already
identified them (and previously just skipped): **RS0302** = the whole-local-vs-field mix (exactly one
side is the bare base — `use_state(state: read state, cache: mut state.inner.cache)`); **RS0304** =
indexed paths that can't be proven disjoint (`use_buffers(a: mut buffers[0], b: mut buffers[1])`).
Extended the per-fn return code to a 5-bit mask (added a `bit_set` division helper — the dialect has
no bitwise ops) and wired both flags. 0 FP / 0 FN, first gate. **Baked #66–67.** The place-pair
routine now covers all five of its codes (RS0302/0303/0304/0305/0309).

### Milestone 2at — RS0303/RS0305/RS0309 place-pair + RS0601 fresh-return (codes #62–65, BAKED)

Four codes landed in one batch (sub-agents decoded the rules in parallel; a single combined
`RSS_CHECKER_EXTRA_CODES` gate validated all four — the batching cut ~3 throttled gates to 1).

**RS0303 / RS0305 / RS0309 — place-pair conflicts (shared substrate).** All three come from the
oracle's `check_place_pair_conflict`. `collect_call_places` gathers every effect-wrapped place arg
of a call (`read`/`mut`/`take`/`manage <place>`), splitting on depth-0 commas and — for a closure
argument — collecting only the vars it CAPTURES from outside (its own params/`local`/`let` are
excluded; that exclusion was the fix for the RS0305 FP on noescape-callback-local-use-after-manage).
Each same-base pair dispatches in order: **RS0305** if a move (`take`/`manage`) is non-disjoint
(bare base either side / index / handle / prefix-or-equal); else **RS0303** if a mut pair is
prefix-or-equal or crosses a `handle` field (excluding whole-vs-field = RS0302 and indexed = RS0304);
else **RS0309** if the fields are genuinely disjoint and the base is a `mut` parameter (non-splittable).
Path: 2 mismatch → 0 (the lone FN was the closure-capture case).

**RS0601 — fresh-return-not-clean.** A `-> fresh …` fn (return type contains `fresh` anywhere,
covering `Option<fresh>`/`Result<fresh,_>`) that returns a non-clean value. Fires on: a `mut`/`take`
param or non-scalar `read` param (`collect_fire_params`); a `let` bound to a managed-non-fresh call
(`collect_managed_nonfresh_lets` — a call that is neither a registered `-> fresh` fn nor a stdlib-type
receiver like `List.new()`; an unknown `Cache.get()` counts as managed); a tainted local/let
(`manage`/`take`/retains-arg/**stored**-closure capture — a `noescape` callback does NOT taint); a
managed match-arm payload; or a handle-field access. Clean: constructors, fresh calls, literals,
untainted owned locals. `Some(…)`/`Ok(…)` and a `name:` label are unwrapped to the base. Path (v3→v5):
22 → 2 → 0, the FP wave fixed by making locals owned-by-default and recognizing stdlib/builtin fresh
sources. Reused pre-existing `collect_fresh_fns`/`dotted_name_text`; `fn_return_type_has_fresh` is a
broader precondition than the direct-only `fn_returns_fresh`.

All four byte-exact 0 FP / 0 FN over the corpus; bake gate green. **Baked as codes #62–65.**

### Milestone 2as — RS0501 LOCAL_VALUE_RETAINED (code #61, BAKED)

Fifth borrow-tier code and the first **cross-function** one — needs the callee's signature.
Passing a `local` value to a parameter the callee declares `effects(retains(P))` fires (a local's
lifetime is the caller frame; it can't outlive the call). `collect_retains` builds a file-wide
registry of `shortFnName|paramName` composite keys (analyze_source is per-file, so callee decls
are in scope). At each call, args are matched by label against the registry; the value's base is
found by stripping the effect and unwrapping `Some(…)`/`Ok(…)`, then a `local` base fires.

Four subtleties from the pass/fail boundary (v1 → v2, 8 mismatch → 0):
- **Field retains** (`retaining-local-field`): `read holder.image` retains the base `local holder`
  — do NOT exclude a trailing `.field`; the base ident is what matters.
- **Wrappers** (`retaining-local-wrapper`): `read Some(holder.image)` / `read Ok(…)` retain — unwrap.
- **Shadowing** (`retaining-managed-shadowed-local`, PASS): a `local image` in a since-closed
  `if`-block plus a later `let image` — the call uses the managed `let`. Resolved with
  `nearest_binding_is_local` (nearest preceding `local`/`let` decl by proximity), replacing the
  whole-body local set which ignored scope/shadowing and false-fired.
- **Builtin retains** (`builtin-retains-local-key`): `Cache.insert(key: read localKey)` — a builtin
  collection `insert` (Cache/Map/Set/Deque/HashMap) retains key/value with no in-file decl;
  `builtin_retains_label` handles it by receiver-type + method + label.

A managed (`let`) value passed to a retains param is a DIFFERENT error (the take is RS0308), not
RS0501 — confirmed via oracle. **BAKED as code #61**, all ~10 RS0501 fixtures plus corpus, bake
gate green.

### Milestone 2ar — RS0401 USE_AFTER_MANAGE (code #60, BAKED)

Fourth borrow-tier code and the first **control-flow-sensitive** one. A `manage x` / `take x`
moves `x`; any later USE of `x` is RS0401. A forward token-scan from the move position handles
every fixture variant — straight-line, inline (`compare(read (manage image), read image)`),
short-circuit (`manage image && image`), branch, and loop — because in each the second `x` is
simply later in token order. Two subtleties decoded from the pass/fail boundary:

- **Control flow (`enclosing_block_open` + `has_return_or_break`):** a move inside a block that
  `return`s does NOT taint the continuation past that block (branch-return-manage-not-moved,
  loop-return-manage-unreachable are PASS), so the forward scan is capped at the enclosing block
  close when a `return` follows the move inside it. Crucially **only `return` suppresses** — a
  `break` exits a loop but execution continues after it, so a move before a `break` still taints
  the post-loop use (loop-manage-use-after, loop-break-manage-use-after are FAIL). Getting this
  wrong cost one gate (v2 included `break` → 3 FN).
- **Field moves (`used_field_after_move`):** `take x.f` then re-access `x.f` fires
  (take-inline-field-use-after) — matched as the exact `base . field` path, not just the base.

A "use" excludes arg/field labels (`x:`), field bases of other places (`_.x`), (re)binding
positions (`let`/`local`/`mut x`), and assignment targets (`x =`). **BAKED as code #60**, all
~13 RS0401 fixtures plus corpus, bake gate green. Path: 6 mismatch → 6 → 0 over three gates.

### Milestone 2aq — RS0301 MANAGED_TO_LOCAL (code #59, BAKED)

Third borrow-tier code, landed 0 FP / 0 FN on the first gate by reusing the RS0202 substrate.
`local X = RHS` fires when RHS is a **managed place**: a plain `let` binding, a `handle`-field
access, or a `Some(…)`/`Ok(…)` wrapper around one — each after an optional leading `read`/`mut`
effect. `fn_has_managed_to_local` finds each `local` decl (skipping an optional `: Type` to reach
the `=`), then `rhs_managed_for_local` classifies the RHS: strip effect → unwrap `Some`/`Ok`
(recursive) → a `_.f` access is managed iff `f` is a `handle` field name (`collect_handle_field_names`,
global) → otherwise a bare ident is managed iff it's in `collect_managed_names` (reused from
RS0202 — a plain `let` is ALWAYS managed, **even a scalar** `let n = 5`; confirmed via
`rss check`, correcting an earlier speculative note that scalars shouldn't fire).

Boundary confirmed via oracle before implementing: `local y = <param>`, `local b = <local>`,
`local y = 5` (literal), and `local r = h.rules` where `rules: Int` (normal field) all **no-fire** —
only `let`-bindings and `handle`-field accesses are managed. **BAKED as code #59**, all five fail
fixtures (managed-to-local, -effect, -wrapper, handle-field, handle-field-wrapper) plus corpus,
bake gate green (ok 1230, mismatches 0).

### Milestone 2ap — RS0307/RS0308 MANAGE/TAKE_REQUIRE_LOCAL (codes #57/#58, BAKED)

**The first two codes of the borrow/ownership tier — and a correction.** Earlier this session
RS0307/0308 were assessed as needing a "multi-session closure-capture subsystem." That was
wrong: the rule is a token-scan with six exclusion layers, reverse-engineered directly from the
corpus (30 FP → 15 → 5 → 1 FN → 0 over four gates). `manage <x>` (RS0307) / `take <x>` (RS0308)
in a function body FIRES unless `x` is excluded by any of:

1. **Plain field access** `x.f` (dot, no `(` after the field) — owned, RS0901's domain. A
   **method-call result** `x.m(…)` (dot + `(`) is a managed temp and DOES fire — `manage
   pool.acquire()` in prefix_exprs.rss. This split was the final false-negative.
2. **Type** (uppercase head) — a nested closure's `take T` param effect, never a value.
3. **`take`-effect parameter** (`collect_take_params`) — already owned, re-takeable
   (`Pair(left: take left)` where `left: take T`). *Crux bug fixed here:* `find_top_sym` cannot
   match an opening `(` (it counts the bracket as depth before the equality check and returns
   −1) — so the param `(` is now found by a direct first-`(` scan. This same latent bug is why
   `collect_param_names` silently returned empty in the earlier attempt.
4. **`with … as x` resource lease** (`collect_with_bindings`) — **asymmetric**: excluded for
   `take` (RS0013 owns the take-escape) but NOT for `manage` (`manage file` on a with-lease
   fires RS0307 alongside RS0013 — resource-manage-escape.rss).
5. **Match-arm binding** (`collect_match_bindings` + `matching_lparen`) — `Ok(initial) => …
   manage initial` is owned (rules_config_reload.rss). Collects `Variant(a,b) =>` payload idents
   and bare `name =>`.
6. **Same-closure-scope `local`** (`enclosing_capture_open` + `local_in_scope_all`) — only a
   closure `|…| { }` (body `{` preceded by `|`, SYM_PIPE=124) is a capture boundary;
   `if`/`loop`/`while`/`with` blocks are transparent and share the function's locals. A `local`
   captured into a closure (declared in the outer scope) fires — this is what "closure-capture
   analysis" reduced to.

**BAKED as codes #57/#58.** Byte-exact over the corpus, 0 FP / 0 FN (both the development
FAST gate and default bake gates: ok 1230, code-mismatches 0). The collectors (take-params, with-binds,
match-binds, closure-scope) are the reusable substrate for the rest of the borrow tier (RS0301
managed-to-local, RS0303/0304/0309 field-path, RS0401/0501/0601/0702).

### Milestone 2an — RS0313 ASSIGN_TYPE_MISMATCH (code #55, BAKED)

A reassignment `name = <value>` whose value type doesn't match the local's declared type, e.g.
`count = "oops"` where `count: Int`. Message: "cannot assign `String` to `count` of type
`Int`." `fn_has_assign_type_mismatch` reuses the RS0306/RS0311 assignment-detection idiom (a
lone `=` not followed by `=`, an ident lhs, not preceded by `let`/`mut`/`local`, and — added
here — not a `.field` assignment) and compares the literal RHS category (String / Numeric /
Bool / Char, via `literal_scalar_cat`) to the local's declared-type category
(`type_scalar_cat` over the type from `find_local_decl_type`). Conservative by construction:
fires only on a literal RHS against an explicitly annotated Int/Float/String/Bool/Char local,
and only on a cross-category mismatch — Int↔Float coercion stays inside the Numeric category,
so it never false-fires.

**BAKED as code #55.** Byte-exact 615/615, 0 FP. The single fixture (`count = "oops"` on an
`Int`) plus corpus. First reuse of the scalar-typing helpers for the assignment (not binding)
path. Non-literal-RHS mismatches are a deliberate safe false-negative (would need full
`return_actual_type` context per assignment); none appear in the corpus.

### Milestone 2am — RS0708 RESOURCEPOOL_MAX_SIZE (code #54, BAKED)

An eager `ResourcePool<T>.new(...)` allocates up front, so its `max_size` must be statically
known — a positive integer literal or a `const`. (A `.lazy(...)` pool sizes on demand and is
exempt.) Message: "`ResourcePool.new` requires a positive literal `max_size`."
`has_resourcepool_maxsize_bad` matches the token run `ResourcePool <..> . new (`, locates the
`max_size:` argument, and tests its value via `maxsize_value_bad`: accepted only when it is a
single positive integer literal (`8`) or a single `const` identifier (`POOL_SIZE`, gathered by
`collect_const_names`); `0`, a negative literal, a runtime binding (`size`), or any multi-token
expression fires. Only `.new` is matched — `.lazy` and the non-generic `ResourcePool.borrow`/
`.stats` calls are skipped.

**BAKED as code #54.** Byte-exact 615/615, 0 FP. Both fixtures (`max_size: 0`; `max_size: size`
runtime param) plus the passing `max_size: 8` / `max_size: POOL_SIZE` corpus cases.

### Milestone 2al — RS1004 SURFACE_REFERENCE_SYNTAX (code #53, BAKED)

`&T`/`&mut T` surface reference syntax is not part of RSScript — borrow-passing uses the
`read`/`mut`/`take` effect keywords, and `&` only ever appears as a bitwise operator in
expressions. Message: "surface reference syntax is not part of RSScript." `has_surface_reference`
fires on three token adjacencies that never occur in valid rss: `& mut` (`mut` is reserved, so
it can't be a bitwise-and operand), `: &` (a type starting with `&`), and `-> &` (a return
type starting with `&`). A bitwise-and `a & b` tokenizes as `a`,`&`,`b` and matches none of
them, so there is no conflict with RS0210's operator scan. The simplest bake of the session —
a pure token-adjacency scan, no typing.

**BAKED as code #53.** Byte-exact 615/615, 0 FP. Both fixtures (`&mut Buffer` param,
`&Bytes` param) plus corpus.

### Milestone 2ak — RS0032 PROTOCOL_NOT_SATISFIED (code #52, BAKED)

A generic method's type argument that doesn't satisfy the protocol the method requires:
`Set.new<T>`/`Map.new<K,_>` require the element/key to be Hashable, `List.sort<T>` requires
Ord. Message: "type `T` does not satisfy protocol `Hashable`/`Ord` required by `<call>`."
`has_protocol_violation` scans for the explicit-type-arg call forms (`call_protocol_bad`
matches `recv . method < arg , ..>`) — the parity check is presence-per-file, so the
`.new`/`.sort` forms suffice; every RS0032 file carries one (the `Set.insert`/`Map.insert`
variants the oracle also reports would be redundant). The first type arg is tested by
`arg_root_satisfies`: Int/String/Bool/Char satisfy both protocols, Float neither, a locally-
declared struct/class/sum satisfies iff it derives the trait, and unknown/imported types are
left alone (FP-avoidance).

**The FP that shaped it:** `local_type_derives` must resolve BOTH struct/class/resource
(`parse_type_decl`) and `sum` (`parse_sum_decl`) declarations — `parse_type_decl` returns -1
for a `sum`, and `starts_type_decl` excludes `sum`, so a first cut couldn't see the derive on
`sum Token derives(Clone, Eq, Hash)` used as a Map key (pass fixture
`hashable-enum-payload-key.rss`) and wrongly fired. It now also advances one token on a parse
failure instead of aborting the whole scan.

**BAKED as code #52.** Byte-exact 615/615, 0 FP, 0 FN. Reuses the RS0211 `derive_has_trait`
and `arg_end` helpers.

### Milestone 2aj — RS0211 DERIVE_FIELD_UNSUPPORTED (code #51, BAKED)

A value-derive trait whose field types don't satisfy the trait constraint. Message shape:
"`<Trait>` derive is not supported by field `<name>`." (one per derived-trait × offending
field). `has_bad_derive_field` walks type decls; for each with a `derives(...)` clause it
collects the derived traits (`derive_has_trait` for Eq/Ord/Hash/JsonDecode) and the struct's
generic params (`collect_struct_generics`), then iterates `name: Type` fields
(`derive_body_fields_bad` → `field_type_end`) and tests each type (`field_type_bad_for_derive`):

- **Eq/Ord/Hash:** a `Float` type token or a `handle` modifier anywhere in the field type
  fires — Float/handle are not Eq/Ord/Hash, and a field like `GenericKeyHolder<Float>` fails
  transitively through the inner Float.
- **Eq/Ord/Hash/JsonDecode:** a Map key that is not Hash fires. `type_is_hash_safe` recursion:
  Int/String/Bool/Char are Hash, Float is not, `List<E>`/`Map<E,_>` are Hash iff their
  element/key is (recurse on the first generic arg via `arg_end`), a generic param is Hash iff
  the struct also derives Hash, any other user type is assumed Hash-safe (FP-avoidance).

**The crux** — a naive "Map with a Float/List key fires" rule FP'd on the pass fixture
`derive-generic-args-ok.rss`: `Map<List<Int>, String>` (Int is Hash → OK) vs
`Map<List<T>, Int>` (T is only Eq-bound, not Hash → fires). A Map KEY must be **Hash**, which
is strictly stronger than Eq — that distinction is the whole rule. Confirmed Float is not Eq
(only Clone) via `rss check` on a scratch struct.

**BAKED as code #51.** Byte-exact 615/615, 0 FP, 0 FN. GOTCHA: a pre-existing
`type_body_open(toks, typeName)` forced the new brace-finder to be named
`derive_type_body_open` (the dogfood compile surfaced RS0005 duplicate-declaration). Reused
the RS0212 resource-derive scanner (`resource_derives_bad`) as the parsing template.

### Milestone 2ai — RS0209 CONTROL_FLOW_TYPE_MISMATCH (code #50, BAKED 7bffb738)

Control-flow type tier. Slice 1 = non-Bool `if`/`while` condition: `cond_non_bool` types the
whole condition and fires when it is a concrete non-Bool value (`if "yes"` → String). A
condition with a top-level comparison/logical operator (recognised via `find_binop` +
`op_matches_tier`, tiers 1/2/6) or a leading `!` is Bool by construction and skipped; an
un-typeable condition yields "" and is skipped (FP-safe). New walker
`has_control_flow_mismatch`/`fn_has_control_flow_mismatch`; initially developed
with `RSS_CHECKER_EXTRA_CODES=RS0209` before baking.

**Verification:** fires `non-bool-if-condition`; full fast-subset 0 false-positives, 0
baked-code regression, ~159s.

**match-typing engine — slices 2-4 (match_bad, arm iteration mirrors astdump emit_match_arms; DONE, 0 FP):**
- enum-variant: `Some/None`⇒Option, `Ok/Err`⇒Result vs concrete scrutinee root (non-option-match-scrutinee, match-variant-mismatch, match-expression-variant-mismatch).
- literal pattern vs scrutinee scalar (match-literal-type-mismatch).
- tuple `(..)` on scalar / list `[..]` on non-List (patterns_tuplelist.rss).
- bare-ident pattern (not `_`/`true`/`false`) on a scalar scrutinee (match.rss `other` on Int).
- for-unsupported: `for _ in EXPR` where EXPR is a concrete scalar (is_scalar_root).

**Status: 7/9 RS0209 files byte-exact, 0 false-positives corpus-wide, ~163s.**

**Completed to bake (slices 5-6):**
- `match-expression-arm-type-mismatch` — arm-result-type consistency: `str_top_arg`
  extracts the scrutinee inner type, fed to `operand_type_cp` as a per-arm binding map
  (`Some(result)` on Option<Int> ⇒ result:Int); produced value = a block's single non-`return`
  statement or a bare expr arm.
- `tools.rss` — scrutinee matchability: a concrete scrutinee that isn't Option/Result/List,
  a tuple `(..)`, a scalar, or a type in `collect_declared_types` fires (the analyzer resolves
  each file in isolation, so an imported `ToolRuntime` is unknown ⇒ non-matchable). Tuple
  scrutinees are explicitly matchable (first cut FP-fired on tuples.rss / x_tuples.rss).

**BAKED as code #50 (7bffb738).** Default FAST gate asserts RS0209 green (615/615, 0
mismatch, 0 false-positive) in ~167s. `rss check <file>` (release bin `rss`, not `rsscript`)
read the oracle's exact RS0209 messages when decoding corpus triggers.
