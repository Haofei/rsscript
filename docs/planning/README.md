# Planning Docs

Planning documents are non-normative. Use them for roadmap context, design
rationale, and measured evidence, not as the source of language or CLI truth.

Current authority order:

1. [`../spec/RSScript_v0.7_Spec.md`](../spec/RSScript_v0.7_Spec.md) for language
   syntax and semantics.
2. [`../spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md)
   for VM/JIT execution contracts.
3. [`../../README.md`](../../README.md) and `rss --help` for implemented user-facing
   commands.
4. The active planning docs below.

The package-manager design doc is still versioned v0.6. Treat it as design
background unless a claim also matches the root README, `rss --help`, or current
tests.

## Active Roadmaps

| Doc | Use it for |
|-----|------------|
| [`spec-todo.md`](spec-todo.md) | Current v0.7 implementation backlog and explicitly removed non-goals. |
| [`vm-optimizing-jit-plan.md`](vm-optimizing-jit-plan.md) | Current VM/JIT optimization state, shipped slices, and remaining perf work. |
| [`dev-build-test-speed-plan.md`](dev-build-test-speed-plan.md) | Proposed developer-loop speed work. |
| [`refactor-plan.md`](refactor-plan.md) | Proposed behavior-preserving module decomposition. |
| [`declarative-rewrite-roadmap.md`](declarative-rewrite-roadmap.md) | Design path for graph rewrite / tinygrad-style scheduler work. |
| [`ml-perf-todo.md`](ml-perf-todo.md) | ML framework performance follow-ups. |
| [`cross-isolate-design.md`](cross-isolate-design.md) | Message-channel contract status and remaining true-isolate design. |

## Evidence And Historical Notes

These files are intentionally kept because they record measurements, rejected
approaches, or design rationale that should not be rediscovered. They are not the
current roadmap unless an active doc above links to them as the source for a
specific claim.

| Doc | Status |
|-----|--------|
| [`RSScript_AI_Generation_Feedback_v0.1.md`](RSScript_AI_Generation_Feedback_v0.1.md) | Final design spec for an AI generation feedback loop; implementation planning reference. |

## Maintenance Rule

When a plan ships, update its status line and either keep it in "Active Roadmaps"
only if more work remains, or move it to "Evidence And Historical Notes". When a
feature changes language semantics, update the spec first and make the plan point
back to the spec instead of restating the rule.
