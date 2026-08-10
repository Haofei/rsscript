# ADR 0006: Resource terminal-path evidence

## Status

Accepted

## Problem

Provider-owned resources can outlive the immediate external call. A bare
runtime error is insufficient evidence that a terminal execution path cleaned
those resources, particularly when a script error, Provider error,
cancellation, deadline, or cleanup failure occurs at the same time.

## Decision and non-goals

The report-preserving VM execution API always finalizes the per-run Provider
resource registry before it returns an `EvalExecutionReport`. Usage records
created, successfully cleaned, cleanup failures, peak live resources, and live
resources at return. A cleanup failure does not resurrect a slot or discard the
original terminal evidence.

This decision does not make arbitrary legacy convenience APIs report-preserving
and does not promise process isolation for Provider cleanup code.

## Compatibility and migration

The counters are additive execution-report evidence. Existing resource handles
and cleanup callbacks keep their ABI. Callers that need an audit record must
use the report-preserving execution path exposed by the SDK runtime façade.

## Verifier and security impact

This is a runtime finalization contract, not a bytecode-verifier claim. The
resource table remains generation-safe and cleanup occurs at most once per live
slot; failed cleanup is counted and exposed without serializing Provider error
details into default portable reports.

## Provider and backend impact

Providers still register resources through their call context. VM execution
finalizes them for every terminal path. Other backends must produce equivalent
usage evidence before claiming conformance.

## Evidence

The SDK MIR migration fixture executes verified bytecode with a resource
registered by a contextual Provider. It covers success, script error, Provider
error, cancellation, deadline, and cleanup failure, asserting exact cleanup and
terminal usage counters for each case.
