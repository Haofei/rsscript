# ADR 0009: Syntax ownership of source linting

## Status

Accepted

## Problem

The deterministic public-surface lint pass depends only on parsed source and
diagnostic spans, but lived in `rsscript-compiler`. Editor lint requests thus
unnecessarily traversed the compiler façade.

## Decision and non-goals

`rsscript-syntax` owns `lint_source` and its tests. The compiler retains a
transitional re-export; language-service imports the syntax API directly.
Semantic type/call/ownership diagnostics remain semantics work, not lint.

## Compatibility and migration

Lint diagnostics and formatting remain unchanged. Existing compiler callers
continue through the re-export while editor clients use syntax directly.

## Verifier and security impact

No executable or verifier contract changes. This removes another compiler-only
dependency from bounded editor requests.

## Provider and backend impact

None.

## Evidence

Syntax, compiler, and language-service tests and Clippy run after the move.
