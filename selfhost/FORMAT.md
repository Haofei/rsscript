# Canonical token-dump format (lexer parity oracle)

Both sides emit **one token per line**, in lex order. A line is three
tab-separated fields:

```
<line>:<col>:<len>\t<KIND>\t<PAYLOAD>
```

- `line`, `col` — 1-indexed position of the token start (matches `Span.line` /
  `Span.column`). `len` — byte length of the token (matches `Span.length`).
- `KIND` — one of the exact `TokenKind` variant names:
  `Ident Number String Char InterpolatedString MultilineString Keyword Symbol
  Unknown Eof`. `Char` is a character literal `'c'`; `Unknown` is a source
  character outside the lexical inventory (payload is that raw char).
- `PAYLOAD` — the token's raw text (ident/number/keyword/symbol/char text, or the
  raw content captured between string delimiters). For `Eof` the payload is empty.
- PAYLOAD is escaped so a token is always exactly one line:
  `\` → `\\`, newline → `\n`, tab → `\t`, carriage return → `\r` (backslash first).

## Comparison tiers (env `RSS_SELFHOST_TIER`, default `0`)

The rss lexer emits all three fields on every line; the harness parses both dumps
into structured tokens and compares only the fields for the active tier, so a
single rss implementation supports graded parity. (The current `selfhost/lexer.rss`
has not implemented span tracking yet, so it emits placeholder `0:0:0` positions
and is exercised at tier 0 — see the note below.)

- **tier 0** (default): compare `(KIND, PAYLOAD)` — pure tokenization logic.
- **tier 1**: also compare `(line, col)` — span position arithmetic.
- **tier 2**: also compare `len` — byte-length arithmetic (hardest).

Positions/len in the rss output are ignored at tier 0, so a lexer that has not
yet implemented span tracking can emit placeholder `0:0:0` and still pass tier 0.

The Rust oracle is `crate::lexer::lex()` (the real lexer); it is never modified —
it defines truth. All divergences are recorded as `SH-NNN` entries in
`docs/ledgers/rss-selfhost-ledger.md`.
