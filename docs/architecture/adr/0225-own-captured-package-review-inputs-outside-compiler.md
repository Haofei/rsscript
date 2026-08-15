# ADR 0225: Own captured package-review inputs outside the compiler

- Status: Accepted
- Date: 2026-08-15

## Problem

The optional package-review compatibility path kept its typed manifest and
bounded source-set loader in `rsscript-compiler/src/package`. Although normal
compiler entry points were already in-memory and provider-neutral, this made
the compiler the physical owner of project/review input representation.

## Decision

`rsscript-package-review` owns the captured package manifest model,
feature-selected source-set model, bounded loader, and source-level contract
extractor. The latter calls `CompilationSession` and syntax directly rather
than routing semantic facts through compiler-local helpers. It also owns the
neutral resource/task execution-fact collector used by package analysis. The crate depends
on `rsscript-project` for confined/no-follow capture and on
`rsscript-package-model` for versioned file-kind identity; it has no compiler
dependency.

The await collector receives a read-only runtime-intrinsic identity table
generated from the shared catalog. That removes the last production use of the
compiler-local `runtime_abi` module without coupling review evidence to the VM.
The dependency resolver likewise consumes project-captured manifest graphs and
uses bounded project reads for diagnostic spans; it no longer lives in the
compiler package module.

The compiler's opt-in `package` compatibility feature has a private forwarding
module during the staged migration so existing authorization, native, lock, and
review callers retain their established behavior. The reviewed compiler default
closure remains unchanged and does not select this crate.

## Consequences

This is the first physical S05.3 migration step, not its completion. Review
execution, risk/policy, lock/check/diff, and public compatibility composition
must move next before the forwarding module can be removed. Architecture tests
assert that the source-set file cannot return under `rsscript-compiler` and
that its loader continues to consume project-owned bounded capture APIs.
