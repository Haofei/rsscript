# Self-Hosting rss as a Stress Test — Plan

**Status:** proposed (2026-07-01)
**Goal:** Port a real slice of the rss toolchain — **lexer → parser → a checker pass** —
into rss itself, run it over rss's own source corpus, and mine the process for two kinds
of finding:

- **GAP** — a language feature rss can't express, a compiler bug, or a runtime crash.
- **PERF** — a path that is pathologically slow versus the Rust reference implementation.

Self-hosting is the *vehicle*; the findings are the *product*. A working self-hosted
front-end is a welcome byproduct, not the success criterion.

**This plan extends the existing self-hosting ledger, not a parallel one.** Findings land
in `docs/ledgers/rss-selfhost-ledger.md` as `SH-NNN` entries (currently through SH-015,
already recording language/stdlib/VM/JIT/AOT gaps and perf from earlier self-hosted tools).
Both GAP and PERF findings use that same schema and ID space — no separate `GAPS.md` /
`PERF.md`. The parser/checker port is a larger, deliberate producer of `SH-NNN` entries.

Target chosen: **Parser + checker (deeper)** — go past pure syntax into semantics, because
that is where the language is least exercised by existing `.rss` programs and where the
richest gaps live.

---

## Why this is a rigorous stress test (not vibes)

Two assets make every phase self-checking:

1. **A reference implementation already exists** — the Rust lexer/parser/analyzer. We diff
   rss-in-rss output against it via **existing test harnesses / private test helpers**, not
   new user-facing or internal CLI commands (UI direction: don't add commands unless
   absolutely necessary). Lexer/parser oracle data is produced by a `#[cfg(test)]` helper
   or a hidden test-only path, kept out of the shipped CLI surface.
2. **A `.rss` corpus already exists** in the repo — ~540 files (run
   `rg --files -g '*.rss' | wc -l` for the exact count; 542 at time of writing): fail-fixtures,
   pass-fixtures, vm/jit benchmarks, micro-benchmarks, and example/package files, including
   17 existing `selfhost-*` programs up to 955 lines. A large, adversarial input set on day one.

So each phase has a built-in oracle — but **the corpus is not uniform**: it contains
valid programs, parse-failures, and check-failures. The oracle must branch by category
(derive the category from the fixture directory + the Rust reference's own verdict):

| Input category      | Lexer oracle            | Parser oracle                              | Checker oracle                                    |
|---------------------|-------------------------|--------------------------------------------|---------------------------------------------------|
| **Valid** (parses + checks) | token stream == Rust | canonical AST == Rust parser's AST         | no diagnostics (matches Rust `rss check`)         |
| **Parse-fail** (invalid syntax) | token stream == Rust (up to the error, where lexing is well-defined) | parse **diagnostic code + span** == Rust's | n/a (never reaches the checker)                   |
| **Check-fail** (parses, fails semantics) | token stream == Rust | canonical AST == Rust parser's AST         | checker **diagnostic code + span** == Rust's      |

Compare on **diagnostic codes + spans**, never human-readable message text (which drifts).

---

## Findings already surfaced (before porting a line)

The method works immediately — spiking the riskiest question produced findings:

- **GOOD — recursive AST works via `handle`.** Verified on the VM:
  ```rss
  sum Expr { Num(value: Int)  Add(left: handle Expr, right: handle Expr) }
  fn eval(e: read Expr) -> Int {
      match read e {
          Num { value }       => { return read value }
          Add { left, right } => { return eval(e: read left) + eval(e: read right) }
      }
  }
  ```
  evaluates correctly (no arena / integer-ID workaround needed). This de-risks the whole
  port — the natural recursive-descent + recursive-AST design is viable.

- **GAP #1 — multi-field variant destructuring is not positional.** `Add(l, r) => …`
  fails with `RS0026 unknown binding`. You must use struct-style `Add { left, right }`.
  Single-field positional (`Circle(r)`) *does* work, so the two-field failure is an
  inconsistency worth filing.

- **GAP #2 — structured patterns hard-require a scrutinee effect** (`RS0202`): you must
  write `match read e { … }`, not `match e { … }`, whenever a pattern projects fields.
  Correct-by-design, but constant friction a self-hosted parser/checker will hit.

---

## Language capability baseline (from review)

Confirmed sufficient for a front-end **in the interpreter tier**:

- Recursion: fully supported in the VM (depth cap `VmLimits::max_depth`, default 16,384 —
  `DEFAULT_MAX_DEPTH` in `reg_vm/mod.rs`; ample for recursive-descent over real files).
- Sum types w/ payloads, exhaustive `match`, guards, struct/tuple/list-slice patterns.
- Generics for containers (`List<T>`, `Map<K,V>`, `Set<T>`, `Option`, `Result`).
- Rich `String`/`Char` intrinsics (chars, char_at, index_of, classification, code points).
- File I/O + `Args` (read source, take a path argument).
- `Result<T,E>` + `?`, first-class **owned Fn** (storable/returnable closures).

Known constraints to design around:

- No direct inline-recursive types → use `handle`/`class` indirection (verified above).
- No closure capture of *mutable* state → thread `mut pos` params explicitly.
- No `const` expressions / macros → build keyword & dispatch tables at runtime.
- `i64` only, monomorphic generics, no reflection — none block a front-end.
- Native JIT accelerates numeric/loop kernels, not tool code: parser/checker workloads are
  dominated by strings, maps, local collections, `CallIntrinsic`, and `Result`/`Option`
  diagnostics, none of which are native-eligible — so they run on the VM (dev/parity) and
  AOT (the only fast path). This is measured, not a missing feature: see ledger SH-001 and
  the SH-004/006 update (JIT ≈ VM on collection code; AOT ~144×). The relevant lever is
  cheaper VM value-representation / intrinsic dispatch, per the parked perf roadmap.

---

## Plan

### Phase 0 — Harness, oracle, ledger wiring (foundation)
- Use a **separate worktree for isolation** (`rsscript-wt-selfhost`, off main HEAD); build
  via the dev container (`docker compose -p rsscript run --rm dev`), serialized against the
  warm target volume; commit only when parity gates pass.
- Produce lexer/parser oracle data from **existing test harnesses or a `#[cfg(test)]` /
  test-only helper** — do **not** add `--dump-tokens` or any AST-dump CLI command (UI
  direction: no new user-facing/internal commands unless absolutely necessary). If the
  Rust parser has no in-test AST serializer, add one behind `#[cfg(test)]`.
- Build a corpus runner over all `.rss` files (count via `rg --files -g '*.rss' | wc -l`)
  that **categorizes each file** (valid / parse-fail / check-fail) and applies the matching
  oracle from the table above; summarize pass/fail per category.
- Findings go into the existing `docs/ledgers/rss-selfhost-ledger.md` as new `SH-NNN`
  entries — no separate `GAPS.md` / `PERF.md`.

### Phase 1 — Lexer in rss (`selfhost/lexer.rss`)
- Char-by-char tokenizer; runtime-built keyword `Map`.
- **Oracle:** token stream == Rust lexer dump over all corpus files.
- Watch for: string-building throughput, `Char`/`String` intrinsic coverage gaps.

### Phase 2 — AST + recursive-descent parser in rss (`selfhost/ast.rss`, `selfhost/parser.rss`)
- Recursive `sum` AST with `handle` fields; recursive descent threading `mut pos`.
- **Oracle (category-aware):** *valid* and *check-fail* files → canonical AST == Rust
  parser's AST; *parse-fail* files → parse diagnostic **code + span** == Rust's. (The
  corpus includes fail fixtures, so "round-trips identically" holds only for the
  successfully-parsing categories.)
- Watch for: **recursion depth** on deeply-nested expressions (cap is 16,384 — unlikely to
  hit, but if it does that's a finding + a `VmLimits` tuning question), pattern-match
  ergonomics, `Result`/`?` error plumbing.

### Phase 3 — Checker pass in rss (`selfhost/check.rss`) — the "deeper" target
- Build a symbol table (`Map<String, Def>`) and reproduce a well-scoped subset of the
  frontend checks (start with name resolution + exhaustiveness; expand as budget allows).
- **Oracle:** checker diagnostics (**codes + spans**) match the Rust analyzer — pass-fixtures
  produce no errors, check-fail fixtures produce matching diagnostics. The fail/pass fixture
  directories are a ready-made labeled test set.
- Watch for: representing HIR-like info without reflection, cross-node graph walks with
  `handle`/`weak` (cycle handling), diagnostic-set equality.

### Phase 4 — Perf pass (the "is anything slow" half)
- Time the full rss-in-rss front-end vs. the Rust front-end across the corpus — a real
  macro-benchmark, a far better perf signal than the existing micro-kernels.
- Rank hotspots (string building, map lookups, recursion overhead, DeepCopy pressure) as
  `SH-NNN` PERF entries in the ledger; feed them into the parked collection-representation /
  perf roadmap. A real workload is exactly the trigger that work was waiting for.

### Stretch (separate decision)
- Extend the checker toward more passes, or add a lowering/formatter arm to close a fully
  self-hosted tool. Larger commitment; decide after Phase 3 evidence.

---

## Risks & mitigations
- **Recursion depth cap on big files** → measure early (Phase 2); if hit, either raise
  `VmLimits` for the tool or restructure to iterative parsing — log the decision as a GAP.
- **Checker oracle drift** (diagnostic wording vs. codes) → compare on **codes + spans**,
  not human text.
- **Scope creep in the checker** → fix a small, explicit check set up front; expand only
  with remaining budget.
- **Warm-tree stale-binary trap** → gate every phase in a **fresh worktree**, full suite.

## Discipline (RSS rules)
- Use a separate worktree for isolation; commit only when the parity gates pass.
- Sub-agents port; orchestrator gates parity against the category-aware oracle over the
  full corpus.
- Findings are the deliverable — recorded as `SH-NNN` entries in
  `docs/ledgers/rss-selfhost-ledger.md`; green oracle runs are the floor, not the goal.

---

## Results (2026-07-01)

Implemented in worktree `rsscript-wt-selfhost` (detached off main), sub-agents porting,
orchestrator gating parity. Harness lives in `crates/rsscript/src/selfhost_parity.rs`
(`#[cfg(test)]` — zero new public API, zero new CLI; reaches the private lexer/parser and
the VM entry point directly). It compiles each rss tool once (`reg_vm_compile_source`) and
runs it on the reg-VM in-process, passing the corpus file's *content* as `argv[0]`.

| Phase | rss tool | Oracle | Corpus result |
|-------|----------|--------|---------------|
| 1 — lexer | `selfhost/lexer.rss` | `crate::lexer::lex` (canonical token dump) | **556/556 tier-0**, 0 run-failures |
| 2 — parser | `selfhost/parser.rss` | `crate::syntax::parse_source_raw` (accept/reject) | **556/556 recognition** |
| 3 — checker | `selfhost/check.rss` | `crate::analyze_source` (code `RS0005`) | **556/556** |
| 4 — perf | lexer on VM vs native | wall-clock over corpus | **~46× slower** (post-SH-022; was ~5100×) |

Gates (all green): `cargo test -p rsscript --features native-jit --lib` (3 tiny-sample
tests plus a non-ignored negative-parser and positive-`RS0005`-checker smoke test, so
a degenerate always-`OK`/always-`CLEAN` tool is caught without the corpus gate); the
corpus gates run with `-- --ignored` (`lexer_parity_corpus`, `parser_parity_corpus`,
`checker_parity_corpus`, `lexer_perf_corpus`).

### Findings (ledger `SH-016` … `SH-023`) — historical; current status per item

These are the findings the stress test surfaced. **All of the actionable ones have
since been fixed or closed** — the table above (556/556, ~46×) is the current state;
the list below records what each finding was and where it landed. See the linked
ledger entry for the full write-up.

- **SH-016** — *(was)* no character-literal syntax; `'` lexed to `?`, cascading a
  misleading `RS0013`. **FIXED** — `'x'` is a real `Char` value end-to-end (and a
  stray out-of-inventory char now lexes to an `Unknown` token the parser reports,
  not `?`; empty/multi-char literals are `RS0038`). *(language + diagnostics)*
- **SH-017** — *(was)* statement-level binary-operator expressions couldn't cross a
  newline; the leading-operator form compiled but was silently wrong. **FIXED** —
  operator-continuation lexing is supported. *(language + correctness diagnostics)*
- **SH-018** — *(was)* no cursor/state object; `mut` params couldn't advance a
  cursor. **FIXED** — scalar-`Copy` `mut` params are reassignable with caller
  write-back (and, as of this review, `mut` field/element *places* are written back
  too, on both backends). *(language ergonomics)*
- **SH-019** — *(was)* a `fresh`-returning fn couldn't build its result via `mut` +
  `List.push` (`RS0601`). **FIXED** — managed fresh bindings survive the flow merge.
  *(analyzer/freshness)*
- **SH-020** — *(was)* recursive descent had to encode `(ok, new-index)` as a
  sentinel `Int`. **CLOSED** — not a real gap; tuple returns work (the original
  over-claim was corrected). *(ergonomics)*
- **SH-021** — `parse_source_raw` defers body validation, so recognition parity
  under-tests the grammar (deep grammar is enforced in the analyzer, not the
  parser). **Accepted methodology limitation** — partly mitigated by the
  non-ignored negative-parser / positive-`RS0005` smoke tests. *(methodology)*
- **SH-022** — *(was)* the self-hosted lexer was ~5100× slower on the reg-VM than
  native Rust. **FIXED (→ ~46×).** NOTE: the original "per-character intrinsic
  dispatch" hypothesis was **wrong**; the real root cause was **O(n²)** — every
  lexer helper's `read List<Char>` param got an eager prologue `DeepCopy` that the
  elision pass KEPT (taint flowed through `List.get` to the extracted `Char`, and
  `Char.*` were classified `Keep`). The fix classifies the pure scalar `Char.*`
  intrinsics as `PureFreshReader` so the copy is elided (VM-only, parity-safe):
  lexer/VM 79.5 s → 732 ms (~108×), VM-vs-native 5100× → ~45.6×. *(VM elision)*
- **SH-023** — the checker reaches `RS0005` parity; the load-bearing rule is the
  analyzer's **merged callable namespace** (fn names + type-constructor names
  collide). *(insight)*

### Current residual perf problem (separate from SH-022)
With SH-022's O(n²) removed, the remaining VM-vs-native gap is a flat **~46×**
general per-op tax (HashMap/`Handle`/`Rc<RefCell>` per op + per-intrinsic dispatch),
not an algorithmic blowup — real code (e.g. the mailbox) already runs ~2.4× native.
The only remaining lever is a `TypedMap`/`TypedSet` representation rewrite (parked;
~10–15% on collection-bound micro-kernels, med-high risk), tracked in the perf
roadmap. Deeper checker passes (name resolution, exhaustiveness) are the natural
next self-hosting step but need a real expression/statement/pattern parser (the
depth `parse_source_raw` let Phase 2 skip — SH-021).

## Next architectural step — frontend object parity (started 2026-07-02)

The agreed next move is from **tool parity** to **frontend object parity**, keeping
the same safe-oracle model. Ordered ladder (a real dependency, not just taste):

1. **AST-dump parity** — the unlock. RS0005 was reachable at the token level, but
   arity/type/effect/resolution diagnostics all need a real AST + symbol table.
2. **Diagnostics parity by category** — largely blocked on (1).
3. **Spans/positions** — last, mirroring the lexer's tier-0→1→2 ladder.

**Step-1 keystone shipped (this is a serialization contract, not parser code):**
- `selfhost/AST_FORMAT.md` — the canonical AST-dump format (indentation tree, one
  node per line, structure+payload at tier 0, spans deferred to a trailing field).
- A **total** Rust oracle serializer over `crate::syntax::parse_source_raw` (the
  surface-preserving tree, never the desugared `parse_source`) in
  `crate::selfhost_parity`.
- Tests: `ast_oracle_dump_tiny_sample_golden` + `ast_oracle_dump_is_deterministic_smoke`
  (non-ignored) pin the format; `ast_oracle_total_over_corpus` (ignored) proves the
  serializer renders all 556 corpus files deterministically without panicking — i.e.
  it is total over the real grammar, the precondition for being a trustworthy oracle.

This deliberately lands **before** `parser.rss` builds an AST (same reason the token
`FORMAT.md` + oracle preceded the rss lexer): the target is fixed and reviewable
first. Two risks to treat as their own line items when `parser.rss` starts emitting
AST: the `scan.rss`-prepend hack (no module/import story) becomes a scaling pain once
an AST type module is added; and corpus-gate runtime (AST dumps ≫ token dumps) will
likely want a sampled fast inner-loop gate plus the full corpus pre-push.

**Step-2 producer shipped (in progress, ratcheting).** `selfhost/astdump.rss` is a
recursive-descent rss parser that STREAMS the canonical dump (no handle-based AST is
materialized). Parity harness: `ast_parity_tiny_sample` + `ast_parity_samples`
(non-ignored, byte-exact vs the oracle over curated `samples/ast/*`) and
`ast_parity_corpus` (`#[ignore]`, ratchets a floor over all 563 files and asserts
**0 run-failures** so a producer crash regresses the gate even if the byte-exact
count still clears the floor). That corpus gate runs the reg-VM over ~560 files and
is slow in a debug build — run it in `--release` for a quick measurement; the fast
non-ignored inner-loop gate is `ast_parity_samples`. Current reach: **121/563
byte-exact, 0 run-failures** (the producer never crashes; unsupported constructs
mismatch rather than panic). Covered: fns (generics/effects/
return/body), struct/class/resource (opaque/generics/derives/handle-weak/defaults/
drop), sum, const/type-alias/module/use, the core statements, and a precedence-exact
expression parser (calls, field/index, array, try, effects, literals). Both risks are
handled: the curated set is the fast inner-loop gate; the module story is decided in
`selfhost/AST_FORMAT.md` (commit to concatenation). The residual grammar (for/match/
closures/object-map/interp/unary-desugars/effect-receiver/attributes/protocols/
line-continuation) is the additive tail, tracked as **SH-025** and gated by the
ratcheting floor.
