# ADR 0104: Semantic ownership of value-property queries

- Status: Accepted
- Date: 2026-08-12

## Context

The compiler's local-flow implementation previously defined both the language's
Copy-value classification and the conservative contract for values that may
cross an isolate boundary. Assignment checking, call checking, and local CFG
state all consumed those functions. Keeping the classification alongside one
consumer made it easy for the other consumers to drift or recreate the same
rule.

## Decision

`rsscript-semantics` owns `is_copy_type_name` and
`is_cross_isolate_transferable`. The latter remains deliberately conservative:
Copy scalars plus immutable owned `String` and `Bytes` values may cross an
isolate boundary; generic, managed, container, handle, closure, and structured
values do not until the language contract explicitly expands that set.

Compiler assignment, call, and CFG checks consume these semantic queries. An
architecture test prevents the local-flow module from reintroducing either
definition.

## Consequences

This moves a language classification to its canonical layer without changing
source syntax, artifact encoding, Provider ABI, or execution behavior. The
current string-based query is a transition boundary over normalized HIR type
renderings; a future typed-MIR/type-table migration may replace its input with
canonical type identities without moving rule ownership back to the compiler.
