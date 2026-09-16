# ADR 0122: Language-service symbol queries reuse session parsing

- Status: Accepted
- Date: 2026-08-14

## Context

Symbol navigation and document outlines are syntax-derived editor queries. They
previously parsed document text independently of CompilationSession, creating
duplicate parsing and revision-cache behavior.

## Decision

The semantic symbol module exposes program-based constructors alongside its
source-text convenience functions. LanguageService supplies programs from its
CompilationSession parse cache to symbol-index and outline queries.

## Consequences

Dependency facts, symbol navigation, and outlines share one revisioned parse
source. Full workspace resolution and diagnostics remain later session queries.
