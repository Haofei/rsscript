# VM/JIT performance baseline

The **baseline suite** for the VM/JIT performance work in
[`docs/planning/vm-jit-perf-plan.md`](../../docs/planning/vm-jit-perf-plan.md).
Its job: pin a stable, slow-path-complete set of micro-kernels and a repeatable
way to measure them across every execution tier, so any optimization can be
proven against a committed "before" number.

It is deliberately separate from [`../micro`](../micro): that folder is the
*feature-coverage* matrix (reg-VM vs. release-Rust, one kernel per VM feature).
This folder is the *performance baseline* — organized by **slow path** (anything
the native Cranelift tier does not cover) and run across all four tiers.

## How to run

From the repo root, inside the dev container:

```sh
docker compose run --rm dev ./benchmarks/vm-jit/run-baseline.sh
# options: --iterations N  --warmup N  --timeout SECS  --out PATH
```

`--timeout` (default 180 s) caps each mode per case: a pathological kernel — e.g.
a super-linear runtime path like `task_group_spawn` at large sizes — degrades to
an `n/a` cell instead of hanging the whole suite. `--timeout 0` disables it.

Each case runs through four modes and reports mean ms + tier ratios:

| column | mode | meaning |
|---|---|---|
| `reg_vm_ms` | `vm-internal --vm reg` | register VM, no JIT |
| `jit_ms`    | `jit-internal --vm reg` | tier-0 in-process JIT |
| `native_ms` | `jit-native --vm reg` | Cranelift native tier (built with `native-jit`) |
| `rust_ms`   | `release-internal` | generated release Rust (the ceiling) |
| `reg/rust`  | — | how far the VM is from native Rust (lower = closer) |
| `jit/reg`, `nat/reg` | — | speedup of each JIT tier over the plain VM (<1 = faster) |

A row prints `n/a` for any tier that does not support the kernel (e.g. native
bails on heap-heavy code) — gaps are shown, never hidden. The run also writes a
machine-readable `baseline/baseline-<date>.json`; **commit that file** as the
reference point for the plan's Phase 0.

### Baseline JSON schema (median + spread)

Each case row keeps the legacy mean fields **unchanged** — `reg_vm_ms`, `jit_ms`,
`native_ms`, `rust_ms` are still that mode's mean ms (or `null` for an
unsupported tier) — so anything reading the old schema keeps working. In
addition, each mode now carries a nested object with the noise statistics
computed from the N per-run samples the `rss bench --json` output exposes
(`samples_ms`); the runner only **reshapes** that already-measured data, it never
re-times anything:

```json
"native": {
  "mean":   29.6, "median": 29.4,
  "min":    28.9, "max":    31.2,
  "p25":    29.1, "p75":    29.8,
  "samples": [28.9, 29.1, 29.4, 29.8, 31.2]
}
```

`median` is the §0.4 comparison statistic; `min`/`max` and the `p25`/`p75` IQR
give the per-kernel **spread band** the comparator uses to tell a real
regression from run-to-run noise. (`row_stats.py` is the small `python3` helper
the runner shells out to for this assembly.)

### Comparing two baselines (plan §0.4 — the win metric / CI gate)

`compare-baselines.py` implements the §0.4 regression rule and is the PR/CI gate:

```sh
python3 benchmarks/vm-jit/compare-baselines.py REF.json CUR.json \
    [--threshold-pct 10] [--mode reg_vm|jit|native|rust|all] \
    [--cohort CATEGORY] [--json]
```

For each kernel in both files, for each requested mode, it compares on the
**median** (falling back to the mean / `*_ms` against the old schema) and applies:

> A kernel **regresses** iff `delta% > threshold` (default 10%) **AND**
> `delta% > spread-band%`, where `delta% = (cur − ref) / ref · 100` and the
> spread band is the current run's relative spread,
> `max((max−min)/median, IQR/median) · 100`.
> A delta over the threshold but **inside** the spread band is `within-noise`,
> not a regression. Improvements (negative delta) are reported, never fail. If a
> run carries no spread fields (old schema) the band is 0, so the rule collapses
> to the bare `>threshold` check — conservative, never hiding a regression.

Verdicts: `OK`, `REGRESSION`, `improved`, `within-noise` (or `n/a` when a mode is
absent in either file). Output is grouped per cohort (category) with a per-cohort
and overall summary; `--json` emits machine-readable results.

**Exit code:** non-zero iff any `REGRESSION` in the requested mode/cohort, zero
otherwise — this is what wires it as a CI/PR gate. Use `--mode native` to check
the Phase-3.0 criterion specifically (known native-bail kernels must not get
slower under jit-native).

## Coverage matrix (slow paths)

The native tier covers only Int/Bool/Float **scalar** arithmetic + control flow +
read-only heap helpers. Everything below is a slow path. ★ = kernel new to this
suite; others are referenced from `../micro` (see `cases.tsv`).

### Native-eligible — the JIT hot path (★ new)
These are the *only* kernels the native (Cranelift) tier actually compiles and
runs (`translated:1, native_calls:1, bails:0`). They are the regression anchors
for the native tier itself — the kernels where `nat/reg` should drop well below
1.0. Every one keeps the hot loop in its own pure function so `main`'s arg
parsing doesn't taint per-function eligibility.

| Native hot path | Kernel | Exercises |
|---|---|---|
| Pure-Int scalar loop | ★ `native_scalar_loop` | the native core (arith + control) |
| Read-only heap | ★ `native_read_heap` | `list_len`/`list_get_int` host helpers |
| Cross-function call | ★ `native_call_chain` | `CallKnown` inside compiled code |
| Div/mod + guard | ★ `int_divmod_loop` | `DivInt`/`ModInt`, div-by-zero guard |
| Boolean/branch | ★ `bool_logic_loop` | `&&`/`||`, `JumpIfIntCompare` |

### VM slow paths (native bails / not eligible)

| Slow path | Kernel | Why it's slow |
|---|---|---|
| Int arith + control *(baseline)* | `pure_loop_sum` | the fast path — the yardstick |
| Struct field read/write | `struct_field_rw` | per-access field lookup |
| Nested struct build + read | ★ `nested_struct_field` | nested `MakeStruct` + `a.b.c` chains |
| Static call frame churn | `function_call_hot_loop` | `CallKnown` setup/teardown |
| Tree recursion | ★ `recursion_fib` | exponential call fan-out |
| Linear recursion (depth) | ★ `linear_recursion` | sustained frame push/pop depth |
| Float arithmetic | ★ `float_loop_sum` | Float family + Int↔Float / Float→String |
| String build/inspect | ★ `string_build_scan` | concat alloc, slice, format |
| String text-processing | ★ `string_text_processing` | `split`/`pad_left`/`starts_with` intrinsics |
| String-keyed map | `map_string_keys` | string hashing |
| User sum-type make/match | ★ `variant_match_loop` | `MakeVariant`/`MatchVariant` |
| Option match | `match_option_loop` | option destructure |
| Option/Result combinators | `option_result_chain` | closure-bearing combinators |
| List scan | `list_index_scan` | push/get/len |
| List closures | `list_closure_pipeline` | map/filter/fold closures |
| Per-iteration closure alloc | ★ `closure_alloc_loop` | `MakeClosure`+`CallClosure` each loop |
| Lazy pipeline | `pipeline_chain` | Pipeline map/filter/collect |
| List sort | ★ `list_sort` | comparison-driven `List.sort` |
| Int map | `map_insert_lookup` | hash insert/get |
| Sorted map insert | `sorted_map_insert` | ordered insert |
| Sorted map scan | `sorted_map_scan` | keys + get scan |
| Set membership | ★ `set_insert_contains` | Set insert/contains |
| Sorted-set membership | ★ `sorted_set_ops` | ordered insert + `contains` |
| Deque FIFO | `deque_queue` | push_back/pop_front |
| Bytes | ★ `bytes_scan` | Bytes construct/slice/len |
| Deep value copy | ★ `deep_copy_list` | per-iter List copy (Rc/clone traffic) |
| Stored `owned Fn` dynamic call | ★ `dynamic_closure_call` | indirect closure dispatch |
| Json | `json_parse_access` | parse + field access |
| Async call/await | `async_call_loop` | park/resume frame state |
| Structured concurrency | ★ `task_group_spawn` | `task_group` spawn + async-let join |
| Realistic mixed | `selfhost_manifest_inspector`, `selfhost_mailbox_bench` | end-to-end blends |

### Known not covered (intentionally)
- **Bitwise ops** — the language has no `& | ^ << >>` operators, so there is no
  kernel for them.
- **Tensor / ML kernels** — those are native-backed already (see the ML perf
  work) and live outside this VM-dispatch baseline.

## Baseline (2026-06-20) — headline findings

Full numbers: `baseline/baseline-20260620.json` (5 iters, 1 warmup). Ratios are
`reg/rust` (VM vs native Rust, higher = worse) and `jit/reg`, `nat/reg` (tier
speedup, **<1 = faster than the plain VM**).

1. **The native JIT codegen is excellent — it just almost never runs.** On the
   native-*eligible* kernels the native tier is **15–50× faster than the VM**
   (`nat/reg` 0.02–0.07). Its distance from native Rust (`nat/rust`), though, is
   **opcode-dependent**: pure-scalar/arith native is near-Rust (~0.9–2.1×), but
   **read-heap native is ~13×** because each heap read crosses the host-helper
   call boundary (§7.1 of the Exec-Spec):

   | kernel | reg_vm_ms | native_ms | rust_ms | nat/reg | nat/rust |
   |---|--:|--:|--:|--:|--:|
   | `native_scalar_loop` | 153.6 | 3.12 | 2.29 | **0.02** | 1.4× |
   | `native_read_heap`   | 110.5 | 7.58 | 0.57 | **0.07** | **13.3×** |
   | `native_call_chain`  | 217.8 | 5.31 | 5.65 | **0.02** | 0.9× |
   | `int_divmod_loop`    | 168.0 | 4.13 | 1.95 | **0.02** | 2.1× |
   | `bool_logic_loop`    | 321.7 | 6.26 | 2.98 | **0.02** | 2.1× |

   So the problem is **not** codegen quality — it's **eligibility/coverage**.
   The moment a function does anything outside the pure-scalar/read-heap subset
   (any heap write, string, collection op, closure, suspend), the whole function
   falls back to the interpreter. This reframes the plan: **widening native
   eligibility (Phase 3) is likely the highest-ROI lever**, not just dispatch.

2. **On the real (ineligible) kernels both JIT tiers do ~nothing** — `jit/reg`
   and `nat/reg` hover at ~1.00, and frequently **>1.0 (the tier makes it
   slower)**. In `baseline-20260620.json`: tier-0 (`jit/reg`) regresses
   `native_read_heap` **2.59×** and `nested_struct_field` 1.36; native (`nat/reg`)
   regresses `task_group_spawn` 1.58, `bytes_scan` 1.42, and
   `closure_alloc`/`option_result_chain`/`pipeline_chain` ~1.20 — the tier
   translates part of the function, bails, and eats the overhead. (The exact
   offenders shift run to run; re-read the JSON before quoting — the pattern,
   tiers *regress* ineligible code, is what's stable.) A cheap early win:
   **don't attempt the JIT on functions that will predictably bail.**

3. **`set_insert_contains` was pathological (reg/rust ≈ 1680×) — now FIXED
   (≈ 4.5×).** The `sorted_set_ops` kernel was the smoking gun: the *same*
   insert+contains workload on an **ordered** set was **2.2×** reg/rust while the
   hash `Set` was ~750× slower, isolating the cost to the hash-`Set` itself. The
   reg-VM was backing `Set` with a plain `Vec` and doing a **linear scan** on
   every insert/contains/remove — O(n²) overall. Fixed by backing `Set` with the
   same FNV `ValueMap` the `Map` type uses (value → `Unit`), making membership
   O(1); `set` now runs **4.5× reg/rust**, on par with `map_int` (4.2×). (Bonus:
   `HashMap` equality is order-insensitive, fixing a latent mismatch where two
   equal sets built in different insertion orders compared unequal under the old
   `Vec` backing.)

4. **Structured concurrency was slow *and* super-linear — now FIXED (linear).**
   `task_group_spawn` once scaled **≈ quadratically**: jit-internal measured
   0.34 / 9.9 / 725 ms at 100 / 1 000 / 10 000 rounds (identically under plain
   `eval`, so a **runtime** bug, not a JIT one). The scheduler never removed a
   finished task's slot from its task table, so its per-step `satisfy_waiters`
   scan grew O(n) and the whole loop went O(n²). Fixed by **reaping a task slot
   on join** (a handle is awaited at most once, RS0030) — it now scales linearly
   (1.8 / 17.4 / 153 ms at 1k / 10k / 100k) and the kernel is restored to size
   20 000 (~30 ms, comparable to `async_call_loop`). The runner also gained a
   per-mode `--timeout` guard so any *future* pathological case degrades to `n/a`
   instead of hanging the suite.

5. **Heap-variant & combinator paths are 340–660×** (`option_result_chain` 662×,
   `match_option_loop` 362×, `variant_match_loop` 342×, `nested_struct_field`
   374×) — make/match/allocate of Option/Result/sum/struct values dominates.

6. **`linear_recursion` is the slowest non-pathological kernel at ~294×** (4.1 s)
   — deep frame push/pop depth is expensive.

7. **Deep value copy (Rc/clone) is a ~22× tax** (`deep_copy_list` 21.7×) — real
   but well below the variant/struct make/match tier, so Phase 4 (value-rep)
   stays low priority: clone traffic is not where the big wins are.

8. **String/Map/Json/Bytes/SortedSet kernels are closest to Rust** (1.5–9×)
   because their work already lives in native runtime helpers — least to gain
   from VM-tier work.

Implications for the plan: (a) Phase 3 (widen native eligibility) is re-weighted
**up** — native is already near-Rust where it runs; (b) add a "predict-and-skip
bail" guard so the tiers stop *regressing* ineligible code (#2); (c) the two
runtime bugs the suite surfaced — the hash-`Set` (#3) and the quadratic
`task_group` (#4) — **are now fixed**, independent of the tier work; (d) Rc/clone
traffic (#7) confirms Phase 4 can stay deferred.

## Adding a kernel

1. Drop a focused `.rss` in `kernels/` using the standard `bench_size` preamble
   (copy one of the existing kernels). Keep it to **one** slow path and make the
   accumulator data-dependent so nothing folds away at compile time.
2. Add a row to `cases.tsv` (`category  path  size  slow_path_tag`).
3. Re-run `run-baseline.sh` and commit the refreshed baseline JSON.
