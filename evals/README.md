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

Alongside the ten original repair/review fixtures, `tasks/` carries forty
`generation`-tagged tasks that describe a small embedded-automation program in
natural language (`prompt`) rather than handing a broken candidate to a model.
The first twenty were added in September 2026; the second twenty were added on
2026-09-19 to cover the shapes a host actually asks an agent for — record-to-
report rendering, typed config errors, bounded retry, channel cancellation,
`with` cleanup on the error path, protocol selection by input, `noescape Fn`
helpers, local closures, payload patterns, `StringBuilder`,
`Map.get_or_default`, sum-type state machines, JSON array walks, `Option`
chains, `?` propagation, `select` against a deadline, two host interfaces in a
fixed order, index assignment, `let ... else`, and tuple destructuring.
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

`tools/gen_eval_tasks.py` regenerates those forty task/expected pairs
deterministically; edit the table there rather than the generated files.

Reference solutions are verified twice: with `rss check` against the task's
interfaces, and — for the core-only tasks — by building and running them with
`rss run --trusted-in-process --features execution`. A task whose only host
surface is a declared `.rssi` builds but cannot run, because the corpus ships no
provider implementation for those external symbols; that is the same for the
host-boundary tasks added in September.

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

### The collector formats what it stores

A candidate that checks clean is passed through `rss fmt`, and the formatted
source is what is written to `<task>/candidate.rss` and therefore what gets
scored. The model's own reply is kept beside it as `<task>/raw.rss`, and the
sidecar records `"formatted": true`.

This is deliberate. `canonical_spelling.formatted` measures a property of the
formatter, not of the model: it was the corpus's largest single gap — 32 to 33
of 50 candidates compiled and scored non-canonical purely on line breaks and
spacing no prompt can teach. Scoring the formatted source asks the question
that matters, which is whether the model wrote the language, and `raw.rss`
keeps the evidence of what it actually typed.

A candidate that does *not* check clean is stored exactly as written: the
formatter declines unparseable source, and a failing candidate's evidence is
the text that failed. If formatting a clean candidate somehow changed what the
checker says, the collector restores the raw text and records
`"formatted": false`.

The sidecar flag is provenance, not a measurement. The scorer re-derives
`canonical_spelling.formatted` from the source it reads, and that is what a
score claims; the flag only says which generation loop produced the sample.
