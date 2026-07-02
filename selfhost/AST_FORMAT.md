# Canonical AST-dump format (parser AST parity oracle)

This is the step-1 contract for moving the self-hosted frontend from **token
parity** (see `FORMAT.md`) to **frontend object parity**: the rss parser will one
day emit this exact dump, and the Rust oracle (`crate::syntax::parse_source_raw`,
the surface-preserving tree — no desugaring) emits it too. Byte-identical dumps =
AST parity. This file ships *before* `parser.rss` builds an AST, exactly as the
token `FORMAT.md` + oracle preceded the rss lexer.

## Shape

An AST dump is an **indentation tree**, one node per line:

```
<indent><TAG>[ <key>=<value>]*[ <PAYLOAD>]
```

- **indent** — exactly `2 × depth` spaces. A node's children are the following
  lines at `depth + 1`. There are no closing delimiters; structure is indentation
  alone (easy to diff, easy for a recursive-descent producer to emit).
- **TAG** — a fixed lowercase node tag (e.g. `program`, `fn`, `block`, `binary`,
  `ident`). The tag set is closed and enumerated below.
- **key=value attributes** — a fixed, ordered set per tag. Values are always
  whitespace-free tokens: identifiers, enum names, `true`/`false`. **Optional**
  attributes (e.g. `alias=`, `effect=`) appear only when present in the AST;
  their absence is meaningful and both sides must agree.
- **PAYLOAD** — free-form literal text (an identifier name, number text, or the
  raw content of a string/char literal). It is always the **last** element of the
  line, so it may contain spaces. It is escaped exactly like a token payload:
  `\` → `\\`, newline → `\n`, tab → `\t`, carriage return → `\r`. Only leaf
  literal nodes (`ident`, `number`, `string`, `char`, `multiline`, and the
  `pat-literal` payload) carry one.

Lines are `\n`-separated with a trailing `\n`. UTF-8.

## Spans are deferred (tier 0 only)

Like the lexer's tier ladder (`FORMAT.md`), AST parity is graded and starts at
**tier 0 = structure + payload, positions ignored**. The tier-0 dump carries **no
spans at all**. When span parity is tackled (the last phase), each node line gains
a trailing ` @L:C:N` field that the harness strips below the active tier — so a
producer without span tracking can still pass tier 0. Until then, span-only AST
vectors (feature/profile spans, the malformed-*-span vectors) are represented by
structural marker lines with **no** coordinates (see "Diagnostics" below).

## Top level

The root is a single `program` node. Its children are emitted in this **fixed
section order** (mirroring the `Program` struct, not source order across
categories):

1. `feature <name>` — one per enabled file feature, in order. `<name>` ∈
   `local native unsafe async device ffi reflection`.
2. the `items` vector, in source order (see "Items").
3. `protocol <name>` — one per `Program.protocols` entry.
4. `protocol-impl protocol=<p> type=<t>` — one per `Program.protocol_impls`,
   each with `mapping method=<m> target=<t>` children.
5. diagnostics markers (only when non-empty; see below).

`feature_scopes`, `feature_spans`, and `profile_spans` are parse bookkeeping and
are **not** part of the dump.

### Diagnostics markers

Emitted only when the corresponding vector is non-empty (well-formed input emits
none):

- `unknown-feature <name>` / `duplicate-feature <name>` — payload is the name.
- `unknown-top-level` / `malformed-declaration` — one line each (span-only in the
  AST, so no payload at tier 0).

## Items

- `module path=<a.b.c>` — dotted path joined by `.`.
- `use path=<a.b.c> glob=<bool>` — plus optional `alias=<name>`.
- `type kind=<class|struct|resource> name=<n> public=<bool> opaque=<bool>`
  children: `generic` params, `derive <name>`, `field` decls, and `drop` (a
  `block`) when a drop body is present. `malformed-generic` / `malformed-field`
  marker lines when those span vectors are non-empty.
- `sum name=<n> public=<bool>` — children: `generic`, `derive`, `variant name=<n>`
  (each with `field` children).
- `type-alias name=<n> public=<bool>` — children: `generic`, then `target` (a
  `type` node).
- `const name=<n> public=<bool>` — optional child `type` (annotation), then
  `value` holding the initializer expression.
- `fn name=<n> public=<bool> async=<bool> native=<bool> has-body=<bool>` — plus
  optional `default-impl=true` and `returns-fresh=true` (booleans, so they stay
  attributes). Children in order: `deprecated <reason>` and `lower-name <sym>`
  when present (child lines, not attributes, since a reason may contain spaces),
  then `generic` params, `param`s, `return-type` (a `type`) when present,
  `effect-name <n>`/`effect-retains <n>`, and `body` (a `block`). `malformed-*`
  marker lines as applicable.

### Supporting nodes

- `generic name=<n>` — optional child `bound managed|struct|resource` or
  `bound protocol=<name>`.
- `field name=<n> handle=<bool> weak=<bool>` — child `type`, optional child
  `default` (an expression).
- `param name=<n>` + optional `effect=<read|mut|take>` — child `type`, optional
  child `default`.
- `type name=<n> fresh=<bool> noescape=<bool> owned=<bool>` — children: `arg`
  (a `type`) per generic argument, `fn-param` (a `type`, optional `effect=`) per
  `Fn(...)` parameter, `fn-return` (a `type`) when present.

## Statements (children of a `block`)

`let kind=<managed|local> name=<n> mut=<bool> async=<bool>` (+ optional
`malformed=true`, `destructure=a,b,_`) · `return` (optional `value` child) ·
`with binding=<n>` · `if` (`cond`, `then`, optional `else`) · `loop` (optional
`cond`, then body) · `for binding=<n> async=<bool>` (`iter`, body) ·
`match` (optional `effect=`, `value`, `arm`s) · `task-group` · `select`
(`select-arm binding=<n>` → `operation`, body) · `break` · `continue` ·
`let-else binding=<n>` (`pattern`, `value`, `else`) · `assign` (`target`,
`value`) · `expr-stmt` (one expression child) · `unknown-stmt` ·
`malformed-with|if|loop|for|match`.

Body wrappers `cond`/`then`/`else`/`value`/`iter`/`target`/`operation`/`resource`
are single-child grouping nodes that name the role of the expression/block beneath
them.

## Patterns

`pat-binding name=<n>` · `pat-variant name=<n>` (sub-pattern children) ·
`pat-struct name=<n> rest=<bool>` (`pat-field name=<n> ignored=<bool>` + optional
`binding=<n>`, `effect=`, and a nested pattern child) · `pat-literal kind=<int|
string|char|bool>` + payload · `pat-list rest=<none|ignore|NAME>` (optional
`list-prefix` / `list-suffix` groups of patterns) · `pat-wildcard`.

## Expressions

Leaves: `ident` · `number` · `string` · `char` · `multiline` — each carries the
raw text as PAYLOAD.

Compound: `object` (`object-field name=<n>` → value) · `map` (`map-entry` →
`key`, `value`) · `array` (item children) · `binary op=<name>` (`left`, `right`
— actually the two operand expressions as direct children, left then right) ·
`field-access name=<n>` (base child) · `index` (base, then index) ·
`call` (a `callee-*` child, then `arg` children) · `effect kind=<read|mut|take>`
(value child) · `manage` · `spawn` · `await` · `try` (single child) ·
`closure explicit=<bool>` (`closure-param <n>`, `capture effect=<e> name=<n>`,
`declared-effect <n>`, `body`) · `match-expr` (optional `effect=`, `value`,
`arm`s) · `unknown-expr`.

Callee child of a `call`: `callee-name name=<n>` · `callee-qualified
namespace=<ns> name=<n>` · `callee-receiver method=<m>` (+ optional `effect=`,
receiver expression child).

`arg` (+ optional `name=<n>`, `malformed=true`) — one expression child.

Binary op names: `add subtract multiply divide modulo bit-and bit-or bit-xor
shift-left shift-right equal not-equal less less-equal greater greater-equal
logical-and logical-or`.

## Producer & the module-story decision

The rss producer is `selfhost/astdump.rss`, a recursive-descent parser that
**streams** this dump (the dump is a pre-order traversal, so no handle-based AST is
materialized — each parse fn emits its node line(s) at a threaded depth and returns
the next token index). It reuses the shared `scan.rss` tokenizer/accessors, which
the harness prepends at compile time.

**Decision (module story):** the single-file VM model still has no cross-file
import, so — like the lexer/parser/checker — `astdump.rss` is authored as one file
and concatenated after `scan.rss` by the harness. We commit to concatenation for
now rather than blocking AST parity on a new language module feature; if the
producer grows unwieldy, the mitigation is to split the *Rust-side* corpus gate
into sampled + full tiers (already done: curated `samples/ast/*` is the fast
non-ignored gate, the full corpus is `#[ignore]`), not to invent an import system.
Revisit only if a genuine multi-file need appears.

## Oracle

`crate::syntax::parse_source_raw` (never the desugared `parse_source`) is truth.
The serializer lives in `crate::selfhost_parity` (`#[cfg(test)]`); it is total
over the AST so a producer can never pass by silently dropping a node. Divergences
are recorded as `SH-NNN` entries in `docs/ledgers/rss-selfhost-ledger.md`.
