# OSR backedge-threshold sweep (adaptive tiering)

Tuning the OSR auto-trigger threshold `OSR_BACKEDGE_THRESHOLD`
(`crates/rsscript/src/reg_vm/mod.rs`). The counting auto-trigger fires `try_osr`
once a native-INELIGIBLE function's hot-loop header has been hit T times; the
loop then runs natively for the rest of its life (fires at most once per
function — the verdict is cached in `func.osr_state`, so it is NOT re-paid per
call).

A bench/test-only `RSS_JIT_OSR_THRESHOLD` env var overrides T without
recompiling (mirrors `RSS_JIT_OSR`); unset/unparseable ⇒ the unchanged default
`OSR_BACKEDGE_THRESHOLD = 1000`. All numbers below are **median ms**, Docker
`dev` container, `--release --features native-jit`, `--mode jit-native` (the
production counting auto-trigger path — OSR NOT forced eager), single benchmark
on a quiet host, runs sequential.

Kernel: `kernels/osr_scalar_loop.rss` — a single hot Int scalar loop wrapped by
`Log.write` so the whole function is native-ineligible; loop bound = the size
arg. Each bench iteration re-evals `main` fresh, so the auto-trigger re-arms and
the threshold gates every iteration identically.

## 1. Full (trip_count × threshold) median-ms matrix + OSR-fired map

iters/warmup: trip≤2000 → 201/20; ≤10000 → 101/10; 50000 → 41/5.
`*` marks cells where OSR actually fired (`osr_entries ≥ 1`); bare cells did NOT
OSR (trip ≤ threshold ⇒ pure interpreter).

| trip \ T | 0       | 100     | 250     | 500     | 1000    | 2000    | 5000    |
|----------|---------|---------|---------|---------|---------|---------|---------|
| 500      | 0.287*  | 0.288*  | 0.282*  | 0.284*  | 0.279   | 0.278   | 0.284   |
| 1000     | 0.282*  | 0.284*  | 0.285*  | 0.287*  | 0.287*  | 0.285   | 0.284   |
| 1500     | 0.282*  | 0.284*  | 0.281*  | 0.284*  | 0.285*  | 0.282   | 0.284   |
| 2000     | 0.284*  | 0.285*  | 0.285*  | 0.286*  | 0.285*  | 0.282*  | 0.285   |
| 3000     | 0.287*  | 0.286*  | 0.293*  | 0.286*  | 0.286*  | 0.285*  | 0.287   |
| 5000     | 0.290*  | 0.292*  | 0.290*  | 0.292*  | 0.285*  | 0.288*  | 0.307*  |
| 10000    | 0.302*  | 0.298*  | 0.298*  | 0.297*  | 0.296*  | 0.303*  | 0.300*  |
| 50000    | 0.379*  | 0.363*  | 0.345*  | 0.362*  | 0.365*  | 0.359*  | 0.362*  |

**Reading:** along any ROW (fixed trip), the threshold value barely moves the
median — every cell is within ~1% noise (~0.28–0.31 ms), whether or not OSR
fired. The OSR-fired map is purely a step function of `trip > T`.

## 2. Why the threshold barely registers — OSR benefit vs the fixed compile cost

Calibration: OSR-on (T=100) vs never-OSR (T=10^8) at large trips, where the
per-iteration native savings dominate:

| trip     | never-OSR ms | OSR-on ms | speedup |
|----------|--------------|-----------|---------|
| 50000    | 5.55         | 0.38      | 14.6x   |
| 200000   | 22.35        | 0.58      | 38x     |
| 1000000  | 110.76       | 1.81      | 61x     |

So a native loop iteration is ~tens-of-x cheaper than interpreted. But the OSR
**compile** is a fixed one-time ~0.28 ms. Crossover map (OSR-on T=100 vs
never-OSR), iters 81/10:

| trip  | never-OSR ms | OSR-on ms |
|-------|--------------|-----------|
| 2000  | 0.226        | 0.301     |  ← OSR LOSES (compile not recouped)
| 3000  | 0.338        | 0.284     |  ← OSR wins
| 4000  | 0.451        | 0.287     |
| 5000  | 0.559        | 0.286     |
| 7000  | 0.787        | 0.290     |
| 10000 | 1.106        | 0.295     |
| 30000 | 3.326        | 0.324     |

Interpreted cost grows ~0.11 ms / 1000 iterations; the OSR floor is flat ~0.28
ms. **Break-even ≈ trip 2500.** Below it the compile is not recouped; above it
OSR wins, growing without bound.

For ANY genuinely hot loop (trip ≫ break-even, or a loop reused across many
calls), the compile is amortized to nothing and the threshold value is
irrelevant — confirmed directly across the realistic mix (iters as above):

| trip      | T=1000 | T=2000 | T=2500 |
|-----------|--------|--------|--------|
| 1500      | 0.289  | 0.285  | 0.283  |
| 2000      | 0.283  | 0.282  | 0.281  |
| 2500      | 0.283  | 0.283  | 0.288  |
| 3000      | 0.286  | 0.286  | 0.282  |
| 5000      | 0.291  | 0.287  | 0.288  |
| 50000     | 0.359  | 0.361  | 0.364  |
| 1000000   | 1.805  | 1.796  | 1.806  |

T ∈ {1000, 2000, 2500} are statistically indistinguishable everywhere.

## 3. Downside guard

### 3a. Reused short loop (`kernels/osr_threshold_sweep.rss`, outer=2000, inner=1500)
`main` calls a 1500-iter native-ineligible loop function 2000 times. The
auto-trigger compiles the loop ONCE (per-function cache); all later calls reuse
the compiled code (`osr_entries` ≈ outer = entries, not compiles).

| threshold | median ms | osr_entries |
|-----------|-----------|-------------|
| 100       | 6.04      | 2000        |
| 500       | 6.11      | 2000        |
| 1000      | 5.96      | 2000        |
| 1500      | 5.91      | 1999        |
| 2000      | 5.89      | 1999        |
| 5000      | 5.92      | 1997        |
| 10^8 (off)| 342.58    | 0           |

OSR-on is 57x faster than OSR-off; threshold value among {100…5000} is pure
noise. A low threshold does NOT cause repeated wasted compiles here — the
auto-trigger fires at most once per function.

### 3b. Worst-case wasted compile — loop runs exactly T+1 iterations, then exits
This is the only place the threshold has a measurable downside: a one-shot loop
that fires OSR at its very last iteration (≈zero native benefit) yet pays the
full ~0.28 ms compile. iters 151/15.

| T    | trip=T+1 | OSR-fire ms | never-OSR ms | wasted ms |
|------|----------|-------------|--------------|-----------|
| 100  | 101      | 0.284       | 0.016        | **+0.269** |
| 250  | 251      | 0.282       | 0.032        | +0.250    |
| 500  | 501      | 0.284       | 0.061        | +0.223    |
| 1000 | 1001     | 0.281       | 0.116        | +0.165    |
| 2000 | 2001     | 0.285       | 0.228        | +0.057    |
| 5000 | 5001     | 0.286       | 0.559        | **−0.273** |

The wasted-compile penalty shrinks as T rises (the interpreted loop it replaces
grows), crossing zero at ~T=2500 (= break-even). A LOW threshold widens the band
of short, one-shot loops that fire OSR but cannot recoup the compile (T=100
wastes 0.27 ms). T=1000 leaves a modest 0.16 ms worst-case waste, confined to
the narrow 1001–2500-iter single-shot band.

## 4. Decision — KEEP 1000

The data does not show any value "clearly better than 1000 across the realistic
mix," which is the task's bar for changing the constant:

1. **No hot loop is hurt by 1000.** Across medium one-shot loops (1.5k–5k), long
   loops (50k–1M), and reused short loops, T ∈ {500…5000} is within ~1% noise.
   The per-iteration native win is so large that any loop worth OSR-ing amortizes
   the fixed compile instantly; the T-iteration delay is negligible.
2. **The only measurable effect of T is the worst-case one-shot waste band**
   (a loop that runs just past T exactly once and is never reused). There 1000
   already sits safely below the ~2500 break-even and well above the noise floor:
   loops with trip < 1000 (where interpreting is < 0.12 ms) are suppressed
   entirely, avoiding their ~0.28 ms compile.
3. **Lowering T would strictly widen the waste band** (T=100 → 0.27 ms wasted on
   any 101-iter one-shot loop). Raising T to ~2500 would shave the residual
   ~0.16 ms worst-case off the 1001–2500 band — but that is a corner case (a loop
   that runs exactly that many iters, exactly once, never reused) and buys
   nothing for every actually-hot loop, all of which are noise-identical.

Net: the threshold barely matters except for extremely short, one-shot loops;
1000 is a sound, conservative choice on the safe side of the ~2500 break-even.
**Constant unchanged at 1000.** The `RSS_JIT_OSR_THRESHOLD` knob is retained as
legitimate measurement infrastructure (default behavior byte-identical).

## Reproduce

```
# matrix
docker compose run --rm dev bash benchmarks/vm-jit/run_osr_threshold_sweep.sh
# downside guard (reused short loop)
docker compose run --rm dev env RSS_JIT_OSR_THRESHOLD=1000 \
  ./target/release/rss bench --json --mode jit-native --iterations 21 --warmup 3 \
  benchmarks/vm-jit/kernels/osr_threshold_sweep.rss -- 2000 1500
```
