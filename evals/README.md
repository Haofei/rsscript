# RSScript evaluation corpus

This is a small, offline-first corpus for repair and review evaluations. It deliberately contains no model runner, online-model integration, Python/TypeScript baseline, or score threshold.

Each task in `tasks/*.toml` points at a deterministic candidate source, the minimal local interfaces needed to check it, and expected outcome data in `expected/`. Candidates are inputs, not golden implementations: an intentional failure records the diagnostic that must disappear; a passing candidate records a safety or shape invariant that must survive a transformation.

Run an individual candidate from the repository root with its listed interfaces:

```bash
cargo run -p rsscript-cli --bin rss -- check --json \
  evals/fixtures/named-args/candidate.rss \
  --interface evals/interfaces/eval_api.rssi
```

Compare diagnostics by code, not locations or rendered prose. `report.v1.json` is a runner-neutral envelope; its candidate metadata keeps task mode distinct from generation mode (`prompt_only`, `language_card`, `repair_loop`, `constrained`, or `offline_fixture`) and records model/model version, nullable temperature, attempt, generation tokens, generation duration in milliseconds, and repair turns without prescribing a model or pass-rate gate.

Score an offline candidate set in process (no CLI shell-out or model access):

```bash
cargo run -p rsscript-xtask -- agent-eval \
  --tasks evals/tasks --candidates evals --output /tmp/rsscript-agent-eval.json
```

`--candidates` may mirror the corpus source layout or supply `<task-id>.rss`,
`<task-id>/candidate.rss`, and an optional `<task-id>.json` metadata sidecar
that conforms to `schemas/candidate.v1.json`.

The scorer's `provenance.static_check_environment` is
`standard_package_interfaces_plus_task_interfaces`: it deliberately matches
the default `rss check` environment rather than checking task interfaces in
isolation. Call-site `read` is canonical by omission in passing/review source;
parameter declarations retain `read`, while `mut` and `take` remain explicit.

Tasks may also declare explicit `completion_probes`. The scorer checks only
oracle-owned claims from those probes: complete fixed terminals must keep the
prefix appendable, and named semantic candidates must not produce a dead
prefix. Intentional repair-fixture failures are not oracle violations. Any
reported oracle violation makes `agent-eval` exit unsuccessfully.

## Status versus canonical spelling

`status` records one claim: the candidate parses, meets the task's target
diagnostic contract, and keeps every structural invariant. It deliberately says
nothing about how the program is spelled. Four measured candidates passed
`rss check` and still scored as failures purely on surface spelling, which
conflated two different questions.

Spelling is reported separately, per task, as `canonical_spelling`:

- `formatted` — `rss fmt` output is byte-identical to the candidate.
- `spelling_invariants_hold` — every invariant the task marks `"spelling": true`
  holds. Such an invariant records a surface fact the checker cannot observe
  (receiver-call shorthand, for instance) and is reported under the
  `canonical_spelling` scope rather than `target`, so it never decides `status`.
- `canonical` — both of the above.

Source-text invariants are checked against the written candidate *and* against
its `rss fmt` output. Accepted surface sugar — a brace struct literal, an
explicitly written call-site `read` — therefore satisfies an invariant written in
the canonical spelling. Formatting can never re-add an operation a candidate
deleted, so a genuine structural loss still fails.

`destructive-symbol` is intentionally a review invariant, not a compiler error: its candidate type-checks because the interface is available, but an acceptable repair removes `Dangerous.write_text`.

`target_call_excludes` checks resolved semantic call targets and fails closed
when analysis or call resolution is incomplete. Use it for forbidden operations;
`target_source_contains` and `target_source_excludes` only check literal source
text and do not establish whether an operation occurs.

## Generation tasks

Alongside the ten original repair/review fixtures, `tasks/` carries twenty
`generation`-tagged tasks that describe a small embedded-automation program in
natural language (`prompt`) rather than handing a broken candidate to a model.
Their `fixtures/<id>/candidate.rss` is a *verified reference solution*: each one
was checked with

```bash
cargo run -q -p rsscript-cli --bin rss -- check \
  evals/fixtures/<id>/candidate.rss --interface evals/interfaces/<name>.rssi
```

so the task is known to be solvable. They keep `mode = "review"` because the
seed candidate already satisfies the target contract; their `expected/*.json`
invariants record the structural facts a correct solution must exhibit
(`retains(`, `Dyn<...>`, `take`/`mut` at the call site, `task_group`, `select {`)
and the diagnostics it must not emit.

`tools/gen_eval_tasks.py` regenerates those twenty task/expected pairs
deterministically; edit the table there rather than the generated files.

## Collecting model samples

`tools/collect-model-samples.py` is an optional, caller-owned runner that drives
the locally installed Claude Code CLI over these tasks and writes candidates in
the layout `agent-eval --candidates` accepts. It is not part of the offline
corpus contract: nothing in `evals/` requires model access, and the scorer still
never shells out to a model.

```bash
python3 tools/collect-model-samples.py --model sonnet --mode prompt_only
cargo run -p rsscript-xtask -- agent-eval \
  --tasks evals/tasks \
  --candidates evals/samples/sonnet/prompt_only \
  --output evals/samples/sonnet/prompt_only/report.v1.json
```
