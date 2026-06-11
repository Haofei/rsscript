# RSScript (rss) — known compiler bugs

Found by a white-box audit of the compiler on **2026-06-10**, reproduced against the
release binary built from commit `f5fab92` (`target/release/rss`). Every item below was
reproduced; the HIGH items were additionally re-verified by hand.

> **Status (2026-06-10, re-audit + fix):** all 11 items were re-verified against a fresh
> build. **RSS-1 … RSS-10 are fixed** and confirmed across `eval`, `run`, and
> `run --release`, with the full `cargo test` suite still green. **RSS-11** (LOW,
> compile-time perf only) is confirmed valid but **deferred** — see its entry.
>
> Repro notes found during the re-audit:
> - **RSS-5 / RSS-10:** the original single-line struct bodies (`{ first: T  second: T }`)
>   additionally tripped a field-separator quirk that masked the documented symptom; with
>   idiomatic newline-separated fields both bugs reproduce exactly as described and are fixed.
> - Separately discovered (not one of the 11, left open): matching on a borrowed
>   `read Option<T>`/`Result<…>` param and using the bound payload by value
>   (`match o { Some(s) => return s }`) lowers to `&T` and fails rustc E0308. Pre-existing
>   at `f5fab92`; independent of the RSS-4 fix.

Run recipe used for all repros:

```sh
cd rsscript
export RSSCRIPT_RUNTIME_PATH="$PWD/crates/runtime"
BIN=target/release/rss
$BIN check  file.rss          # parse + typecheck only
$BIN run    file.rss          # reg_vm interpreter
$BIN run --release file.rss   # AOT: lower to Rust -> cargo build -> run
$BIN eval   file.rss
```

The recurring theme: **the interpreter, `check`, and the AOT Rust-lowering path disagree.**
The green cargo suite does not exercise the `eval` / `run` / `run --release` triad, operator
precedence trees, or borrow/clone coercion on assignment — which is where every HIGH bug lives.

---

## HIGH

### RSS-1 — [FIXED] — Prefix `!` and `~` bind looser than every binary operator (silent wrong result)
- **Class:** wrong result (no error; both backends agree on the wrong parse)
- **Source:** `crates/rsscript/src/syntax/parser.rs:2464` (`parse_unary_expr` is tried before
  `parse_binary_expr`), `:2960-2968` (`unary_operand_range` extends the operand to the rest of
  the expression)
- **Root cause:** prefix `!`/`~` are parsed before binary operators and their operand is
  extended to the entire remaining expression, so they bind *looser* than every binary op — the
  inverse of all C-family languages. `!a && b` parses as `!(a && b)`; `~a + b` as `~(a + b)`.

```rust
// not_and.rss
fn main() -> Unit {
    let a = false
    let b = false
    let r = !a && b
    if r { Log.write(message: read "T") } else { Log.write(message: read "F") }
}
```
- **Expected:** `(!a) && b` = `true && false` = `F`. **Actual:** prints `T` (parsed `!(a && b)`).
  Also `~a + b` with a=0,b=1 prints `-2` (correct `(~0)+1` = `0`). Wrapping the prefix operand in
  parens gives the right answer, proving the mis-grouping.
- **Fix:** give prefix `!`/`~` precedence above all binary operators.

### RSS-2 — [FIXED] — Integer overflow silently wraps in release AOT but traps everywhere else
- **Class:** soundness / cross-backend divergence
- **Source:** `crates/rsscript/src/rust_lower/lowerer.rs` Add/Sub/Mul emit bare `+`/`-`/`*`; the
  generated release profile sets no `overflow-checks = true`. Sibling: `crates/runtime/src/math.rs`
  `Math.pow` uses `i64::pow` not `checked_pow`.
- **Root cause:** reg_vm uses `checked_*` and traps; the AOT lowerer emits plain Rust arithmetic
  and ships a release profile with overflow checks off.

```rust
// ovf.rss
fn bump(x: Int) -> Int { return x + 1 }
fn main() -> Unit {
  let a = 9223372036854775807
  let b = bump(x: a)
  Log.write(message: read Int.to_string(value: read b))
}
```
- **Expected:** all modes agree (trap). **Actual:** `eval` → "integer addition overflow…";
  `run` → `panic: attempt to add with overflow`; **`run --release` → prints `-9223372036854775808`
  (exit 0)**. Breaks the documented `interp == compiled` invariant; release builds compute wrong
  finite values where every other mode errors.
- **Fix:** lower `+ - *` to `wrapping_*`/`checked_*` (as `<<` already uses `wrapping_shl`), or set
  `overflow-checks = true` in the generated release profile; route `Math.pow` through `checked_pow`.

### RSS-3 — [FIXED] — `read`-param / borrowed RHS not cloned on assignment → non-compiling Rust (E0308)
- **Class:** miscompile (valid program won't compile under AOT)
- **Source:** `crates/rsscript/src/rust_lower/lowerer.rs:1865-1869` (`Stmt::Assign` emits
  `target = lower_expr(value)` with no clone coercion; the `let`-init path at `:1601-1613` *does*
  clone). Aliased-view variant: `let_init_is_clonable_read_param_ref` at `:3915-3941` (returns
  false for `read_view_bindings`, `:3922`).
- **Root cause:** the clone-coercion fix that lets `let mut x = <read-param>` own a `T` was applied
  only to the `let`-initializer path, not to plain assignment (`x = b`) nor to read params reached
  through an alias. Those lower to `chosen: Vec<i64> = b /*&Vec<i64>*/`, which rustc rejects.

```rust
// pick.rss
fn pick(a: read String, b: read String, useB: read Bool) -> String {
  let mut s = a
  if useB { s = b }
  return s
}
fn main() -> Unit {
  Log.write(message: read pick(a: read "AA", b: read "BB", useB: read true))
}
```
- **Expected:** prints `BB`. **Actual:** `check` ok, `eval` prints `BB`, **`run` fails** with
  `error[RS1101] … mismatched types` (rustc E0308 at `s = b`). Hits `String`, `List<T>`, any
  non-Copy struct. (This is the "known open" let-mut bug from the port TODO — still live, plus the
  new aliased-view variant.)
- **Fix:** apply the same `.clone()` coercion in `Stmt::Assign` lowering; also clone when the RHS is
  a read-view alias (`:3922`).

### RSS-4 — [FIXED] — Untyped `Some(...)` / `Ok(...)` local bypasses argument type checking (false-accept)
- **Class:** soundness (false-accept → backend E0308)
- **Source:** `crates/rsscript/src/hir.rs:2431-2459` (`infer_hir_expr_type` → `None` for
  `CallResolution::EnumVariant`), `:1422-1446` (`Stmt::Let` records `type_name=None`),
  `crates/rsscript/src/checks/calls.rs:1330-1345` (arg check skipped when actual type is `None`).
- **Root cause:** `let o = Some(5)` gets no inferred type, so when `o` is passed where
  `Option<String>` is required, the arg check is skipped and `Option<Int> → Option<String>` is
  accepted. rustc then rejects the lowered Rust. The inline form `describe(o: read Some(5))` *is*
  caught (RS0207) — only the untyped let escapes.

```rust
// optbug.rss
fn describe(o: read Option<String>) -> String {
  match o { Some(s) => { return s } None => { return "none" } }
}
fn main() -> Unit {
  let o = Some(5)
  Log.write(message: read describe(o: read o))
}
```
- **Expected:** rejected at `check`. **Actual:** `check` ok, then `run` → E0308 (`expected
  &Option<String>, found &Option<i64>`).
- **Fix:** infer `Option<Int>` / `Result<Int,_>` for enum-variant constructors so the local carries
  a type.

### RSS-5 — [FIXED] — Turbofish struct constructor miscompiles to a positional tuple-struct call (E0423)
- **Class:** miscompile
- **Source:** `crates/rsscript/src/rust_lower/lowerer.rs:2342` (named-field path looked up via the
  raw callee string), fall-through at `:2646`; `type_kinds` keyed by bare name at `:58`.
- **Root cause:** the named-field constructor path is gated by `type_kinds.get(name)` where `name`
  is the raw callee `"Pair<Int>"` (turbofish embedded), but `type_kinds` is keyed by the bare
  `"Pair"`. The lookup misses, so the call falls through to a positional emission
  `Pair(11i64, 22i64)` against a named-field struct → rustc E0423.

```rust
// turbofish_ctor.rss
struct Pair<T> derives(Clone, Eq, Hash) { first: T  second: T }
fn main() -> Unit {
    let p = Pair<Int>(first: 11, second: 22)
    Log.write(message: read Int.to_string(value: read p.first))
}
```
- **Expected:** prints `11`. **Actual:** `check` ok, `run` → E0423. Non-turbofish
  `Pair(first:…, second:…)` lowers correctly.
- **Fix:** use `type_root_name(name)` for the `type_kinds` lookup at `:2342`.

---

## MEDIUM

### RSS-6 — [FIXED] — `<<` / `>>` share the comparison precedence tier
- **Source:** `crates/rsscript/src/syntax/parser.rs:2883-2891` (`ShiftLeft`/`ShiftRight` grouped
  with `Equal`/`Less`/`Greater` in `find_top_level_operator`).
- `4 == 1 << 2` parses as `(4 == 1) << 2` (shift on a bool). In C/Rust shift binds tighter than
  comparison, so the correct parse is `4 == (1 << 2)` = `4 == 4` = true. `check` passes; both
  backends then fail (`no method named wrapping_shl for type bool`, rustc E0599). False-reject.
- **Fix:** move `<<`/`>>` to a tier between additive and relational.

### RSS-7 — [FIXED] — Lexer accepts malformed multi-dot number literals (`1.2.3`, `5.`)
- **Source:** `crates/rsscript/src/lexer.rs:114-134` (`lex_number` consumes any run of digit-or-`.`).
- `1.2.3` and `5.` tokenize as one Float; `check` reports `ok`; the lowerer emits the verbatim
  token `let x = 1.2.3;` → rustc E0610. False-accept that defers to a confusing backend error.
- **Fix:** reject more than one `.` (and trailing `.`) in `lex_number`.

### RSS-8 — [FIXED] — Mixed numeric operands skip operand-type checking
- **Source:** `crates/rsscript/src/checks/forbidden.rs:377-381` (empty arm), `:341-358`.
- `Float + Int` and `Float < Int` are not type-checked (the `==` path correctly rejects), so they
  pass `check` then fail the backend with E0277/E0308.
- **Fix:** require matching numeric roots in the arithmetic/relational arms.

### RSS-9 — [FIXED] — reg_vm compares floats bitwise, diverging from AOT's IEEE `==`
- **Source:** `crates/rsscript/src/.../vm_value.rs:308` (Float `PartialEq` via `to_bits`).
- `NaN == NaN` → true and `0.0 == -0.0` → false in the interpreter; AOT uses IEEE `==` and gives
  the opposite. A real interp-vs-compiled divergence.
- **Fix:** use IEEE `==` in the Float equality arm.

### RSS-10 — [FIXED] — Generic struct constructor drops its type argument
- **Source:** `crates/rsscript/src/hir.rs:3391`, `:2483` (`constructor_sig_from_type` returns the
  bare `"Wrap"`).
- `let w: Wrap<Int> = Wrap(item: 7)` is wrongly rejected (RS0207) because the constructor's return
  type loses `<Int>`. False-reject.
- **Fix:** synthesize `Wrap<T>` and substitute the type arg.

---

## LOW

### RSS-11 — [VALID · DEFERRED] — ~O(n³) `check` blowup on deeply nested generics (compile-time DoS surface)
- **Source:** `crates/rsscript/src/checks/calls.rs:2052-2142` (`collect_type_param_substitutions`
  and `substitute_type_params` re-parse the full type string at every nesting level).
- Nested generics passed to a **generic** function (e.g. `fn id<T>(x: read T) -> T` called with
  `List<…<Int>>`) make `rss check` super-linear. Re-measured on this machine: depth 100 ≈ 21 ms,
  300 ≈ 74 ms, 500 ≈ 283 ms, 800 ≈ 1.1 s, 2000 ≈ 16.8 s. Confirms the ~cubic shape. (The original
  repro used a *non-generic* `id`, which never enters the substitution recursion and stays flat —
  the trigger requires a generic callee.) Not a correctness fault.
- **Fix:** parse type strings into a tree once / memoize substitutions.
- **Status — deferred (intentional):** the asymptotic fix means replacing the string-based type
  representation in the generic-substitution path with a parse-once tree — a sizeable refactor of
  correctness-critical, heavily-relied-upon generics code. The trigger is adversarial, self-authored
  source (the compiler only processes local source, so there is no remote DoS vector), real code
  never nests generics more than a handful of levels, and the item is rated LOW / non-correctness.
  The risk of regressing generics resolution was judged disproportionate to the benefit, so the fix
  is left for a dedicated change rather than bundled with the RSS-1…RSS-10 correctness fixes.

---

## Added during tinygrad-port work (2026-06-11)

### RSS-12 — [FIXED] — Nested generic type-arguments in method calls parsed as comparisons
- **Source:** `crates/rsscript/src/syntax/parser.rs` `find_top_level_operator` (~:3031).
- `List.new<List<Int>>()`, `List.get<List<Int>>(...)`, `List.len<List<Int>>(...)` all failed with
  RS0015 "unsupported syntax". The binary-operator splitter tracked `angle_depth` asymmetrically:
  it decremented on a nested inner `>` but never incremented on the nested inner `<` (the inner
  `<` of `List<Int>` is not a generic-open per `is_generic_angle_open`, which requires the matching
  `>` to be followed by `.`/`(`), so `angle_depth` hit 0 one `>` early and the outer `>` was parsed
  as a `Greater` comparison → the call collapsed to `Expr::Unknown`.
- **Fix:** inside a generic argument list (`angle_depth > 0`), also `angle_depth += 1` on `<`. Inside
  a type-argument list `<` is unambiguously a nested generic open, never a comparison.
- **Verified:** `List<List<Int>>` build/index/len/param all parse, typecheck, and run.

### RSS-13 — [FIXED] — User sum payload-variant construction did not resolve/lower
- Previously documented in fixtures as "user payload-variant construction is not yet executable":
  declaring and `match`ing payload variants worked, but constructing one (`ArgInt(value: 5)`,
  `Shape.Circle(radius: 5)`) failed with RS0206 "does not resolve" / a backend "cannot find tuple
  variant" error.
- **Sources / fix (two sites):**
  1. `crates/rsscript/src/hir.rs` `resolve_call` (~:762): also resolve a call as
     `CallResolution::EnumVariant` when `self.sum_type_for_variant(name).is_some()` (not only the
     builtins Ok/Err/Some/None). The reg-VM already lowered `MakeVariant` via `sum_variant_fields`.
  2. `crates/rsscript/src/rust_lower/lowerer.rs` constructor lowering (~:2412): emit the qualified,
     struct-style form `Enum::Variant { field: value, ... }` (nullary: `Enum::Variant`) for user sum
     variants, matching the lowered enum (whose payload variants use named fields), instead of the
     bare/tuple fall-through.
- **Verified:** construct + `match`/discriminate + extract (via owned/`take` scrutinee) all run.
- **Known follow-up:** extracting a *Copy* payload (Int/Float) directly from a *borrowed* (`read`)
  match still needs the binding deref'd (use an owned `take`/cloned scrutinee for now).

### RSS-15 — [ADDED] — Explicit, callable `.clone()` for `derives(Clone)` types
- Previously only implicit clone existed (e.g. `read` args retained into a collection); there was no
  surface syntax to deep-copy a user value, and `derives(Copy)` on a sum is rejected. This blocked
  rebuilding immutable graph nodes (e.g. a UOp simplifier).
- **Added (kept implicit clone):**
  - `hir.rs` `resolve_receiver_call`: when no method candidate resolves and the method is `clone`,
    synthesize a builtin sig `(self: read T) -> fresh T` so `x.clone()` resolves and types as `T`.
  - `rust_lower/lowerer.rs` receiver-call lowering: for a user struct/sum receiver, emit
    `{receiver}.clone()` (Rust's derived Clone = deep copy). Gated to user types so builtins
    (JSON/Map/List) keep their own clone lowering (`json_clone`, …).
  - `rust_lower/lowerer.rs` `lower_call_arg_for_expected_type`: exclude `clone` from the
    "`read` receiver-call result is re-borrowed" rule — `.clone()` yields an owned value, so passing
    it to a by-value param must not add `&` (was emitting `&x.clone()` into an owned param).
- **Verified:** clone of struct/sum/field; clone passed to a by-value param; whole rss suite green.

### RSS-13 follow-ups — [FIXED] — payload-variant construction is now typed & checked
- (Audit found the initial RSS-13 resolved variant calls but did not type/check them.)
- **P1 type:** `hir.rs` `infer_enum_variant_type` now returns the variant's sum type for user variants
  (was only Some/Ok/Err), so `Number(value: 5)` has type `Token` and misuse (e.g. passing it to a
  `read String` param) is caught (RS0207) instead of emitting invalid Rust.
- **P1 check / no panic:** `checks/calls.rs` `check_enum_variant_form` validates user-variant
  construction against declared fields — unknown field name (RS0203), wrong arity (RS0203/RS0204) —
  so a malformed constructor (`Number(1, 2)`, `Number(bad: 5)`) is a checker error, not an
  out-of-bounds panic in the lowerer.
- **P2 field types:** `rust_lower/lowerer.rs` variant construction lowers each arg against its declared
  field type (mirroring struct ctors) and resolves fields by name bounds-safely. Note: once round-2
  value-type checking landed, `Tiny(value: 1)` with `value: Int32` is *rejected* at check (RS0207, an
  `Int` literal is not `Int32`) — same as struct constructors; the field-type lowering applies to
  values that already have the field's type (e.g. an `Int32`-typed binding), which lower without a cast.

### RSS-13 follow-ups (round 2) — [FIXED] — duplicate fields & field-value types
- (Second audit: the round-1 variant checker tracked names/counts but not duplicates or value types.)
- **Exactly-once coverage:** `check_enum_variant_form` now tracks per-field coverage — duplicate field
  (RS0205), unknown field (RS0203), too many (RS0203), and any unfilled field (RS0204). So
  `Both(left: 1, left: 2)` reports duplicate `left` + missing `right` (was: passed check, then E0062).
- **Value types:** each variant field value is type-checked against its declared field type (reusing
  `argument_type_matches` + the JSON/Map/List literal acceptances, like binding-payload checks). So
  `Number(value: "x")` for `value: Int` reports RS0207 (was: passed check, then E0308).

### RSS-15 / RSS-13 follow-ups (round 3) — [FIXED] — clone gated on `Clone`; variants are named-only
- **`.clone()` only for `Clone`-deriving types:** the synthesized clone previously resolved for *any*
  user type, so `struct Boxy derives(Eq)` passed `check` then failed Rust with E0599. Now `Hir` tracks
  `clone_types` (declared types whose derive list includes `Clone`); `resolve_receiver_call` synthesizes
  `clone` only for those (non-user receivers keep prior behavior), and rust_lower emits `.clone()` only
  when `type_derives_clone(root)`. A declared type without `Clone` now reports RS0206 at check time.
- **Variants are named-field-only:** `check_enum_variant_form` rejects any unnamed payload arg
  (RS0201), matching the v0.6 spec (variants use the same named-field construction form as structs).
  `Number(5)` is now a checker error instead of lowering to `Token::Number { value: 5i64 }`.
