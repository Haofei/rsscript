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
  - VM (`rss eval`): prints `Err { value: "missing JSON field \`package\`" }`,
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
