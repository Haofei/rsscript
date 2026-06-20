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
# options: --iterations N  --warmup N  --out PATH
```

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

## Coverage matrix (slow paths)

The native tier covers only Int/Bool/Float **scalar** arithmetic + control flow +
read-only heap helpers. Everything below is a slow path. ★ = kernel new to this
suite; others are referenced from `../micro` (see `cases.tsv`).

| Slow path | Kernel | Why it's slow |
|---|---|---|
| Int arith + control *(baseline)* | `pure_loop_sum` | the fast path — the yardstick |
| Struct field read/write | `struct_field_rw` | per-access field lookup |
| Static call frame churn | `function_call_hot_loop` | `CallKnown` setup/teardown |
| Deep recursion | ★ `recursion_fib` | frame/register-window alloc per call |
| Float arithmetic | ★ `float_loop_sum` | Float family + Int↔Float / Float→String |
| String build/inspect | ★ `string_build_scan` | concat alloc, slice, format |
| String-keyed map | `map_string_keys` | string hashing |
| User sum-type make/match | ★ `variant_match_loop` | `MakeVariant`/`MatchVariant` |
| Option match | `match_option_loop` | option destructure |
| Option/Result combinators | `option_result_chain` | closure-bearing combinators |
| List scan | `list_index_scan` | push/get/len |
| List closures | `list_closure_pipeline` | map/filter/fold closures |
| Lazy pipeline | `pipeline_chain` | Pipeline map/filter/collect |
| List sort | ★ `list_sort` | comparison-driven `List.sort` |
| Int map | `map_insert_lookup` | hash insert/get |
| Sorted map insert | `sorted_map_insert` | ordered insert |
| Sorted map scan | `sorted_map_scan` | keys + get scan |
| Set membership | ★ `set_insert_contains` | Set insert/contains |
| Deque FIFO | `deque_queue` | push_back/pop_front |
| Bytes | ★ `bytes_scan` | Bytes construct/slice/len |
| Stored `owned Fn` dynamic call | ★ `dynamic_closure_call` | indirect closure dispatch |
| Json | `json_parse_access` | parse + field access |
| Async call/await | `async_call_loop` | park/resume frame state |
| Realistic mixed | `selfhost_manifest_inspector`, `selfhost_mailbox_bench` | end-to-end blends |

### Known not covered (intentionally)
- **Bitwise ops** — the language has no `& | ^ << >>` operators, so there is no
  kernel for them.
- **Tensor / ML kernels** — those are native-backed already (see the ML perf
  work) and live outside this VM-dispatch baseline.

## First baseline (2026-06-20) — headline findings

Full numbers: `baseline/baseline-20260620.json` (5 iters, 1 warmup). Ratios are
`reg/rust` (VM vs native Rust, higher = worse) and `jit/reg`, `nat/reg` (tier
speedup, **<1 = faster than the plain VM**).

1. **The JIT tiers do almost nothing.** `jit/reg` and `nat/reg` sit at ~1.00
   across nearly every kernel — the tier-0 "JIT" and even the native tier give
   no measurable speedup on the common path. This is the central problem the
   perf plan is built around, now quantified. The **only** real win is
   `deque_queue` (jit 0.62, native 0.44); native is occasionally *slower*
   (`selfhost_mailbox` 1.27, `closure_dynamic` 1.09) — it translates, bails, and
   eats the overhead.
2. **`set_insert_contains` is pathological: reg/rust ≈ 1628×** (1779 ms vs
   1.09 ms). Set membership is wildly more expensive than the comparable Map
   kernels (`map_int` 3.1×). This looks like a real Set implementation bug, not
   just dispatch cost — flagged for its own investigation, separate from the
   tier work.
3. **Heap-variant & combinator paths are 300–610×**
   (`option_result_chain` 610×, `match_option_loop` 344×, `variant_match_loop`
   299×) — make/match/allocate of Option/Result/sum values dominates.
4. **Struct field churn 255×** and **recursion 68×** confirm the
   field-hashing and frame-alloc costs the plan calls out.
5. **String/Map/Json kernels are the closest to Rust** (1.3–9×) because their
   work is already in native runtime helpers — least to gain from VM-tier work.

Implication for the plan: Phase 1 (dispatch) addresses the broad ~1.0 tier
ratios; the Set anomaly (#2) is a separate, likely higher-ROI bug fix.

## Adding a kernel

1. Drop a focused `.rss` in `kernels/` using the standard `bench_size` preamble
   (copy one of the existing kernels). Keep it to **one** slow path and make the
   accumulator data-dependent so nothing folds away at compile time.
2. Add a row to `cases.tsv` (`category  path  size  slow_path_tag`).
3. Re-run `run-baseline.sh` and commit the refreshed baseline JSON.
