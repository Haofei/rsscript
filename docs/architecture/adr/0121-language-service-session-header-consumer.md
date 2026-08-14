# ADR 0121: Language service consumes session header facts

- Status: Accepted
- Date: 2026-08-14

## Context

CompilationSession now owns revision-keyed module and import facts. Keeping a
separate language-service parser path would retain two cache owners for the
same dependency information.

## Decision

LanguageService mirrors document updates into CompilationSession and resolves
its diagnostic dependency cache through the session ModuleHeader query. Its
remaining document caches stay in place while workspace diagnostics migrate out
of the compiler compatibility facade.

## Consequences

The language service now exercises the shared frontend query boundary in its
main diagnostic path. Interface invalidation remains a follow-up migration, so
this change does not yet remove the local cache or the compiler dependency.
