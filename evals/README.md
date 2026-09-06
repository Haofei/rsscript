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

`destructive-symbol` is intentionally a review invariant, not a compiler error: its candidate type-checks because the interface is available, but an acceptable repair removes `Dangerous.write_text`.
