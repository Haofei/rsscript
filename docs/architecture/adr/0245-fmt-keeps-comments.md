# ADR 0245: `rss fmt` keeps every comment

## Status

Accepted. Date: 2026-09-23.

## Problem

`rss fmt` prints the AST, and the AST has no comments: the lexer skipped
`//` comments and the formatter never saw them, so every comment in a
formatted file was deleted. `rss fmt` runs inside the eval generation loop and
its output is read by agents and people, so that was silent data loss on every
format.

Checking every checked-in `.rss` file for a second-format fixpoint also found
three older ways the formatter could turn a valid file into an invalid one:

- a function with an empty body (`fn f() -> Unit {}`) was printed as a
  bodyless declaration, which a `.rss` file rejects;
- protocols, which the formatter prints first, were printed above the
  file's `module` declaration, which must come first;
- the argument splitter (`parser/items.rs::split_param_ranges`) counted the
  `<` of a comparison as an open angle bracket, so in
  `pick(flag: g(flag: x < y), value: 5)` — and in the formatter's own
  multi-line layout of a closure argument whose body compares — the comma
  after the argument did not separate, and the call failed to parse.

## Decision and non-goals

- The lexer records line comments beside the token stream
  (`lexer::lex_with_comments`, `LineComment`); the token stream and the parser
  are unchanged.
- `format_source` puts each comment back (`formatter/comments.rs`). A comment
  is *trailing* when code precedes it on its line and *own-line* otherwise.
  Before printing a statement, argument, match or `select` arm, struct field,
  sum variant, parameter, list element, protocol method, impl mapping, or
  top-level declaration on a fresh line — and before the closing bracket of
  each of those lists — the formatter flushes every pending comment written
  before that point: a trailing one to the end of the previous output line, an
  own-line one on its own line at the current indent, after a blank line when
  the source had one. Because the formatter prints protocols before and impls
  after other declarations, each comment is owned by the top-level declaration
  it is written in, trails, or leads, and moves with it. A comment inside an
  argument, parameter, or list literal keeps that list one element per line.
  A comment the formatter finds no nearer place for is printed at the next
  line boundary; none is dropped.
- An empty function body is printed as `{` and `}`; `module` and `use`
  declarations are printed before the hoisted protocols; the argument splitter
  keeps a stack of open brackets, treats `<` as one only when it opens a type
  argument list (it follows a name and a matching `>` is followed by what can
  follow a type), and lets each closer pop back to its own opener.
- `format_program(&Program)` still prints no comments (an AST has none), and
  the package lock hashes interfaces through the new
  `format_source_without_comments`, the comment-free normal form it used
  before, so a comment edit does not change an interface's reviewed hash.

Non-goals: no block comments, no doc-comment semantics, no reflowing of
comment text, and no preservation of blank lines between statements that have
no comment between them.

## Compatibility and migration

Formatter output changes: comments now survive, and the three invalid outputs
above are valid. Files formatted by an earlier `rss fmt` already lost their
comments; those cannot be recovered from the formatted file. The parser now
accepts calls it previously rejected (a comparison inside a nested argument
followed by another argument); no previously accepted program changes meaning.
Package lock hashes are unchanged. No Artifact, MIR, bytecode, or Provider
format changes. Rollback is a revert.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

- `rsscript-syntax` `formatter::tests`: `keeps_comments_at_the_top_level_and_between_items`,
  `keeps_comments_inside_blocks_and_between_statements`,
  `keeps_comments_inside_argument_lists`,
  `keeps_comments_between_match_and_select_arms`,
  `keeps_comments_in_declarations`,
  `keeps_comments_on_protocols_and_impls_printed_out_of_source_order`,
  `keeps_comments_in_list_literals_and_empty_bodies`,
  `a_comment_marker_inside_a_string_is_not_a_comment`, and
  `a_comparison_inside_a_nested_argument_does_not_merge_arguments` — each
  asserts the exact output and that a second format is a fixpoint.
- `rsscript-cli` `tests/fmt_comments.rs`: runs `rss fmt` on every `.rss` file
  under `examples/`, `stdlib/`, `packages/`, and
  `crates/rsscript-sdk/tests/fixtures/`, asserting that no comment is lost,
  that formatting the output again changes nothing, and that only
  syntax-error fixtures are refused.
