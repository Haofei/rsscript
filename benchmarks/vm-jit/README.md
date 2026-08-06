# VM/JIT performance baseline

The **baseline suite** for VM/JIT performance work.
Its job: pin a stable, slow-path-complete set of micro-kernels and a repeatable
way to measure them across every execution tier, so any optimization can be
proven against a committed "before" number.

It is deliberately separate from [`../micro`](../micro): that folder is the
*feature-coverage* matrix (reg-VM vs. release-Rust, one kernel per VM feature).
This folder is the *performance baseline* — organized by **slow path** (anything
the native Cranelift tier does not cover) and run across all four tiers.

## How to run

The supported fast gate is a Rust test run through the Docker dev container:

The command below is the lighter gate for day-to-day JIT work. It runs a
selected set of kernels in the current tree through the existing Rust test
harness, compares their median wall time to an existing baseline JSON, and fails
on either a large regression or unexpected native bails:

```sh
docker compose run --rm dev cargo test --release -p rsscript-engine --test runtime jit_perf_gate_against_baseline --features native-jit -- --test-threads=1 --nocapture
```

Defaults are intentionally tolerant and fast: `jit-native`, 3 iterations, 1
warmup, a 75% regression threshold, one retry for timing-only regression
candidates, and native/OSR smoke kernels including the native-call ABI cases and
profile-guided closure/branch cases. Override the defaults with environment
variables when needed: `RSS_JIT_PERF_BASELINE`, `RSS_JIT_PERF_CASES`
(comma-separated case basenames), `RSS_JIT_PERF_ITERATIONS`,
`RSS_JIT_PERF_WARMUP`, `RSS_JIT_PERF_THRESHOLD_PCT`,
`RSS_JIT_PERF_TIMING_RETRIES`, `RSS_JIT_PERF_ALLOW_BAILS=1`, or
`RSS_JIT_PERF_SKIP_TELEMETRY=1`. Telemetry failures and native bails are not
retried; those are mechanism failures, not timing noise. Benchmark subprocess
failures, including RSScript compile/runtime errors, are reported as per-case
gate failures with compact stdout/stderr context. The table includes native
telemetry for bails, retained direct-list bounds checks, memoized and ordinary
host-call sites, direct-list store/load forwarding evidence, native call edges
and depth, and compiled code bytes. This shows which intended native mechanisms
were present in the machine-code input, not only whether native code compiled.

Native compilation admission is bounded by `RSS_JIT_MAX_CODE_BYTES` (default
16 MiB of code admitted to dispatch caches) and `RSS_JIT_MAX_COMPILE_MS`
(default 2000 ms of cumulative compilation). Set either to `0` to disable new
compilation while preserving interpreter fallback. `NativeStats` exposes
`admission_admitted`, `admission_admitted_bytes`, `admission_rejected`, and
`admission_rejected_bytes` in both the text summary and JSON. Recursive groups
are admitted or rejected as a unit. A post-compile rejection is intentionally
not cached for dispatch and conservatively closes further code admission for
that VM, but its emitted bytes remain owned by the JIT module until VM drop;
these admission budgets do not claim executable-memory reclamation.

The corresponding JSON keys are `direct_list_bounds_check_sites`,
`memoized_host_call_sites`, `host_call_sites`, and
`direct_list_store_load_forwarded_moves`. The forwarding counter recognizes the
exact adjacent direct-store/`Move` shape left by the translation pass; the store
still contributes one retained bounds-check site.

The gate reads those counters from both the steady-state JIT stats block and the
benchmark harness's `cold_start` block. That keeps compile/speculation telemetry
visible even when a kernel compiles during the first run and the measured loop
only observes already-cached code.

In the table, `try` is the number of timing attempts used for that row.

### Cost-model (profitability) proof

The native-tier profitability cost model (`RSS_JIT_COST_MODEL`, default `off`) is
proven by running the same gate under `enforce`:

```sh
docker compose run --rm dev env RSS_JIT_COST_MODEL=enforce cargo test --release -p rsscript-engine --test runtime jit_perf_gate_against_baseline --features native-jit -- --test-threads=1 --nocapture
```

In `enforce` the gate adapts its expectations for the cost-model-declined kernels
(currently `profile_closure_pic`, whose native PIC is ≈ the interpreter): they are
proven by `unprofitable_declines > 0` instead of native/PIC telemetry, and — now
running on the deterministic interpreter rather than the noise-dominated native
PIC — their wall time is timing-gated again and must stay within the baseline.
Both runs (default `off` for the native-win kernels, `enforce` for the cost-model
proof) belong in the Docker perf check. `report` mode scores and logs every region
(under `RSS_JIT_REPORT`) without changing execution, for calibration.

The JSON files under `baseline/` are archived comparison points. The old
script/Python baseline runner was removed with the public benchmark CLI; any new
full-baseline runner should be a Rust harness or Make target, not a `rss`
subcommand.

For focused Docker smokes around the telemetry mechanisms themselves, use:

```sh
docker compose run --rm dev cargo test -p rsscript-engine --test runtime native_jit_precompiles_cold_scalar_call_chains --features native-jit -- --test-threads=1
docker compose run --rm dev cargo test -p rsscript-engine --test runtime report_profile_guided_pic_shows_hottest_first_order --features native-jit -- --test-threads=1
docker compose run --rm dev cargo test --release -p rsscript-engine --test runtime jit_perf_gate_against_baseline --features native-jit -- --test-threads=1 --nocapture
```

The first command exercises compiled native-to-native call edges and call-depth
telemetry. The second exercises profile-guided PIC and branch-feedback
reporting. The release test command runs the tolerant Docker perf gate over the
scalar, native-call, profile-guided PIC, profile-guided branch cold-layout,
profile-guided branch side-exit, OSR closure, and native Bytes slice/len smoke
kernels. The scheduled JIT hardening workflow runs the broader Docker command
set directly. The dedicated `JIT perf gate` GitHub workflow also runs the Docker
perf-gate command on pull requests and pushes that touch the native JIT,
VM-JIT crate, or benchmark baselines/kernels.

Some smoke kernels also carry minimum telemetry expectations: the native-call
ABI cases, including Bool, Float, flat Int/Float lists, Handle, mut-Handle, and nested
chains, must emit at least one `native_call_edges` site and a
`native_call_depth_max` of at least one. The profiled closure OSR case must emit
at least one `profile_closure_guard_sites` site. The profile-guided PIC case
must emit at least one `profile_closure_pic_sites` site and at least three
`profile_closure_pic_arms`. The profiled closure OSR and PIC smoke kernels must
also report nonzero `profile_branch_sites` and `profile_branch_samples`, proving
the branch-feedback PGO substrate stayed active. The branch cold-layout smoke
kernel must additionally report nonzero `profile_branch_cold_blocks`, proving
the compiler consumed a strong branch profile as backend layout metadata. The
branch side-exit smoke kernel must additionally report nonzero
`profile_branch_side_exits`, proving whole-function lowering converted a cold
profiled edge into a native side exit. Those new branch fixtures are
telemetry-only until the next full baseline refresh; the older smoke kernels
remain baseline-regression checked. The smoke kernels must also report nonzero
`compiled_code_bytes`. That keeps the gate from passing when the wall time is
noisy but the mechanism under test, or the code-size telemetry itself, stopped
firing. Use `RSS_JIT_PERF_SKIP_TELEMETRY=1` only for exploratory diagnostics.

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
| Realistic mixed | `selfhost_mailbox_bench` | provider-neutral end-to-end blend |

### Known not covered (intentionally)
- **Bitwise ops** — the language has no `& | ^ << >>` operators, so there is no
  kernel for them.

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
