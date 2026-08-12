# ADR 0052: Semantic ownership of `for` iterable diagnostics

## Status

Accepted.

## Problem

The compiler body checker diagnosed `for` values whose resolved type was not
`List<T>`, or whose `await for` value was not `Stream<T>`. This is a checked-HIR
type rule, separate from compiler loop-flow handling.

## Decision

`rsscript-semantics` owns `for_iterable_diagnostic`. The compiler supplies the
resolved iterable type already recorded in HIR, appends any diagnostic, and
continues to own local-state and loop-flow orchestration.

## Compatibility

The diagnostic code, span, messages, causes, and suggested fixes remain
unchanged. Unresolved iterable types receive no additional diagnostic.

## Security and verification

This alters no Provider contract, artifact layout, verifier rule, or runtime
behavior.

## Evidence

Focused semantic tests cover rejected non-list input and accepted fresh list and
async stream inputs. Architecture tests prevent duplication in the compiler.
