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

## Baseline (2026-06-20) — headline findings

Full numbers: `baseline/baseline-20260620.json` (5 iters, 1 warmup). Ratios are
`reg/rust` (VM vs native Rust, higher = worse) and `jit/reg`, `nat/reg` (tier
speedup, **<1 = faster than the plain VM**).

1. **The native JIT codegen is excellent — it just almost never runs.** On the
   native-*eligible* kernels the native tier is **15–50× faster than the VM**
   and within ~1.4–2× of native Rust:

   | kernel | reg_vm_ms | native_ms | rust_ms | nat/reg |
   |---|--:|--:|--:|--:|
   | `native_scalar_loop` | 153.5 | 3.18 | 2.31 | **0.02** |
   | `native_read_heap`   | 115.6 | 6.58 | 0.58 | **0.06** |
   | `native_call_chain`  | 202.0 | 5.41 | 5.58 | **0.03** |
   | `int_divmod_loop`    | 168.9 | 4.23 | 1.90 | **0.03** |
   | `bool_logic_loop`    | 326.7 | 6.60 | 2.99 | **0.02** |

   So the problem is **not** codegen quality — it's **eligibility/coverage**.
   The moment a function does anything outside the pure-scalar/read-heap subset
   (any heap write, string, collection op, closure, suspend), the whole function
   falls back to the interpreter. This reframes the plan: **widening native
   eligibility (Phase 3) is likely the highest-ROI lever**, not just dispatch.

2. **On the real (ineligible) kernels both JIT tiers do ~nothing** — `jit/reg`
   and `nat/reg` hover at ~1.00, and frequently **>1.0 (native makes it
   slower)**: `list_sort` 1.31, `map_int` 1.19, `closure_alloc` 1.13,
   `recursion_fib` 1.09 — the tier translates part of the function, bails, and
   eats the overhead. Tier-0 similarly regresses `json` (1.48), `dynamic_closure`
   (1.66), `float` (1.27). A cheap early win: **don't attempt native on
   functions that will predictably bail.**

3. **`set_insert_contains` is pathological: reg/rust ≈ 1680×** (1774 ms vs
   1.06 ms), vs the comparable `map_int` at 4.5×. Almost certainly a Set
   implementation bug, not dispatch cost — flagged for its own investigation,
   separate from the tier work.

4. **Heap-variant & combinator paths are 290–655×** (`option_result_chain` 655×,
   `match_option_loop` 331×, `variant_match_loop` 288×, `nested_struct_field`
   380×) — make/match/allocate of Option/Result/sum/struct values dominates.

5. **`linear_recursion` is the slowest non-Set kernel at 314×** (4.6 s) — deep
   frame push/pop depth is expensive; the one kernel where tier-0 actually helps
   a little (0.88).

6. **String/Map/Json/Bytes kernels are closest to Rust** (1.5–9×) because their
   work already lives in native runtime helpers — least to gain from VM-tier work.

Implications for the plan: (a) Phase 3 (widen native eligibility) is re-weighted
**up** — native is already near-Rust where it runs; (b) add a "predict-and-skip
bail" guard so the tiers stop *regressing* ineligible code; (c) the Set anomaly
(#3) is a separate, likely cheap, high-impact bug fix.

## Adding a kernel

1. Drop a focused `.rss` in `kernels/` using the standard `bench_size` preamble
   (copy one of the existing kernels). Keep it to **one** slow path and make the
   accumulator data-dependent so nothing folds away at compile time.
2. Add a row to `cases.tsv` (`category  path  size  slow_path_tag`).
3. Re-run `run-baseline.sh` and commit the refreshed baseline JSON.
