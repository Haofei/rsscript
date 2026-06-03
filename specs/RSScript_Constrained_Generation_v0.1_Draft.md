# RSScript Constrained Generation — API Draft v0.1

*Status: draft / design sketch. Implements the tooling side of Constitution
Article IX (§2B): make the source cheap to generate, not only cheap to review.*

## 1. Why

RSScript has **zero presence in any LLM's training data**. We attack that on two
layers:

```text
AGENT.md            teaches the language in-context           (persuasion)
constrained gen     forbids illegal tokens during decoding    (enforcement)
```

`AGENT.md` raises the model's prior; constrained generation makes an *invalid*
RSScript program **unrepresentable** — the decoder can only emit tokens that keep
the program on a path to a well-formed, well-typed result. This is the same idea
as grammar-constrained decoding (GBNF/Outlines/XGrammar), extended with a
**semantic** layer that only RSScript's own checker can provide.

The checker is already the authority RSScript trusts everywhere else (`rss
check`, the LSP, package review). This API exposes that authority **one token at
a time, over an incomplete program.**

## 2. The core problem: LLM tokens ≠ RSScript tokens

A model decodes in **BPE subword pieces**, not RSScript tokens. `read` might be
one piece or `re`+`ad`; `Path.from_string` is many. So the API works at the
**RSScript-token frontier** and an *adapter* projects that onto the model's
vocabulary as a logit mask (token healing). Two integration shapes, both built on
the same core:

```text
A. Acceptor   given the text so far (+ a candidate), say Valid / Invalid / Incomplete.
              Universal: drives rejection sampling, backtracking, speculative decoding.
              (MoonBit's "check token, backtrack if invalid" loop.)

B. Completion given the text so far, enumerate the legal next RSScript tokens
              (keywords, symbols, in-scope names, expected types).
              Drives logit biasing and dynamic grammar slices.
```

The adapter (out of scope here) turns B's continuation set + A's acceptor into a
per-step allowed-LLM-token mask.

## 3. Three layers (RSScript's "local + global sampling")

```text
L0 Lexical     is the in-progress token a valid prefix of some keyword / ident /
               number / string literal?                         (reuses lexer)

L1 Syntactic   given the token stream, which token kinds / keywords / symbols can
               grammatically come next?                          (reuses parser)

L2 Semantic    among L1's candidates, which names / types are in scope and
               type-valid here? After `Path.` only Path's methods; after `f(`
               only f's remaining parameter names with their required effect
               keyword.                          (reuses analyzer + symbol_index
                                                  + core_interfaces)
```

L0+L1 = "local sampling" (syntactic validity). L2 = "global sampling" (semantic
validity). L2 is where RSScript wins over a plain CFG grammar: it knows that
`Path.nonexistent` and an unbound identifier are dead ends *before* a token is
emitted.

## 4. The API

New public module `rsscript::generate`, layered on existing internals.

### 4.1 Prefix acceptor (the universal primitive)

```rust
/// Whether `partial_source` can still be extended into a well-formed,
/// well-typed program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixStatus {
    /// A valid, complete program as written.
    Complete,
    /// Incomplete but on track: at least one completion exists.
    Incomplete,
    /// A committed error: no completion can repair what is already written
    /// (the offending span is reported so the decoder can backtrack to it).
    Dead { at: Span, reason: String },
}

/// Cheap check used by rejection-sampling / backtracking decoders.
pub fn prefix_status(file: &str, partial_source: &str) -> PrefixStatus;
```

The hard part is distinguishing **Incomplete** ("truncated mid-construct, fine")
from **Dead** ("already wrong"). RSScript's parser is already error-tolerant
(`Unknown` / `Malformed` nodes), but tolerance is the wrong default here: a
truncation must be *pending*, not an error. See §6 for how this is obtained
without a second grammar.

### 4.2 Continuation enumerator (the steering primitive)

```rust
#[derive(Debug, Clone)]
pub struct Continuations {
    /// Keywords legal at this point (subset of the lexer KEYWORDS table).
    pub keywords: Vec<&'static str>,
    /// Punctuation/operator symbols legal here, e.g. `(`, `:`, `->`, `?`.
    pub symbols: Vec<&'static str>,
    /// Literal classes legal here.
    pub literals: Vec<LiteralClass>, // Int, String, Bool, ...
    /// Concrete in-scope names, already filtered by the semantic layer.
    pub names: Vec<Completion>,
    /// If the position has an expected type, the decoder can bias toward it.
    pub expected_type: Option<String>,
    /// True once a complete program is a legal stopping point (EOF allowed).
    pub may_stop: bool,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,            // e.g. "from_string", "path", "Response"
    pub kind: CompletionKind,    // Local | Param | Type | Function | Method | ArgName
    pub signature: Option<String>, // for methods/functions, the .rssi signature
    /// For a named argument, the effect keyword the call site must use, so the
    /// decoder emits `path: read ` not a bare value.
    pub required_effect: Option<&'static str>, // "read" | "mut" | "take"
}

pub fn valid_continuations(file: &str, partial_source: &str) -> Continuations;
```

### 4.3 Incremental acceptor (speed)

Re-lexing/parsing the whole prefix per token is wasteful. An incremental handle
keeps lexer + parse-frontier state and advances one RSScript token at a time —
the form a speculative decoder wants.

```rust
pub struct Generator { /* incremental lexer + parser frontier + scope */ }

impl Generator {
    pub fn new(file: &str) -> Self;
    /// Feed one accepted RSScript token; cheap, reuses prior state.
    pub fn push(&mut self, token: &str) -> PrefixStatus;
    /// Continuations from the current frontier without re-parsing the prefix.
    pub fn continuations(&self) -> Continuations;
    /// Speculative: would `token` be accepted? (no commit)
    pub fn peek(&self, token: &str) -> PrefixStatus;
    pub fn checkpoint(&self) -> GeneratorState; // for backtracking
    pub fn restore(&mut self, state: GeneratorState);
}
```

## 5. Worked example

Model is generating the body and has produced:

```
fn main() -> Result<Unit, FileError> {
    let path = Path.
```

`valid_continuations(...)` at the cursor after `Path.`:

```text
L1 says: an identifier (a method name) is expected.
L2 resolves the receiver `Path` to the core type Path, looks it up in
   core_interfaces(), and filters to Path's methods:

Continuations {
  keywords: [],
  symbols:  [],
  names: [
    { text: "from_string", kind: Method, signature: "Path.from_string(value: read String) -> fresh Path" },
    { text: "exists",       kind: Method, signature: "Path.exists(path: read Path) -> Bool" },
    ...
  ],
  may_stop: false,
}
```

`Path.nonexistent` is now unreachable. One token later, after
`Path.from_string(`:

```text
L2 knows from_string's signature -> the only legal next name is the parameter
   label `value`, and it must be followed by the effect keyword `read`:

names: [ { text: "value", kind: ArgName, required_effect: Some("read") } ]
```

So the decoder is steered into `Path.from_string(value: read ...)` — exactly the
two things LLMs get wrong in RSScript (named args + effect keywords) become
**impossible to get wrong.**

## 6. Reuse map — what exists vs. what is new

```text
LAYER  NEEDS                         REUSES (today)                       NEW WORK
L0     partial-token validity        lexer (lex_ident/number/string)      thin: "is this a prefix of a token?"
L1     expected tokens at frontier   syntax::parse_source (tolerant AST)  parser must report its expectation
                                                                          set at the truncation point
L2     in-scope names + types        symbol_index (locals/params/types/   receiver-type resolution for
                                     functions), core_interfaces(),       `Recv.` and arg-label/effect
                                     standard_package_interfaces(),       completion from callee signature
                                     analyze_source_with_core (types)
```

The one genuinely new piece is **L1's expectation set**: a hand-written
recursive-descent parser does not naturally report "what token did I expect at
EOF?" Two paths, in order of pragmatism:

- **MVP — generate-and-test.** The candidate set at any point is *finite and
  small*: the ~30 keywords, the symbol set, and the in-scope names from
  `symbol_index`. For each candidate, tentatively run the tolerant parse/analyze
  on `prefix + candidate` and keep those that do not introduce a *committed*
  error. No second grammar; reuses the checker wholesale. Cost is bounded by the
  candidate-set size and made cheap by §4.3 incrementality.
- **Optimization — generated grammar.** Emit a GBNF/CFG for the syntactic layer
  **from the same source of truth** that already generates the TextMate grammar
  (the lexer `KEYWORDS` table + parser productions). RSScript already generates
  `tmLanguage.json` from `KEYWORDS` with a freshness-guard test
  (`src/editor_grammar.rs`); a `src/decode_grammar.rs` would follow the exact
  same pattern, so L1 never drifts from the parser.

## 7. Why this fits RSScript specifically

- **Named args + effects** are the highest-error surface for a model with no RSS
  training data; L2 turns both into forced completions (the `value: read`
  example). No other constrained-decoding stack has the signature knowledge to do
  this — it needs *RSScript's own* checker.
- **One canonical form (§2.3)** means the legal continuation set is small and
  sharp — exactly the property §2B.1 calls a generation win.
- **Flat, top-level methods (Article IX)** mean receiver-type resolution for
  `Recv.method` is local and cheap — no `impl`-block scanning.
- The acceptor is the **same authority** as `rss check`: anything the decoder
  emits is guaranteed to pass the checker, collapsing the generate→check→repair
  loop into generate-only for syntactic and many semantic errors.

## 8. Phased plan

```text
P0  prefix_status + valid_continuations, keyword/symbol/L0 only (no semantics).
    Generate-and-test MVP. Wins: no syntax errors, ever.
P1  L2 names from symbol_index + core/standard interfaces; receiver-type and
    arg-label/effect completion. Wins: the `value: read` class of errors gone.
P2  Generator incremental handle (§4.3) for production decoding speed.
P3  Generated decode grammar (GBNF) from the KEYWORDS/parser source of truth,
    with a freshness-guard test mirroring the TextMate grammar generator.
P4  LLM-token adapter (logit mask / token healing) + a reference llama.cpp /
    vLLM integration.
```

## 9. Non-goals / risks

- **Not a substitute for review.** Constrained generation guarantees *well-formed
  and well-typed*, not *correct*. Article I still holds: review is the
  bottleneck; this only removes the noise floor of syntactic/type errors.
- **Semantic completeness is best-effort.** L2 prunes provable dead ends; it does
  not guarantee the *only* offered names lead to a valid whole program (some
  errors are non-local). Acceptable: the decoder still can't emit a checker
  error, it may just occasionally need to backtrack at a later token.
- **Latency.** Each step calls the checker; P2 incrementality and P3 grammar
  slicing are what make it production-viable. The MVP is for offline / batch
  generation and evaluation.
- **Token-boundary healing** (BPE vs RSScript tokens) lives entirely in the P4
  adapter and must be correct or the mask leaks invalid tokens.
```
