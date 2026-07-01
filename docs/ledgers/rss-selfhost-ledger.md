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
- **Status:** decided.

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
- **Tests:** `crate::selfhost_parity::lexer_parity_tiny_sample` (drives the rss
  lexer through the VM against `crate::lexer::lex`).
- **Status:** open (worked around; language decision pending).

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
- **Decision:** rule for all self-hosted rss — **keep a statement-level
  expression on one line**; break long boolean tests into a single line or into
  helper predicates / early-return `if`s. Worked around by making `is_kw` a
  single-line chain. Language-side follow-up: either support operator
  continuation or make the leading-operator form a hard error.
- **Tests:** `crate::selfhost_parity::lexer_parity_tiny_sample`.
- **Status:** open (worked around; language decision pending).

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
  parity (544/544). The general lever (method syntax or a mutable-cursor pattern)
  is a language-ergonomics follow-up, not a blocker. Recorded so the plumbing
  cost of self-hosting stateful passes is visible.
- **Tests:** `crate::selfhost_parity::lexer_parity_corpus` (tier 0, 544/544).
- **Status:** decided (worked around).

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
- **Root cause:** the freshness checker treats a binding written through `mut`
  (or passed as `mut` to `List.push`) as no longer provably fresh, so the natural
  "allocate-empty, fill in a loop, return" builder pattern can't be annotated
  `fresh`.
- **Classification:** language (freshness analysis) — ergonomics gap.
- **Decision:** worked around by dropping the `fresh` annotation
  (`-> List<Tok>`); the value is still a freshly built list, the caller binds it
  to a plain `let` and only reads it. The general lever (let a locally-built,
  never-aliased mutable collection satisfy a `fresh` return) is an analyzer
  follow-up, not a blocker.
- **Tests:** `crate::selfhost_parity::parser_parity_corpus` (recognition, 545/545).
- **Status:** decided (worked around).

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
- **Root cause:** no ergonomic multi-value/tuple return and no mutable cursor, so
  the classic "parser returns Result<Node, Err> while advancing self.pos" shape
  collapses into an overloaded sentinel Int. Fine for a recognizer (which only
  needs accept/reject), but a node-building parser would want a real result
  struct per nonterminal.
- **Classification:** language (ergonomics) + docs — same family as SH-018.
- **Decision:** worked around with the `-1`-sentinel convention; reaches full
  recognition parity (545/545). Recorded so the plumbing cost of a stateful
  recursive-descent pass in rss is visible alongside SH-018.
- **Tests:** `crate::selfhost_parity::parser_parity_corpus` (recognition, 545/545).
- **Status:** decided (worked around).

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

### SH-022 — self-hosted lexer is ~5100× slower on the VM; cost is per-char intrinsic/collection dispatch, NOT string building

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
- **Backend:** vm (also relevant to tier-0/native — this code is intrinsic-bound,
  not native-eligible, cf. SH-001/SH-004).
- **Root cause:** the main loop is O(n) (single `String.chars` + one pass), but does
  ~6 intrinsic dispatches PER CHARACTER — `List.get` on a `List<Char>` (VmValue
  boxing + refcount) plus `Char.to_code`/`is_whitespace`/`is_digit`/… and the
  `code_at` peeks (c1,c2). The dominant cost is the VM's per-op value-representation
  + intrinsic-dispatch overhead over ~712 K chars, exactly the lever flagged by
  SH-001/SH-004/SH-011 and the parked perf roadmap. This is the first REAL-workload
  profile that is unambiguously VM-dispatch-bound.
- **Classification:** VM (value representation + intrinsic dispatch) + stdlib
  (no char-cursor / native String-iteration intrinsic; `String.chars → List<Char>`
  forces per-char boxed access).
- **Decision:** the real lever is cheaper per-char access, NOT string building:
  e.g. a native string byte/char cursor intrinsic (iterate without materializing a
  boxed `List<Char>`), and lower per-intrinsic dispatch overhead. Feeds
  [[perf-refactor-roadmap]] / [[jit-collection-perf-measurement]] with real-workload
  evidence (the trigger that work was waiting for).
- **Measured vs. extrapolated (be honest):** only two things here are *measured* —
  (a) the VM-vs-native table above, and (b) the `String.concat`→`StringBuilder`
  control. The VM-vs-**AOT** split is **NOT measured**: SH-006 measured AOT ~144×
  faster than the VM on comparable tool code, which *would* put an AOT-compiled
  self-hosted lexer near ~0.5 s (~30× native Rust), but this lexer has not actually
  been run under AOT. That AOT number is the piece that would separate *fixable VM
  per-op overhead* from the *inherent per-char intrinsic count* (which AOT also
  pays) — worth measuring as a follow-up (needs a file-reading lexer variant so a
  700 KB input isn't passed via `argv`).
- **Tests / bench:** `crate::selfhost_parity::lexer_perf_corpus`
  (`--release -- --ignored`).
- **Status:** open (VM/stdlib lever identified; feeds perf roadmap; AOT split
  still to be measured).

### SH-023 — self-hosted checker reaches RS0005 parity at declaration level; the merged callable namespace is the load-bearing rule

- **Tool:** self-hosted checker (`selfhost/check.rss`) run on the reg-VM vs
  `crate::analyze_source` filtered to error-severity `RS0005`
  (DUPLICATE_DECLARATION), over the whole 546-file corpus.
- **Symptom (positive):** the checker reproduces RS0005 with **546/546** parity
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
  scanner conservatively STOPS on the first malformed/unknown top-level item
  (mirroring the recognizer), which can only under-report on syntactically broken
  files — safe, since the analyzer emits RS0005 on exactly the 2 well-formed
  fixtures and the other 544 files stay CLEAN (zero false positives).
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
  // fails:  match p        { Both(a, b)      => ... }   // RS0026 on a, b
  // fails:  match p        { Both { a, b }   => ... }   // RS0202 missing effect
  // works:  match read p   { Both { a, b }   => ... }
  ```
- **Backend:** all (frontend / parser + binding resolution).
- **Root cause:** positional binding is only wired for single-field variants;
  multi-field variants must be destructured with named `{ field, … }` patterns,
  which additionally project fields and so require a `read`/`mut`/`take` scrutinee
  effect. The two rules compound into confusing errors for the natural
  `Variant(a, b)` shape.
- **Classification:** language (parser / pattern binding) + docs.
- **Decision:** worked around throughout the self-hosted code by using
  `match read scrutinee { Variant { field, … } => … }`. Recorded here for
  completeness — this was found in the initial spike and used to choose the AST
  representation, but had not been written to the ledger. Language-side: allow
  positional binding for multi-field variants (or emit a targeted diagnostic
  pointing at the missing feature rather than `RS0026`).
- **Tests:** covered indirectly by every `match read … { V { … } }` in
  `selfhost/parser.rss` / `selfhost/check.rss`.
- **Status:** open (worked around; language decision pending).
