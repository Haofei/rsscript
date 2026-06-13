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
- **Fast path (until fixed):** package-scale `rss check` / `rss pkg` is comfortable when the
  compiler is built in **release** (debug leaves the hot string-reparse path unoptimized). For
  repeated package-wide validation use a release `rss` (`cargo build --release --bin rss`); this is
  documented under "Performance" in `README.md`. Observed on generics-heavy ports (e.g. tinygrad-rss):
  debug package check slow, release usable.
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

### RSS-14 — [VALID · DEFERRED] — dependency-defined types not in the lowering type environment
- **Source:** `crates/rsscript/src/rust_lower/lowerer.rs` — `RustLowerer`'s `type_kinds` map (and the
  `self.program.items` lookups for sum variants / fields) is built from the **current program only**;
  the `interface_programs` (builtin + dependency `.rssi`) are not folded in.
- **Effect:** a `class`/`resource`/`struct`/`sum` declared in a *dependency* package's interface and
  then constructed/held/matched in the current source can typecheck against the contract but lower
  incorrectly, because `is_class_type`/`is_resource_type`/`field_type`/`sum_variant_fields_for_type`
  don't know its kind/fields. Not reproducible from a single file (`rss run` takes no `--interface`);
  it needs a real multi-package build.
- **Why not a blanket fix:** simply ingesting every `interface_programs` `Item::Type` into `type_kinds`
  is **wrong** — the bundled stdlib interfaces declare *runtime-backed* types (e.g. `ProcessRequest`,
  which must lower as `rsscript_runtime::ProcessRequest`) as plain structs. Classifying those as local
  user types drops the `rsscript_runtime::` qualification and changes their lowering (verified: it
  breaks `rust_lowering_maps_process_request/stream/rules_config_reload` while parity stays green, i.e.
  a silent shape regression).
- **Fix:** build a lowering type environment that ingests dependency interface types **while
  distinguishing runtime-backed stdlib types from genuine dependency types** (e.g. by interface
  origin / a runtime-binding marker), and validate with a multi-package fixture that constructs and
  matches a dependency-defined class/resource/sum. Deferred so this isn't rushed into a stdlib-lowering
  regression.
