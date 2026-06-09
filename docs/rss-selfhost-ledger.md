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
