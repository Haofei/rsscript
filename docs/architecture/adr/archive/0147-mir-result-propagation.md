# ADR 0147: MIR owns Result construction and propagation

- Status: Accepted
- Date: 2026-08-14

## Problem

The direct checked-HIR MIR path treated `Ok`/`Err` as unresolved calls and
rejected `?`. As a result, ordinary verified SDK execution could not run a
basic structured task group that returns `Result` without selecting the legacy
source-shaped executable-IR feature.

## Decision and non-goals

MIR now has `MakeResult` and `TryResult`. `MakeResult` represents the
canonical `Ok` or `Err` constructor without preserving a source builtin name.
`TryResult` identifies its source value and the reverse lexical list of live
resource places to clean up if the runtime short-circuits. The VM bytecode
codegen maps these typed operations to the existing verified `MakeVariant` and
`TryResult` instructions.

This does not add general sum-variant lowering or change Result language
semantics. The scalar MIR conformance interpreter now covers the closed
Result subset as a migration oracle; the VM remains the reference executor for
runtime resource and scheduler behavior.

## Compatibility and migration

The Artifact and Provider contracts remain unchanged because the emitted v1
opcodes already existed. Existing legacy artifacts continue to load. Direct
MIR execution now covers Result-returning internal task functions and `?`
without selecting executable IR.

## Verifier and cleanup impact

MIR validation requires the Result source to dominate `TryResult` and checks
each named cleanup place is live. The non-normal cleanup edge is therefore
present in typed MIR before bytecode codegen, instead of being inferred by a
backend. Runtime failure/cancellation cleanup beyond `?` remains governed by
the existing VM resource model.

## Evidence

The SDK execution-feature tests run a task group returning `Ok` values and a
verified bytecode program that short-circuits an `Err`; the MIR conformance
test executes the same short-circuit shape. MIR, lowerer, codegen, and SDK
lint/test closures are checked with warnings denied.
