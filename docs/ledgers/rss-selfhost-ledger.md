# RSS Self-Hosting Ledger

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
- **Symptom:** an IO-free, loop-heavy tool *still* shows `translated: 0,
  native_calls: 0` — the JIT accelerates none of it.
- **Minimal RSS:**
  ```
  let mut xs = List<Int>.new()
  while i < n { List.push<Int>(list: mut xs, value: read i); i = i + 1 }
  while j < List.len<Int>(list: read xs) { total = total + List.get<Int>(list: read xs, index: j); ... }
  ```
- **Backend:** jit-native (and tier-0).
- **Root cause:** two gaps compound. (1) Collection *construction/mutation*
  (`List.push`, `Map.insert`) is not in the native subset at all. (2) The
  read ops that *are* native (`ListLen`/`ListGet`/`GetFieldSlot`) only fire when
  the collection is a **handle parameter** — handles never originate in native
  code, so a locally-built `let mut xs` can't be read natively. Real tool code
  builds and processes collections locally, so the Phase-2 read-heap coverage
  rarely applies.
- **Classification:** JIT (coverage) + VM (representation).
- **Decision:** this is the measured case for **Phase 3 (local mutation)**: to
  accelerate real tool loops the native tier needs (a) native `List.push`/
  `Map.insert` on locally-owned collections and (b) native reads of *local*
  (not just parameter) collections — i.e. handles that originate from native
  `MakeList`/`MakeMap`, with the VM's copy-on-write/aliasing rules. Larger than
  a single helper; recorded as the next high-value JIT direction with real-program
  evidence (rather than guessed from microbenchmarks).
- **Tests:** `backends_agree_on_stdlib_reporter` (5-way).
- **Benchmark:** `selfhost_stdlib_reporter.rss` in the matrix.
- **Status:** open (informs Phase 3).

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
- **Decision:** worked around in the Mailbox driver (match the *owned* Option from
  the take call directly, so `v` is an owned `Int`). The general auto-deref fix is
  the same shape as the earlier `read_effect_lowers_by_value` work; recorded for a
  scoped follow-up.
- **Status:** open (worked around).

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
- **Decision:** worked around by dropping the placeholder pre-fill and growing the
  ring lazily on send (`if tail < len { set } else { push }`), so the generic
  element only enters the list via the proven `read value` send path. The general
  AOT auto-clone fix is the SH-010 follow-up.
- **Status:** open (worked around).

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

### SH-027 — AST-dump parity COMPLETE: streaming rss producer at 619/619 byte-exact

- **Context:** completes the SH-025 AST-structure arm (step 1 of the 3-step
  frontend-object-parity goal). The self-hosted streaming producer
  (`selfhost/astdump.rss`) now matches the Rust oracle (`parse_source_raw` via
  `crate::selfhost_parity`) **byte-for-byte over the ENTIRE corpus**.
- **Reach:** **619 / 619** corpus files byte-exact (**100%**), **0 run-failures**.
  `AST_CORPUS_PARITY_FLOOR = 619`; `ast_parity_samples` fast gate over 62 curated
  `samples/ast/*.rss` (added coverage for every construct fixed in the final push).
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
  rss parser that STREAMS the canonical dump (`selfhost/AST_FORMAT.md`); the
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
- **Status:** open, but well-formed grammar is COMPLETE — 92.5% byte-exact with
  every remaining mismatch a malformed-recovery fixture. The producer + parity gate
  + risk mitigations (curated fast gate; module-story decision in AST_FORMAT.md) are
  in place; the deferred residuals (malformed recovery, protocols, `@L:C:N` spans)
  are additive and tracked by the ratcheting floor.

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
    - **RS0036** (payload-not-transferable) — needs message-payload Send/transferability
      analysis. → **needs type inference**. RS0038 (char-literal) still has 0 corpus
      fixtures.
- **THE TOKEN-DECIDABLE TIER IS EXHAUSTED; the cross-function signature table adds exactly
  RS0022 (25 codes total).** The remaining candidates SKIPPED because they need type
  inference / callee-signature resolution (measured, not ducked): RS0201, RS0013, RS0202,
  RS0036 (all above). None is blocked on *borrow* analysis specifically — they are all
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
  tail, RS0015 stays **OUT of CHECKER_TARGET_CODES**; it is planned for the semantic-frontier
  phase alongside RS0207-0210 / RS0301-0313 / RS0025-0026. RS0025/RS0026 have 0 corpus
  fixtures (skip, as noted in 2j).
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

### Milestone 2l — type-inference engine slice 1: conservative `expr_type_root` + RS0013 (FALLBACK: foundation committed, RS0013 left OUT of the gate)

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
- **BLOCKER (why RS0013 is NOT in CHECKER_TARGET_CODES):**
  `tests/fixtures/fail/ast-call-missing-effect-nested.rss` (fn returns
  `Result<Unit, IOError>`) fires sub-rule C on TWO **qualified stdlib** operands —
  `File.open_write(..)?` and `File.write(..)?`, whose error type the oracle knows
  is `FileError` (≠ `IOError`). Reproducing that needs per-method stdlib
  error-type inference on qualified/receiver calls, which the spec deliberately
  keeps UNKNOWN (typing them risks corpus-wide false positives on the many clean
  `File.*?`/`Json.*?` calls whose error type matches). So RS0013 cannot reach
  0-mismatch at the spec's conservatism level — FALLBACK taken.
- **Outcome:** `expr_type_root` + the RS0013 sub-rule A/B wiring are committed and
  exercised across all 619 corpus files (0 run-failures). RS0013 is EMITTED by the
  checker but stays OUT of `CHECKER_TARGET_CODES` (filtered from parity), so the
  gate is unchanged. It fires correctly on 11 of the 13 oracle-RS0013 files (all
  sub-rule A + B); the 2 sub-rule-C files stay unflagged (the documented blocker),
  and every clean `?` file stays unflagged (0 false positives, verified).
- **Gate:** `checker_parity_corpus` 619/619 ok, 0 mismatches, 0 run-failures at
  the SAME 26 codes (RS0013 absent). Green.
- **Next slice:** RS0013 becomes gateable once qualified-call error-type inference
  exists; meanwhile `expr_type_root` is ready for RS0210/RS0207/RS0208.

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
