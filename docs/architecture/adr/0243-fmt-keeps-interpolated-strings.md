# ADR 0243: `rss fmt` prints an interpolated string as written

## Status

Accepted. Date: 2026-09-23.

## Problem

The parser desugars `$"{name} scored {total}"` to
`String.format(template: read "{} scored {}", args: read [name, total])`
(`parser/expr.rs::parse_interpolated_string_expr`), and the formatter printed
that call. `rss fmt` therefore rewrote every interpolated string into a
`String.format` call, so the form could not appear in the language card, whose
canonical-form rows are by definition what `rss fmt` prints. Models, never
shown interpolation, wrote nested `String.concat(left: ..., right: ...)`
chains instead.

## Decision and non-goals

The formatter recognizes the desugared call and prints it back as `$"..."`
(`formatter.rs::inline_interpolated_string`). The desugared call is
distinguishable from a hand-written one without a new AST field: the parser
gives the call, both arguments, and the template literal the interpolated
token's own span, which a written `String.format(template: "...", args: [...])`
cannot have because its callee and its string literal are different tokens. A
hand-written `String.format` call is still printed as a call.

Printing rules: the template's `{{`/`}}` and backslash escapes are kept as
written, each `{}` placeholder is replaced by its item printed on one line, and
a lone `}` in the source is printed as its canonical `}}`. An interpolation
whose arguments are marked malformed, whose placeholder count disagrees with its
items, or whose item cannot be printed on one line falls back to the call form.

Non-goals: the parser, the AST, the checker, and lowering are unchanged, and so
is the rule that each interpolated item is a `String` (the desugared `args` is
a `List<String>`).

## Compatibility and migration

Formatter output only. A file containing `$"..."` now formats to itself instead
of to a `String.format` call; both spellings parse to the same AST, so no
program changes meaning. Files already rewritten by an earlier `rss fmt` keep
their `String.format` calls, which remain valid. No Artifact, MIR, bytecode, or
Provider format changes. Rollback is a revert.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

- `rsscript-syntax`: `formatter::tests::interpolated_strings_are_printed_as_written`
  (fixpoint with escapes, a nested string literal, an effect-wrapped item, and
  a hand-written `String.format` that stays a call).
- `rsscript-xtask`: `interpolation_row_checks_and_is_what_fmt_prints` and the
  worked-example fixpoint tests, which now contain interpolated strings.
- `rsscript-sdk`: `interpolated_strings_build_and_run` and the pass fixture
  `string-interpolation.rss` (build corpus).
