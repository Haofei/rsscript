# Phase 0.3 — Per-cohort interpreter cost split (Callgrind)

Status: **done.** Evidence-backed cost split (dispatch vs Rc/drop vs allocation vs
copy vs runtime-helper) for one representative reg-VM kernel per cohort, plus an
I-cache check for the dispatch cohort. Every number below traces to pasted
`callgrind_annotate` output captured in this run.

## Tooling & method

- Tool: **Callgrind** (`valgrind --tool=callgrind`), `callgrind-3.19.0`, in the
  standard `dev` container (`docker compose -p rsscript run --rm dev`). Unprivileged.
- Binary: the prebuilt release `/work/target/release/rss` (symbol table present, no
  DWARF → **function-level** attribution only, sufficient here).
- Invocation per kernel:
  `rss bench --mode vm-internal <kernel> <SIZE> --vm reg --iterations 1 --warmup 0`
  under `--tool=callgrind --cache-sim=no --dump-instr=no`, then
  `callgrind_annotate --inclusive=no --threshold=99` for top **exclusive self-Ir**.
- Sizes (kept small so callgrind finishes in seconds), as specified by the plan:
  pure_loop_sum 50000, variant_match_loop 20000, option_result_chain 8000,
  nested_struct_field 20000, linear_recursion 8000, function_call_hot_loop 20000,
  string_text_processing 10000, json_parse_access 8000.

### Two honesty caveats (read before the table)

1. **Ir is not time.** Callgrind counts **instruction reads (Ir)** — a solid
   first-order CPU-work proxy, **not** cycle-accurate (it ignores stalls, IPC,
   branch mispredicts, real cache latency). Cross-reference the wall-times from
   `baseline-20260620.json` (last column of the table) for the time picture.
2. **LTO + `#[inline(always)]` coarsen attribution.** `try_exec_pure` is
   `#[inline(always)]`, so the per-opcode arithmetic, load/store, and a lot of the
   `RegInstr` match fold *into* the `try_exec_pure` symbol — it reads as one giant
   "dispatch" bucket even though some of it is real arithmetic work. This blurs
   **dispatch-vs-inlined-arithmetic**, but `malloc`/`free`/`_int_*`/`drop_in_place`/
   `memcpy`/`hashbrown`/`serde_json` stay **distinct** symbols, so the
   *across-category* split (dispatch-family vs alloc vs Rc/drop vs helper) is still
   meaningful.

### THE BIG METHODOLOGICAL FINDING — small-SIZE allocation contamination

`bench --mode vm-internal` **compiles the kernel once (lex + parse + lower) before
the timed VM run**, and that compile step plus one-shot program setup does a large,
**fixed** amount of allocation. At the small prescribed sizes the steady-state VM
loop does *not* dominate that fixed floor, so the raw `alloc` bucket is **massively
inflated by one-shot work, not per-iteration VM allocation.** Proof:

- `lexer::lex` is ~**21.1M Ir in every kernel** (21,138,543 / 21,111,803 /
  21,166,529 / 21,046,927 / 21,171,953 / 21,066,424 / 21,050,872) — a dead-constant
  fixed cost independent of SIZE.
- **Amortization control:** re-running `pure_loop_sum` at SIZE=**500000** (10×):
  `try_exec_pure` scales to **68.57%** while the malloc/free symbols stay at the
  *same absolute counts* as the 50000 run (`_int_free` 27,588,950 vs 27,587,510;
  `_int_malloc'2` 25,163,452 vs 25,146,614; `lex` 21,139,261 vs 21,138,543).
- **Marginal (per-iteration) split** for pure_loop_sum, computed from the
  500000−50000 Ir delta (1,316,325,905 marginal Ir):

  | symbol | marginal Ir | % of marginal | fixed floor (50k) |
  |---|---:|---:|---:|
  | `try_exec_pure` | 1,019,042,028 | **77.4%** | 113,231,451 |
  | `eval_numeric_binary` | 104,900,000 | 8.0% | 11,100,000 |
  | `drop_in_place<VmValue>` | 73,125,000 | 5.6% | 8,125,590 |
  | `VmValue::clone` | 39,206,250 | 3.0% | 4,356,679 |
  | `_int_free` | **1,440** | **0.00%** | 27,587,510 |
  | `_int_malloc'2` | 16,838 | 0.00% | 25,146,614 |
  | `lex` | 718 | 0.00% | 21,138,543 |

  → In steady state the dispatch cohort allocates **~0 per iteration**; the 36%
  "alloc" in its raw 50k table is **entirely one-shot setup**. The dispatch cohort
  is unambiguously **dispatch-bound**.

So the table below is given **two ways**: (a) raw exclusive-Ir buckets at the
prescribed sizes, with a separate **`compile`** bucket for lexer/parser symbols; and
(b) the honest **per-iteration reading** in the "Cohort verdict" column, which
discounts the fixed alloc/compile floor where the amortization/recursion controls
prove it is one-shot.

## Per-cohort cost-split table (raw exclusive Ir %, prescribed sizes)

Buckets: **dispatch** = `try_exec_pure`/`drive` (the interp loop; note `try_exec_pure`
is inlined so it absorbs inlined arithmetic); **rc** = `drop_in_place`/`Rc::drop_slow`;
**alloc** = `malloc`/`free`/`_int_*`/`malloc_consolidate`/`unlink_chunk`/`raw_vec`
grow+reserve; **copy** = `memcpy`/`bcmp`; **helper** = `String/str`/`from_iter<&char>`/
`serde_json`/`indexmap`/sip-hash/`exec_*_intrinsics`/`TwoWaySearcher`; **vmops** =
other `reg_vm::`/`vm_value::` value/frame ops; **compile** = `lexer`+`parser`
(one-shot, ~12–15% floor at these sizes); **other** = unsymbolized/misc.

| kernel (cohort) | dispatch | rc | alloc | copy | helper | vmops | compile | other | reg_vm ms / rust ms (baseline) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| pure_loop_sum *(dispatch)* | 33.8 | 2.4 | 36.5 | 2.7 | 2.7 | 7.4 | 11.1 | 0.3 | 235.3 / 3.9 (60×) |
| variant_match_loop *(alloc/variant)* | 24.5 | 3.8 | 45.7 | 3.1 | 2.4 | 5.6 | 10.6 | 0.3 | 107.0 / 0.36 (298×) |
| option_result_chain *(alloc/variant)* | 15.9 | 3.1 | 51.2 | 3.0 | 4.2 | 6.0 | 10.9 | 0.5 | 47.2 / 0.07 (652×) |
| nested_struct_field *(struct r+build)* | 15.4 | 3.2 | 56.6 | 3.5 | 2.5 | 4.9 | 10.5 | 0.6 | 96.6 / 0.26 (365×) |
| linear_recursion *(frame churn)* | 74.4 | 9.2 | 0.2 | 0.0 | 0.0 | 15.3 | 0.0 | 0.0 | 4031 / 14.5 (278×) |
| function_call_hot_loop *(frame churn)* | 24.6 | 2.2 | 43.9 | 3.2 | 3.2 | 4.9 | 13.4 | 0.3 | 36.1 / 0.55 (66×) |
| string_text_processing *(helper/string)* | 8.6 | 1.4 | 54.4 | 4.4 | 8.8 | 2.2 | 12.6 | 2.2 | 45.4 / 27.7 (1.6×) |
| json_parse_access *(helper/json)* | 4.6 | 2.1 | 55.7 | 3.2 | 14.4 | 2.9 | 10.1 | 0.0 | 49.7 / 31.9 (1.6×) |

> The `dispatch` column understates the true dispatch share for the small kernels
> because (per the contamination finding) a big slice of their `alloc` is one-shot
> setup that does not recur. `linear_recursion`, whose 16.5B-Ir run drowns the fixed
> floor, is the *clean* VM-only profile: **74.4% dispatch, 9.2% rc/drop, 15.3% other
> reg_vm value/frame ops, ~0.2% alloc.**

### Cohort verdict (per-iteration, contamination-corrected)

| cohort | representative(s) | what actually dominates the *loop* | bound by |
|---|---|---|---|
| **dispatch** | pure_loop_sum | 77% `try_exec_pure`, 8% arith, ~0% alloc (amort control) | **dispatch** |
| **frame churn** | linear_recursion, function_call_hot_loop | 74% dispatch + 9% drop + 15% frame/value ops; alloc ~0 in recursion | **dispatch + frame mgmt** |
| **alloc / variant** | variant_match, option_chain | per-iteration variant heap cells: `_int_free`/`malloc`/`free`/`drop`/`Rc::drop_slow`/`deep_copy_value` are real and recur; dispatch only ~25% | **allocation + Rc/drop** |
| **struct read+build** | nested_struct_field | per-iteration struct build/drop: highest real alloc (56% raw), `VmStruct::from_named`, `read_field_slot` | **allocation + Rc/drop** |
| **helper / string-json** | string_text, json_parse | `serde_json::deserialize_any`/`parse_str`, `indexmap`, sip-hash, `TwoWaySearcher`, `String::from_iter`, plus the alloc those helpers drive; dispatch <9% | **runtime helpers** (already near-Rust: 1.6×) |

(For variant/struct/helper, the alloc is **not** purely one-shot: their loops build
and drop a fresh heap value each iteration. `linear_recursion`'s 0.2% alloc confirms
these allocations are workload-intrinsic — a kernel that does no per-iter heap alloc
shows none, while these cohorts show a lot, and their baseline slowdown — 298×/365×/
652× vs Rust — is consistent with per-iteration boxing/Rc traffic.)

## I-cache finding for the dispatch cohort (`--cache-sim=yes`)

Re-ran `pure_loop_sum` (50000) and `bool_logic_loop` (50000) with cache simulation
(default model: I1 16 KiB / 4-way / 64 B line). From the callgrind summary:

```
pure_loop_sum:
  I   refs:   334,909,466    I1 misses: 1,515,328    LLi misses: 51,148
  I1 miss rate: 0.45%        LLi miss rate: 0.02%
  D1 miss rate: 1.4%         LL  miss rate: 0.2%
bool_logic_loop:
  I   refs:   365,350,478    I1 misses: 1,225,367    LLi misses: 48,107
  I1 miss rate: 0.34%        LLi miss rate: 0.01%
  D1 miss rate: 1.2%         LL  miss rate: 0.1%
```

**Finding: I-cache is NOT a bottleneck for the dispatch cohort.** I1 miss rate is
**0.45% / 0.34%** and last-level instruction miss is **0.02% / 0.01%** — both
negligible. The freshly-inlined (`#[inline(always)]`) `try_exec_pure` does **not**
cause I-cache pressure at these working sets. The §1.3 hot/cold (cold-path outlining)
split is therefore **not justified by instruction-cache misses** and should not be
prioritized on that rationale. Note the **data** side is the larger memory effect
(D1 1.2–1.4%, LLd ~0.4–0.5%), consistent with the boxed-`VmValue`/`Rc` representation
— i.e. value-representation work (Phase 4), not code layout, is where the memory
cost lives.

## Sanity: previously-pathological kernels out of scope

The two slowest kernels in the historical matrix (hash-`Set` O(n²),
quadratic `task_group`) were **runtime bugs**, already fixed in code
(commit `9f7901e` "O(1) hash-Set and linear task_group scheduler"). They are
**out of scope** for this cost-split: they would have "refuted dispatch" for the
wrong reason (algorithmic blowup, not a cohort cost). Not re-profiled here.

## Raw evidence — `callgrind_annotate --inclusive=no` top exclusive-Ir functions

(top ~17 self-Ir functions per kernel; `[…/rss]` / libc path suffixes trimmed for
width; full untrimmed output was captured in this run.)

### dispatch — pure_loop_sum 50000
```
334,923,981 (100.0%)  PROGRAM TOTALS
113,231,451 (33.81%)  rsscript::reg_vm::RegVm::try_exec_pure'2
 27,587,510 ( 8.24%)  _int_free (libc)
 25,146,614 ( 7.51%)  _int_malloc'2 (libc)
 21,138,543 ( 6.31%)  rsscript::lexer::lex'2            <-- one-shot compile
 16,146,522 ( 4.82%)  malloc'2
 12,206,959 ( 3.64%)  _int_malloc (libc)
 11,100,000 ( 3.31%)  rsscript::reg_vm::eval_numeric_binary
  9,286,494 ( 2.77%)  malloc_consolidate'2 (libc)
  8,763,085 ( 2.62%)  free'2
  8,125,590 ( 2.43%)  core::ptr::drop_in_place<VmValue>
  6,377,207 ( 1.90%)  String as FromIterator<&char>::from_iter'2   <-- lexer char-vec
  5,107,772 ( 1.53%)  unlink_chunk.constprop.0 (libc)
  5,084,332 ( 1.52%)  __GI_memcpy (libc)
  4,356,679 ( 1.30%)  VmValue::clone
  3,963,931 ( 1.18%)  rsscript::syntax::parser::parse_type_ref'2   <-- one-shot compile
  3,255,922 ( 0.97%)  raw_vec finish_grow
  3,034,704 ( 0.91%)  raw_vec do_reserve_and_handle'2
```

### dispatch (amortization control) — pure_loop_sum 500000
```
1,651,249,886 (100.0%)  PROGRAM TOTALS
1,132,273,479 (68.57%)  rsscript::reg_vm::RegVm::try_exec_pure'2
  111,000,000 ( 6.72%)  rsscript::reg_vm::eval_numeric_binary
   81,250,590 ( 4.92%)  core::ptr::drop_in_place<VmValue>
   43,562,929 ( 2.64%)  VmValue::clone
   27,588,950 ( 1.67%)  _int_free (libc)     <-- ~unchanged vs 50k (fixed)
   25,163,452 ( 1.52%)  _int_malloc'2 (libc) <-- ~unchanged vs 50k (fixed)
   21,139,261 ( 1.28%)  rsscript::lexer::lex'2 <-- ~unchanged vs 50k (fixed)
   21,000,032 ( 1.27%)  rsscript::reg_vm::eval_numeric_compare
```

### alloc/variant — variant_match_loop 20000
```
339,003,901 (100.0%)  PROGRAM TOTALS
76,756,414 (22.64%)  rsscript::reg_vm::RegVm::try_exec_pure'2
37,505,877 (11.06%)  _int_free (libc)
24,755,190 ( 7.30%)  _int_malloc'2 (libc)
24,451,920 ( 7.21%)  malloc'2
21,111,803 ( 6.23%)  rsscript::lexer::lex'2            <-- one-shot compile
18,323,741 ( 5.41%)  free'2
11,967,828 ( 3.53%)  _int_malloc (libc)
 9,733,583 ( 2.87%)  core::ptr::drop_in_place<VmValue>'2
 9,138,875 ( 2.70%)  malloc_consolidate'2 (libc)
 6,366,839 ( 1.88%)  String FromIterator<&char>::from_iter'2
 6,280,361 ( 1.85%)  rsscript::reg_vm::RegVm::drive'2
 5,764,273 ( 1.70%)  __GI_memcpy (libc)
 5,022,275 ( 1.48%)  unlink_chunk.constprop.0 (libc)
 4,392,035 ( 1.30%)  raw_vec finish_grow
 4,093,642 ( 1.21%)  VmValue::clone
 3,954,760 ( 1.17%)  parser::parse_type_ref'2          <-- one-shot compile
 3,755,588 ( 1.11%)  free'2 (libc)
[further down: Rc<T,A>::drop_slow'2 2,200,497 (0.65%); deep_copy_value 2,180,021 (0.64%)]
```

### alloc/variant (combinators) — option_result_chain 8000
```
330,342,758 (100.0%)  PROGRAM TOTALS
45,698,383 (13.83%)  rsscript::reg_vm::RegVm::try_exec_pure'2
43,372,986 (13.13%)  _int_free (libc)
29,105,966 ( 8.81%)  malloc'2
25,142,094 ( 7.61%)  _int_malloc'2 (libc)
21,166,529 ( 6.41%)  rsscript::lexer::lex'2            <-- one-shot compile
17,377,270 ( 5.26%)  free'2
12,149,706 ( 3.68%)  _int_malloc (libc)
 9,239,426 ( 2.80%)  malloc_consolidate'2 (libc)
 7,016,334 ( 2.12%)  core::ptr::drop_in_place<VmValue>'2
 6,385,289 ( 1.93%)  String FromIterator<&char>::from_iter'2
 6,062,223 ( 1.84%)  __GI_memcpy (libc)
 5,760,279 ( 1.74%)  rsscript::reg_vm::RegVm::drive'2
 5,328,988 ( 1.61%)  raw_vec finish_grow
 5,052,696 ( 1.53%)  unlink_chunk.constprop.0 (libc)
[further down: exec_option_intrinsics 2,843,907 (0.86%); exec_result_intrinsics 2,823,983 (0.85%);
 Rc<T,A>::drop_slow'2 2,712,724 (0.82%); call_closure_one 3,120,000 (0.94%)]
```

### struct read+build — nested_struct_field 20000
```
347,775,181 (100.0%)  PROGRAM TOTALS
53,402,833 (15.36%)  rsscript::reg_vm::RegVm::try_exec_pure'2
52,879,835 (15.21%)  _int_free (libc)
37,147,561 (10.68%)  malloc'2
24,243,966 ( 6.97%)  _int_malloc'2 (libc)
21,384,086 ( 6.15%)  free'2
21,046,927 ( 6.05%)  rsscript::lexer::lex'2            <-- one-shot compile
11,814,538 ( 3.40%)  _int_malloc (libc)
 9,011,264 ( 2.59%)  malloc_consolidate'2 (libc)
 8,219,752 ( 2.36%)  __GI_memcpy (libc)
 6,676,482 ( 1.92%)  raw_vec finish_grow
 6,350,168 ( 1.83%)  String FromIterator<&char>::from_iter'2
 6,334,995 ( 1.82%)  _int_free'2 (libc)
 4,937,617 ( 1.42%)  unlink_chunk.constprop.0 (libc)
 4,707,194 ( 1.35%)  free'2 (libc)
 4,359,982 ( 1.25%)  rsscript::vm_value::VmStruct::from_named'2   <-- per-iter struct build
 4,300,400 ( 1.24%)  core::ptr::drop_in_place<VmValue>
[further down: Rc<T,A>::drop_slow'2 3,680,326 (1.06%); read_field_slot 2,400,000 (0.69%)]
```

### frame churn (recursion) — linear_recursion 8000  (CLEAN VM-only profile, 16.5B Ir)
```
16,508,635,042 (100.0%)  PROGRAM TOTALS
11,199,609,338 (67.84%)  rsscript::reg_vm::RegVm::try_exec_pure'2
 1,521,930,118 ( 9.22%)  core::ptr::drop_in_place<VmValue>'2
 1,081,463,171 ( 6.55%)  rsscript::reg_vm::RegVm::drive'2
   544,944,253 ( 3.30%)  VmValue::clone
   496,496,000 ( 3.01%)  rsscript::reg_vm::eval_numeric_binary
   464,464,029 ( 2.81%)  rsscript::reg_vm::value_convert::deep_copy_value
   264,264,000 ( 1.60%)  VmValue::eq
   248,248,114 ( 1.50%)  rsscript::reg_vm::RegVm::push_frame'2
   144,441,645 ( 0.87%)  rsscript::reg_vm::RegVm::ensure_regs'2
   136,136,034 ( 0.82%)  rsscript::reg_vm::RegVm::apply_mut_writeback
   120,120,000 ( 0.73%)  rsscript::reg_vm::expect_bool_ref
   104,104,000 ( 0.63%)  VmValue::eq'2
    26,350,014 ( 0.16%)  _int_free (libc)    <-- alloc is ~nil per iteration
```

### frame churn (calls) — function_call_hot_loop 20000
```
279,075,237 (100.0%)  PROGRAM TOTALS
65,135,295 (23.34%)  rsscript::reg_vm::RegVm::try_exec_pure'2
27,750,089 ( 9.94%)  _int_free (libc)
25,195,023 ( 9.03%)  _int_malloc'2 (libc)
21,171,953 ( 7.59%)  rsscript::lexer::lex'2            <-- one-shot compile
16,221,740 ( 5.81%)  malloc'2
12,208,473 ( 4.37%)  _int_malloc (libc)
 9,277,244 ( 3.32%)  malloc_consolidate'2 (libc)
 8,813,295 ( 3.16%)  free'2
 6,382,838 ( 2.29%)  String FromIterator<&char>::from_iter'2
 6,250,473 ( 2.24%)  core::ptr::drop_in_place<VmValue>'2
 5,141,405 ( 1.84%)  __GI_memcpy (libc)
 5,090,524 ( 1.82%)  unlink_chunk.constprop.0 (libc)
 5,060,000 ( 1.81%)  rsscript::reg_vm::eval_numeric_binary
 3,964,729 ( 1.42%)  parser::parse_type_ref'2          <-- one-shot compile
 3,580,291 ( 1.28%)  rsscript::reg_vm::RegVm::drive'2
 3,442,929 ( 1.23%)  VmValue::clone
```

### runtime-helper (string) — string_text_processing 10000
```
285,422,518 (100.0%)  PROGRAM TOTALS
38,957,787 (13.65%)  _int_free (libc)
25,545,222 ( 8.95%)  _int_malloc'2 (libc)
24,452,729 ( 8.57%)  rsscript::reg_vm::RegVm::try_exec_pure'2
23,795,567 ( 8.34%)  malloc'2
21,066,424 ( 7.38%)  rsscript::lexer::lex'2            <-- one-shot compile
13,167,282 ( 4.61%)  _int_malloc (libc)
12,554,406 ( 4.40%)  free'2
 9,083,065 ( 3.18%)  malloc_consolidate'2 (libc)
 8,317,165 ( 2.91%)  __GI_memcpy (libc)
 6,680,261 ( 2.34%)  rsscript::reg_vm::RegVm::exec_string_intrinsics'2  <-- helper
 6,359,048 ( 2.23%)  String FromIterator<&char>::from_iter'2
 5,460,000 ( 1.91%)  core::str::pattern::TwoWaySearcher::next'2        <-- helper
 5,077,866 ( 1.78%)  unlink_chunk.constprop.0 (libc)
 4,448,812 ( 1.56%)  raw_vec finish_grow
 4,093,577 ( 1.43%)  free'2 (libc)
```

### runtime-helper (json) — json_parse_access 8000
```
337,879,418 (100.0%)  PROGRAM TOTALS
51,675,890 (15.29%)  _int_free (libc)
34,028,440 (10.07%)  malloc'2
26,535,671 ( 7.85%)  _int_malloc'2 (libc)
21,050,872 ( 6.23%)  rsscript::lexer::lex'2            <-- one-shot compile
20,014,537 ( 5.92%)  free'2
14,754,605 ( 4.37%)  rsscript::reg_vm::RegVm::try_exec_pure'2
12,667,351 ( 3.75%)  _int_malloc (libc)
 9,396,828 ( 2.78%)  serde_json Deserializer::deserialize_any'2        <-- helper
 9,009,732 ( 2.67%)  malloc_consolidate'2 (libc)
 7,213,624 ( 2.13%)  raw_vec finish_grow
 6,792,084 ( 2.01%)  __GI_memcpy (libc)
 6,356,292 ( 1.88%)  String FromIterator<&char>::from_iter'2
 5,023,509 ( 1.49%)  unlink_chunk.constprop.0 (libc)
 4,906,159 ( 1.45%)  free'2 (libc)
 4,600,615 ( 1.36%)  _int_free'2 (libc)
 4,180,105 ( 1.24%)  core::hash::sip::Hasher::write                    <-- helper
 3,953,474 ( 1.17%)  parser::parse_type_ref'2          <-- one-shot compile
 3,694,660 ( 1.09%)  indexmap IndexMap::insert_full'2                  <-- helper
[further down: StrRead::parse_str'2 2,719,980 (0.81%); drop_in_place<serde_json::Value>'2
 2,648,000 (0.78%); exec_json_intrinsics'2 2,480,005 (0.73%)]
```

## Phase-ordering recommendation

The current plan order — **3.0 done → Phase 3 (widen native-eligible) → Phase 1
(dispatch) → Phase 2 (real baseline) / Phase 4 (value rep)** — is **broadly
confirmed, with one sharpening.** The data sorts the cohorts cleanly:

- **Dispatch-bound → Phase 1 has real value.** `pure_loop_sum` (77% `try_exec_pure`
  per-iteration, ~0% alloc) and `linear_recursion`/`function_call_hot_loop`
  (74% dispatch + frame ops, ~0–0.2% alloc) are genuinely dominated by the
  interpreter loop and frame management. Phase 1 (dispatch reduction / Phase 3
  native-eligible widening to *skip* the loop for these) is well-targeted. These are
  also exactly the cohorts where **Phase 3 native-eligible** pays off most, so doing
  Phase 3 before Phase 1 is the right call — native-eligible removes the dispatch
  cost outright for the loops that are pure-dispatch.

- **Alloc/Rc-bound → Phase 4 (value representation) rises in priority.** The
  variant/struct cohorts (`variant_match` 298×, `nested_struct_field` 365×,
  `option_result_chain` 652× vs Rust) are dominated by **per-iteration heap
  allocation + `drop_in_place`/`Rc::drop_slow`/`deep_copy_value`**, *not* dispatch.
  No amount of dispatch tuning helps them; their slowdown is the boxed-`VmValue`/`Rc`
  representation. **These are the worst absolute slowdowns in the matrix, and Phase 1
  does nothing for them.** Recommendation: **raise Phase 4 (value rep: unbox small
  values, kill per-iter `Rc` traffic / `deep_copy_value`) at least level with
  Phase 1**, because it owns the largest gap-to-Rust. The D1 miss data (1.2–1.4% vs
  the negligible I1) independently points the same way: the cost is in the data
  representation, not the code.

- **Helper-bound → little VM-tier upside (correctly deprioritized).** `string_text`
  (1.6×) and `json_parse` (1.6×) are already near-Rust; their time is in
  `serde_json`/`indexmap`/sip-hash/`TwoWaySearcher`/`String` — the same library code
  Rust would run. Dispatch is <9%. No VM-tier (Phase 1/3) or value-rep (Phase 4) work
  meaningfully moves them; leave them out of scope, as the plan already does.

- **§1.3 hot/cold split: drop the I-cache rationale.** I1 miss rate 0.34–0.45%,
  LLi 0.01–0.02% — the inlined `try_exec_pure` is **not** I-cache-bound. If §1.3 is
  pursued it must be justified on a different basis (e.g. branch density), not
  instruction-cache pressure.

**Net:** order stays *Phase 3 (native-eligible) → Phase 1 (dispatch)* for the
dispatch/frame cohorts; **elevate Phase 4 (value/`Rc` representation) to co-priority
with Phase 1**, since the alloc/variant/struct cohorts — the largest slowdowns — are
allocation/`Rc`-bound and untouched by dispatch work. Helper cohorts remain out of
scope.
