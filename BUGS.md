# RSScript (rss) — known compiler bugs

Open issues only. Fixed items have been removed once verified across `eval`, `run`,
and `run --release` with the test suite green; see git history for their write-ups.

Run recipe used for repros:

```sh
cd rsscript
export RSSCRIPT_RUNTIME_PATH="$PWD/crates/runtime"
BIN=target/release/rss
$BIN check  file.rss          # parse + typecheck only
$BIN run    file.rss          # reg_vm interpreter
$BIN run --release file.rss   # AOT: lower to Rust -> cargo build -> run
$BIN eval   file.rss
```

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
  is left for a dedicated change rather than bundled with correctness fixes.
- **Scoping note (for the eventual parse-once change):** memoization alone does **not** help — a
  strictly-nested type has all-distinct substrings, so there are no repeated cache keys; the win has
  to come from parsing each level once. The two hot functions also do **not** parse identically:
  `substitute_type_params` takes its `Fn(...)` branch on `fn_return_type(..).is_some()` (requires a
  `->`), while `collect_type_param_substitutions` takes it on `is_fn_type(..)` (does not). So an
  arrow-less `Fn(A)` is an opaque leaf to `substitute` but a recursed fn-type to `collect`. A
  parse-once tree must preserve each function's branch rules (or the divergence must be deliberately
  unified) and be guarded by a differential/fuzz test comparing old vs new output across many type
  strings — otherwise it risks silently changing which programs typecheck. That verification, not the
  tree itself, is the bulk of the work and the reason this stays a dedicated, separately-reviewed change.
