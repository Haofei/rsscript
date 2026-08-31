# 2026 Q4 experimental-decision milestone

This milestone implements the convergence priority in
[../roadmap.md](../roadmap.md) by resolving the time-bounded experimental
surfaces the project has already committed to, rather than adding scope. It is
the active one-month spine; the language specification, tests, and existing
schemas remain authoritative for behavior.

## Why this is the milestone

The project has no invented deadline problem. Two machine-checked debt calendars
already exist and are enforced by `tools/rsscript-xtask` `validate-ci`:

1. `docs/architecture/experimental-retention.toml` — 11 experimental surfaces,
   **every one with an empty `evidence_uri`/`evidence_sha256`**. After a
   surface's `decision_by`, `validate-ci` fails with *"expired without immutable
   evidence; remove it or renew through an ADR"* (`main.rs:536`).
2. `docs/architecture/module-size-allowlist.toml` — oversized modules, each with
   `target_bytes = 60000` and `decision_by = 2026-12-31`.

The nearest wall is **2026-11-30**: five JIT surfaces expire together with no
evidence attached. That wall — not a tag, not new features — is the real forcing
function for convergence, and it is the project's own contract. This milestone
executes it.

## The forcing function

Per surface, `decision_by` admits exactly three outcomes:

- **Prove** — run the surface's named `workloads`, demonstrate a *repeatable*
  end-to-end gain at or above `minimum_end_to_end_gain_percent` under the
  controlled-baseline protocol (`.github/workflows/jit-controlled-baseline.yml`;
  schema audited by `controlled_jit_baseline_schema_accepts_only_auditable_evidence`),
  and attach immutable `evidence_uri` + `evidence_sha256`. Flip `status`.
- **Cut** — execute the surface's `removal_rule`: delete the feature/pass and its
  retention entry. A cut also retires the matching `module-size-allowlist` entry
  (see convergence below), so one action clears two debts.
- **Extend** — ADR under `docs/architecture/adr`, `decision_by` moved by ≤90
  days. This is a deferral, not a resolution; use it only when a workload is
  genuinely mid-measurement.

The default is **Cut, not Extend.** Extension is the exception that needs an ADR.

## Deadline 1 — JIT surfaces (2026-11-30)

All `package = rsscript-vm`/`rsscript-jit-cranelift`, `evidence = empty`.

| id | maturity | threshold | workloads | prior signal |
| --- | --- | --- | --- | --- |
| `jit-tier0` | experimental | 10% | string-text, mailbox-ring, closure-dynamic | mixed; collections native≈interp |
| `jit-cranelift-engine` | experimental | 15% | native-scalar, native-call, native-read-heap | strongest survivor candidate |
| `jit-speculation` | research | 15% | profile-closure-pic, profile-branch-cold | unproven |
| `jit-native-recursion` | research | 15% | recursion-tree, recursion-linear | heuristic stack boundary, off by default |
| `jit-struct-scalar-replacement` | research | 15% | native-struct-sr | DeepCopy elision v1 ~no win |

Working hypothesis from prior measurements (to be confirmed, not assumed): the
three `research` surfaces are Cut candidates; `jit-tier0` and
`jit-cranelift-engine` are the two that must earn their keep with evidence or be
narrowed. Measurement decides — no surface is removed or retained on assertion.

**The cut is already half-done.** Per `benchmarks/vm-jit/README.md` "Retention
rule", the three `research` surfaces are already quarantined behind non-default
features — `jit-speculation` (closure PIC, branch side exit),
`jit-recursion-experimental`, and `jit-struct-sr-experimental` (struct SR,
isolated "after the canonical case failed to establish stable native entry and
end-to-end benefit"). The ordinary `native-jit` feature compiles none of them.
So their Cut is a low-risk deletion of already-isolated code, not a surgery on
the hot path. The two `experimental` surfaces already carry keep-rationale in
that README (baseline scalar loops, native call chains, Option/Result/Variant
SR); what they lack is the immutable evidence JSON attached to the retention
entry.

### Measurement substrate (verified in place)

- All 11 workloads resolve to real cases in `benchmarks/vm-jit/cases.tsv`
  (`xtask validate-ci` enforces this at `main.rs:653`).
- The auditable scorecard run (emits `rsscript.native_jit_scorecard.v1` per
  case):

  ```sh
  cargo test --locked --release -p rsscript-sdk --features native-jit \
    --test native_jit_scorecard -- --ignored --nocapture
  ```

- Evidence format for the retention entries: a controlled-hardware
  `benchmarks/vm-jit/baseline/canonical-<os>-<arch>.json` conforming to
  `benchmarks/vm-jit/baseline/schema.json` (must pin commit, CPU, OS,
  Rust/Cranelift version, profile, sample counts, fixture digests). Its URL and
  sha256 become `evidence_uri`/`evidence_sha256`.

## Week 1 baseline — measured 2026-08-30 (directional)

Ran the scorecard in the `dev` container (release, `native-jit`). This is an
**uncontrolled** laptop run: `retention_threshold_met` is gated on
`RSS_JIT_CONTROLLED=1` + 0 bails + speedup ≥ 1.15 (`native_jit_scorecard.rs:551`),
so it reads `False` for every case regardless of speed. The **speedup** (=
interpreter_ns / cold_e2e_native_ns) is the directional signal; formal Prove
evidence still needs a pinned controlled-hardware run.

| retention surface | named workload → scorecard case | speedup | direction |
| --- | --- | --- | --- |
| `jit-cranelift-engine` (15%) | native-scalar → scalar-loop | **30.1×** | **Prove** — decisive |
| | native-call → native-call-chain | **22.8×** | |
| | native-read-heap → list-read-loop | **10.5×** | |
| `jit-tier0` (10%) | string-text → string-processing | **1.23×** | **Prove** — clears; re-run mailbox-ring / closure-dynamic explicitly (not in this scorecard set) |
| `jit-struct-scalar-replacement` (15%) | native-struct-sr → struct-scalar-replacement | **0.27×** (3.6× *slower*, `declined`) | **Cut** — data-confirmed |
| `jit-speculation` (15%) | profile-branch-cold → profile-branch-side-exit | **0.00×** (`declined`) | **Cut** — no positive evidence |
| | profile-closure-pic | `unsupported_by_canonical_compiler` | |
| `jit-native-recursion` (15%) | recursion-tree / recursion-linear | not measured (see below) | **Cut** — fails its removal_rule by construction |

Corroborating context: the whole shared scalar-replacement family runs
net-negative on these stress kernels (option 0.41×, result 0.45×, variant 0.44×,
struct 0.27×), and `map-get-loop` enters native but loses (0.25×). Only struct-SR
is a retention surface (research-gated); the Option/Result/Variant path is the
KEPT default per `benchmarks/vm-jit/README.md` and is out of scope here — flag it
for a controlled re-check, do not cut it on one uncontrolled run.

**`jit-native-recursion` — decided without a timing run (2026-08-30).** Its
`removal_rule` is a conjunction: *"Keep disabled unless a non-heuristic stack
boundary **and** canonical benefit are both demonstrated."* The stack boundary is
heuristic by construction — `native_recursion_depth_cap` (`rsscript-jit-cranelift/src/codegen_call.rs:101`)
is `1 MiB "research budget" / native_recursion_frame_bytes_estimate`, itself a
"conservative estimate" (4096-byte fixed overhead + `regs*4` slots), hard-clamped
to a magic `NATIVE_RECURSION_DEPTH_CAP_MAX = 250`. A speed number cannot satisfy
the non-heuristic-boundary conjunct, so it cannot change the verdict; the number
is moot. Corroboration: `stable_native_feature_cannot_enable_host_stack_recursion`
(`execution_plan.rs:321`) proves the shipping `native-jit` feature already refuses
native recursion, and both kernels' own comments say the recursive call graph is
"ineligible for native." The sdk `native-jit` feature does not even forward
`jit-recursion-experimental`, so the path is unreachable from the scorecard
without new plumbing. **Cut.**

**Week 2 decision (from the data):** Cut `jit-struct-scalar-replacement` and
`jit-speculation` now (both research-gated, already isolated, data-negative);
run the recursion workloads then Cut `jit-native-recursion` unless they surprise;
schedule one controlled CI run (`RSS_JIT_CONTROLLED=1`) to mint the immutable
evidence JSON for `jit-cranelift-engine` and `jit-tier0`, then flip their status.

**DONE (2026-08-30):** All three Cuts executed. Removed the VM features
`jit-speculation` / `jit-recursion-experimental` / `jit-struct-sr-experimental`
and the Cranelift `speculation` / `recursion` features, their ~2760 lines of
gated code, their three `experimental-retention.toml` entries, and the now-retired
`module.rs` module-size allowlist entry (dropped below the hard ceiling); ratcheted
the size ceilings of the five shrunk allowlisted files. `struct-SR` gated code was
kept as `#[cfg(test)]` regression scaffolding (its gates were `any(test, feature)`).
Both feature configs compile clean; native-JIT differential/smoke, VM lib, and all
97 SDK architecture tests pass; `xtask validate-ci` is green. Remaining: the
controlled `RSS_JIT_CONTROLLED=1` evidence run for the two surviving Prove surfaces.

## Deadline 2 — experiments workspace (2026-12-31)

Six surfaces (`aot-backend`, `aot-model`, `aot-runtime`, `artifact-store`,
`reir-model`, `selfhost-parity`, `reir-review`). These are out of the immediate
month but share the same procedure; they are addressed after the JIT wall.

## Convergence with the module-partition backlog

`region_optimization.rs` and `scalar_replacement.rs` are on **both** calendars:
retention (2026-11-30) and module-size (`target_bytes = 60000`, 2026-12-31).
Sequencing matters: **decide retention first.** A Cut deletes the file and its
size-allowlist entry outright; only surfaces that survive the retention decision
are worth partitioning. Do not partition a module that is about to be removed.

## One-month plan

Docker (`compose.yaml` `dev` service) is required for workload runs; the
controlled-baseline protocol must run in-container for comparable numbers.

- **Week 1 — Baseline.** Run the scorecard command above in the `dev` container
  (release) and capture one `native_jit_scorecard.v1` record per JIT workload.
  Produce one auditable gain number per surface. No code changes; this week only
  tells us which surfaces clear their threshold. (Blocked on Docker being up; the
  substrate itself is verified in place.)
- **Week 2 — Decide.** For each surface, choose Prove / Cut / Extend from the
  Week 1 numbers. Draft evidence attachments for survivors and removal patches
  for the rest. Expect the majority to be Cut.
- **Week 3 — Cut.** Land removals: delete the feature gate, its code paths, its
  retention entry, and its size-allowlist entry. Keep both `default` and
  `native-jit` builds green at every step (`cargo check --tests` under each).
  This is where `reg_vm/native` LOC (~23k today) should visibly fall.
- **Week 4 — Close.** Attach immutable evidence for survivors, flip their
  `status`, write any extension ADRs, and confirm `xtask validate-ci` passes
  against the new reality. The milestone is done when no JIT surface is
  `pending` with empty evidence.

## Out of scope / frozen

Unchanged from `roadmap.md` "Frozen scope": no new language syntax or
qualifiers, no new JIT tiers or speculation, no C backend, no self-host
bootstrap. New JIT `feat` work is frozen for the duration of this milestone;
only fix/parity/removal changes land in `native/` and `rsscript-jit-cranelift`.
The bytecode `v1` contract and all schemas stay put.
