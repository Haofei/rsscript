# ADR 0008: Syntax ownership of formatting

## Status

Accepted

## Problem

Formatting was implemented in `rsscript-compiler` even though it only consumes
the parsed syntax tree. This forced editor formatting requests through the
compiler façade and obscured the required `syntax -> semantics -> lowering`
dependency direction.

## Decision and non-goals

`rsscript-syntax` owns `format_source` and `format_program`, including their
deterministic formatting tests. The compiler retains a transitional re-export;
language-service imports the syntax formatter directly.

This decision does not move lint or semantic diagnostics into syntax.

## Compatibility and migration

The formatter API and output remain unchanged. Existing compiler callers keep
working through the re-export while new editor-facing callers use syntax.

## Verifier and security impact

Formatting has no artifact or verifier authority. The move prevents a
format-only editor request from gaining compiler, VM, Provider, or package
dependencies.

## Provider and backend impact

Providers and execution backends are unaffected.

## Evidence

The syntax, compiler, and language-service test suites run after the move; the
formatter's prior golden and round-trip tests now execute in `rsscript-syntax`.
