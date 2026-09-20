# ADR 0236: A diagnostic that names a type must also name the replacement

- Status: Accepted
- Date: 2026-09-20

## Problem

`docs/planning/2026-09-model-failure-modes.md` measured 300 model-written
RSScript candidates across two models and found one rule that holds for every
failure class it tracks: a diagnostic whose fix carries a concrete replacement
gets taken and cleared, and a diagnostic that only describes the failure
persists through the whole repair loop. Sonnet applied 121 of 129 offered
machine-applicable replacements and haiku 131 of 154.

Two classes were on the wrong side of that rule.

`RS0206` named a replacement *callee* and nothing else. Both models take the
rename — sonnet 87% of the time, haiku 74% — and then fail on the arguments of
the call the compiler had just named, because the name was all it named.

`RS0207` named the expected type and offered no replacement at all. Its fix was
`manual` for every instance. It is the only class haiku's repair loop
*introduces* more often than it clears, five against one, and the loop spends
turns re-deriving a conversion the checker could have spelled.

A third, smaller problem sits underneath `RS0207`: the helper that split a
`Fn(...)` type's parameter list used `str::split_once(')')`, which stops at the
first `)`. For a callback whose own parameter is a function type that cut the
list mid-type, so the diagnostic a caller reads reported an expected type of
`Fn(read Int` and an arity one parameter short of the truth.

## Decision and non-goals

Three additions to the semantic frontend's diagnostic surface, and one bug fix.

1. `NameSuggestion` gains `signature: Option<String>`, and
   `unknown_callee_diagnostic_with_suggestions` renders the best candidate's
   whole signature in its help title. Runners-up stay bare names.
2. `ArgumentTypeRepair` and
   `argument_type_mismatch_diagnostic_with_repair` are added.
   `argument_type_mismatch_diagnostic` is retained and delegates with `None`.
   Three argument shapes carry a more specific fix:
   - an identifier holding an unhandled `Result<T, E>` or `Option<T>` where `T`
     is wanted, in a function that can propagate the failure under the `RS0013`
     rules, gets a machine-applicable insertion of `?` after the identifier;
   - an `Int` literal where a `Float` is wanted gets a machine-applicable
     replacement with the same literal plus `.0`;
   - a `String` literal where an `Int` is wanted gets advice naming
     `String.parse_int` and **no** edit.
3. `render_callable_signature` is added to `rsscript_semantics::generation` and
   is the single renderer for a checked signature, used by both generation
   continuations and the `RS0206` help.
4. `FixEdit::insert_after` is added to `rsscript-diagnostics`, and the
   `Fn(...)` parameter-list split is made paren- and generic-balanced.

Non-goals. No new diagnostic code, no code retired, and no change to which
programs are accepted or rejected: every addition is to the *fixes* and *help*
attached to a diagnostic instance, not to the rule that produced it. The
`String`-to-`Int` case deliberately carries no edit — `"12"` and `"twelve"` are
the same shape to the checker and only one of them has a value, so an unseen
edit there would be a guess. `?` is offered only where the try checker would
accept it; where the enclosing function cannot propagate, the fix is withheld
rather than trading `RS0207` for `RS0013`.

## Compatibility and migration

No ABI, MIR, bytecode, Artifact, Provider or persisted-data contract changes.
Nothing in `rsscript-abi-model`, `rsscript-mir`, `rsscript-bytecode` or
`rsscript-provider-api` is touched, and no wire type, signature hash or
serialized form gains, loses or reorders a field.

The `Diagnostic`/`Fix`/`FixEdit` JSON shape is unchanged. `Fix` already carried
an optional `edit`, and `rss check --json` and `rss fix --json` already
documented machine-applicable edits as instance-level data; `RS0207` instances
now populate the field the schema always had. A consumer that reads only
`match_argument_type` sees exactly what it saw before — the advisory fix is
retained, unchanged, behind the specific one.

Source-compatibility for the semantics crate's Rust API is additive.
`NameSuggestion` gains a public field, so an exhaustive struct literal outside
the crate would need updating; it has no such constructor outside this
workspace. `argument_type_mismatch_diagnostic` keeps its signature.

Two rendered strings change and are not contracts: the `RS0206` help title now
carries a signature, and a checked signature is rendered with `read` omitted —
the spelling a call site uses and the spelling `docs/generated/signatures.md`
prints — rather than with `read` written out.

Rollback is a revert; no data written under this change has to be read back.

## Verifier and security impact

None. No untrusted input crosses a new boundary, no validation is relaxed, and
no cancellation or resource behavior changes. A fix edit is data returned to a
caller that chooses whether to apply it; `rss fix --write` applies only edits
marked `machine-applicable`, as before.

## Provider and backend impact

None. The diagnostics changed here are semantic-frontend diagnostics that run
before any lowering, and no VM, AOT, JIT or conformance surface reads them.

## Evidence

- `rsscript-semantics`: `argument_type_mismatch_carries_a_repair_where_one_is_mechanical`,
  `a_function_types_parameter_list_is_split_at_its_own_closing_paren`,
  `unknown_callee_help_carries_the_best_candidates_whole_signature`,
  `unknown_callee_help_falls_back_to_the_bare_name_without_a_signature`,
  `argument_type_mismatches_carry_the_edit_the_checker_can_derive`,
  `a_try_repair_is_withheld_where_the_function_cannot_propagate`.
- `rsscript-cli`: `fix_json_carries_argument_type_repairs_and_withholds_the_unsafe_one`,
  `unresolved_calls_suggest_in_scope_names_and_rename_fixes_apply`.
- `rsscript-xtask`: the language-card generator's tests, which assert the
  rendered card carries every signature and that the `RS0207` explanation
  describes the frontend check.
- `docs/planning/2026-09-model-failure-modes.md` carries the before/after
  measurement these changes were made against.
