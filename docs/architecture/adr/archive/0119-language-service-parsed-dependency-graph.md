# ADR 0119: Parse language-service dependency edges

- Status: Accepted
- Date: 2026-08-14

## Context

The language service maintained module and import invalidation by scanning text
lines for module and use declarations. That could disagree with the syntax
parser when comments, string literals, aliases, or formatting were involved.

## Decision

Language-service dependency discovery parses each document and derives module
and import paths from syntax Item::Module and Item::Use nodes. The service
keeps its document cache during the broader CompilationSession migration, but
does not own a second textual grammar.

## Consequences

Dependency invalidation follows RSScript syntax rather than a line-oriented
approximation. This is an incremental S04.1 step; diagnostics still use the
compiler transition query until workspace semantic queries are exposed from
CompilationSession.
