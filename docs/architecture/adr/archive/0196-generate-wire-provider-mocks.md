# ADR 0196: Generate Provider mocks over canonical wire values

- Status: Accepted
- Date: 2026-08-14

## Problem

Generated Provider traits already used descriptor-derived types, but their
generated mock implementations recorded `NativeValue` and registered legacy
native callables. This made ordinary contract tests exercise a compatibility
path rather than the canonical Provider wire boundary.

## Decision

Generated mocks record `WireValue` arguments and register `WireSync` or
`WireAsync` callables according to the descriptor's async shape. They still
return the same structured unavailable error until a test supplies behavior;
the change affects only the boundary representation used to reach that error.

## Consequences

Provider conformance and generated contract tests now follow the canonical
typed dispatch route. `NativeValue` remains only in visibly named compatibility
adapters, including resource adapters required for the existing v1 VM/report
projection.
