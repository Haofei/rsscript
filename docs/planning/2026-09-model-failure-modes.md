# How models actually fail at writing RSScript

*September 2026. Evidence for the generation-oracle design.*

## Why this exists

The generation oracle (`rss generate`, `rss check --json`, `rss fix --json`) is
the project's top priority, but until now it was being designed without data.
The eval corpus had ten repair/review fixtures and no model runner, so nobody
could say which mistakes a code-generating model actually makes, how often, or
whether the language card removes them.

This report is the first measurement. Twenty new from-scratch generation tasks
were added to `evals/tasks/` (each with a reference solution verified by `rss
check`), a runner was written (`tools/collect-model-samples.py`), and the whole
30-task corpus was sampled in three generation modes.

## Method, and what the numbers do not mean

- **Model**: `claude -p "<prompt>" --model sonnet --output-format text
  --allowedTools ""`. Exactly one sample per task per mode.
- **Isolation**: the CLI runs with no tools *and* from an empty temporary
  working directory. This matters. A first probe run with the CLI's cwd set to
  the repository returned something almost character-identical to the corpus's
  own reference solution, including string literals that appear nowhere in the
  prompt. The same task run from an empty directory produced an ordinary — and
  ordinarily wrong — first attempt. Every number below comes from the isolated
  configuration; the contaminated probe was discarded.
- **Prompt content**: `prompt_only` is the task prompt plus the task's `.rssi`
  interface text. `language_card` prefixes `docs/generated/language-card.md`
  and `AGENT.md`. `repair_loop` starts from the `language_card` prompt and then
  feeds `rss check --json` diagnostics back for up to three turns total.
- **Scale**: 127 model calls (30 + 30 + 67). Total wall time ≈ 2.5 h.
- **Scoring**: `cargo run -p rsscript-xtask -- agent-eval` per mode, reports at
  `evals/samples/sonnet/<mode>/report.v1.json`. "Compiles" means `rss check`
  reports no errors; "scorer pass" additionally requires the task's structural
  invariants.

**n = 1 per task per mode.** Thirty samples per mode is enough to rank failure
classes that occur in ten or more candidates and to tell 0/20 from 5/20. It is
not enough to trust a difference of one or two candidates, and no
single-candidate class below should be read as a rate. Two `repair_loop`
candidates needed a re-run after a 180 s timeout and a session limit truncated
their second turn; they were resampled from scratch with a 400 s timeout so
every candidate in the reported set had its full three-turn budget.

## Pass rates

| mode | compiles (30) | scorer pass (30) | on the 20 generation tasks | on the 10 original repair/review tasks |
|---|---|---|---|---|
| `prompt_only` | 9 (30%) | 8 (27%) | **0 / 20 compile** | 9 / 10 compile, 8 / 10 pass |
| `language_card` | 11 (37%) | 10 (33%) | **1 / 20 compile** | 10 / 10 compile, 9 / 10 pass |
| `repair_loop` | 15 (50%) | 13 (43%) | **5 / 20 compile** | 10 / 10 compile, 8 / 10 pass |

The headline is the split. The ten original tasks are repair/review: the model
is shown a nearly-correct program and asked to change one thing, and it nearly
always succeeds. The twenty new tasks ask for a whole small program, and the
model wrote **zero** compiling programs out of twenty on the first attempt with
the prompt alone. Every aggregate pass rate the corpus reported before today
was measuring the easy half.

## Every failed candidate, with its diagnostic codes

`t*N*` is the number of repair turns consumed. "invariant" means the candidate
compiled but lost a structural fact the task requires.

| task | `prompt_only` | `language_card` | `repair_loop` |
|---|---|---|---|
| async-stream-consume | RS0015, RS0026, RS0030, RS0206, RS0208, RS0311 | RS0015, RS0031, RS0202, RS0203, RS0204, RS0206 | **pass** t1 |
| csv-record-transform | RS0015, RS0206, RS0208 | RS0015, RS0026, RS0203, RS0204, RS0206, RS0307, RS0308 | RS0015, RS0202, RS0206 (t2) |
| destructive-symbol | pass | pass | pass |
| generic-struct-bound | RS0015, RS0202, RS0206, RS0208, RS0308 | RS0015, RS0026, RS0308 | RS0015, RS0026, RS0206, RS0308 (t2) |
| host-interface-only | RS0201, RS0204, RS0208 | **pass** | **pass** |
| json-config-validate | RS0015, RS0206, RS0208 | RS0015, RS0021, RS0206, RS0209 | RS0015, RS0025, RS0203, RS0204 (t2) |
| list-try-fold-validate | RS0015, RS0206, RS0208, RS1001 | RS0015, RS0201, RS0203, RS0204, RS0206, RS1001 | RS0206 (t2) |
| map-aggregate-counts | RS0015, RS0021, RS0206, RS0207, RS0209 | RS0015, RS0206, RS0501 | RS0206 (t2) |
| moved-value | pass | pass | pass |
| named-args | pass | pass | pass |
| nested-json-array-sum | RS0015, RS0206, RS0209 | RS0015, RS0021, RS0026, RS0206, RS0209, RS0311 | RS0209 (t2) |
| noescape-filter-callback | RS0015, RS0026, RS0206, RS0802 | RS0015, RS0201, RS0204, RS0206, RS0802 | RS0015, RS0206 (t2) |
| option-defaults-chain | RS0015, RS0202, RS0206 | RS0015, RS0206, RS0208, RS0209, RS1001 | RS0206 (t2) |
| option-result | pass | pass | pass |
| producer-consumer-bounded | RS0002, RS0015, RS0026, RS0029, RS0030, RS0206 | RS0206, RS0308, RS0401 | RS0202, RS0308 (t2) |
| protocol-dyn-dispatch | RS0015, RS0028, RS0201, RS0204, RS0206, RS0207, RS1001, RS1002, RS1301 | RS0015, RS0024, RS0206, RS1301 | RS0206, RS0207, RS1301 (t2) |
| read-mut-take | invariant | pass | pass |
| receiver-method | pass | invariant | invariant |
| resource-across-await | RS0015, RS0022, RS0201, RS0202, RS0204, RS0208 | RS0015, RS0208, RS0702 | **pass** t1 |
| resource-escape | pass | pass | pass |
| resource-scope-host | RS0015, RS0208 | RS0005, RS0015, RS0702 | **pass** t2 |
| resource-with | RS0015, RS0208 | pass | pass |
| result-error-mapping | RS0015, RS0021, RS0206, RS0208, RS0209 | RS0021, RS0206, RS0209, RS1001 | RS0206, RS1001 (t2) |
| retains-declaration | RS0015, RS0202, RS0206 | RS0015, RS0202, RS0206, RS0306 | RS0206 (t2) |
| retry-bounded | RS0201, RS0204, RS0206, RS0208 | RS0206 | **pass** t2 |
| sum-type-state-machine | RS0015, RS0206, RS0208 | RS0015, RS0026, RS0209 | RS0015, RS0026 (t2) |
| take-mut-chain | RS0015, RS0202, RS0208, RS0308 | RS0005, RS0015, RS0202, RS0308 | RS0308 (t2) |
| task-group-cancel | pass | pass | invariant t1 |
| task-group-select | RS0015 | RS0015, RS0026, RS0030, RS0202, RS0206, RS0208 | RS0207 (t2) |
| unknown-external | pass | pass | pass |

## Ranked failure classes

Counts are **candidates showing the class at least once**, out of 30 per mode.

| # | failure class | codes | `prompt_only` | `language_card` | `repair_loop` | concrete example |
|---|---|---|---|---|---|---|
| 1 | **Hallucinated syntax** | RS0015 | **19** | **16** | 5 | `language_card` take-mut-chain: `return Report { title: title, lines: [] }` — Rust/Swift struct-literal braces instead of `Report(title: …)` |
| 2 | **Invents a symbol not in scope** | RS0206 | **15** | **14** | **9** | `prompt_only` retains-declaration L26: `print(EventLog.size(log))` — `print` does not exist |
| 3 | **Return-type mismatch (implicit return)** | RS0208 | **13** | 3 | 0 | `prompt_only` csv-record-transform: `fn total_amount(…) -> Int` ends in a bare expression; "return in `total_amount` has type `Unit`, expected `Int`" |
| 4 | **Wrong call-site data effect** | RS0202, RS0308 | 5 | **7** | 4 | `prompt_only` take-mut-chain L10: `ReportSink.emit(report)` — "argument `report` … must use `take`" |
| 5 | **Omits argument labels** | RS0201, RS0204 | 4 | 4 | 1 | `prompt_only` resource-across-await L27: `ConfigStore.write_key(store, "last_page", page)` |
| 6 | **Branch/match used as a value** | RS0209 | 3 | 5 | 1 | `prompt_only` map-aggregate-counts L6: `let existing: Int = if counts.contains(level) { counts.get(level) } else { 0 }` |
| 7 | **Unknown binding** | RS0026 | 3 | 5 | 2 | `prompt_only` noescape-filter-callback L17: `let alerts: List<Alert> = List[…]` |
| 8 | **Invented argument label** | RS0203 | 0 | 3 | 1 | `language_card` list-try-fold-validate: `List.try_fold(list:, init:, op:)` — the parameters are `initial` and `folder` |
| 9 | **Non-exhaustive match** | RS0021 | 2 | 3 | 0 | `prompt_only` result-error-mapping L10: `match error {` missing a variant |
| 10 | **String `+` as concatenation** | RS1001 | 2 | 3 | 1 | `prompt_only` list-try-fold-validate L10: `Err("negative amount not allowed: " + raw)` |
| 11 | **Await outside async / awaits a non-async value** | RS0029, RS0030 | 3 | 1 | 0 | `prompt_only` producer-consumer-bounded L36: `await consumer_handle;` in a non-async `main` |
| 12 | **Wrong `self` / protocol-method convention** | RS0028, RS1301 | 1 | 1 | 1 | `prompt_only` protocol-dyn-dispatch L23: `fn format_json(self: read JsonFormatter)` — the name must be `JsonFormatter.format` |
| 13 | **Resource escapes its `with` scope** | RS0702 | 0 | 2 | 0 | `language_card` resource-scope-host L17: `return match ConfigStore.open(path: path) {` — a resource producer must be consumed by `with` |
| 14 | **Redeclares the interface inside the `.rss` file** | RS0005 | 0 | 2 | 0 | `language_card` take-mut-chain L6: `pub fn ReportSink.emit(report: take Report) -> Unit` pasted into the implementation file |
| 15 | **`noescape` callback escapes** | RS0802 | 1 | 1 | 0 | `prompt_only` noescape-filter-callback |
| 16 | **Local value live across `await`** | RS0031 | 0 | 1 | 1 | `language_card` async-stream-consume L6: `match await Stream.next(stream: mut stream)` |
| 17 | **Missing `retains`** | RS0501 | 0 | 1 | 0 | `language_card` map-aggregate-counts L15: `Map.insert(map: mut counts, key: level, …)` — "retaining API `Map.insert` cannot retain local value `level`" |
| 18 | **Use after move** | RS0401 | 0 | 1 | 0 | `language_card` producer-consumer-bounded L24: `let (sender, receiver) = Channel.split(channel: take channel)` |

### What "hallucinated syntax" actually is

RS0015 is the largest bucket, so it is worth splitting. Candidates showing each
construct:

| construct | `prompt_only` | `language_card` | `repair_loop` |
|---|---|---|---|
| Rust/Swift struct literal `T { field: v }` | 8 | 9 | 3 |
| expression-bodied match arm ending in `,` | 8 | 7 | 0 |
| Rust path syntax `::` (`List::new()`, `JobState::Queued`, turbofish) | 6 | 0 | 0 |
| `task_group` / `with` / `spawn` used as an expression | 3 | 3 | 0 |
| tuple destructuring in `for` (`for (k, v) in …`) | 1 | 1 | 0 |
| `mut x: T = …` as a binding form instead of `let mut` | 0 | 1 | 1 |
| invented `as Dyn<P>` cast | 1 | 0 | 0 |
| `mut` on the `with … as` binding | 1 | 0 | 0 |
| other | 7 | 4 | 2 |

Two constructs dominate and they are *both* surface syntax the model already
knows from other languages: brace struct literals and comma-terminated match
arms. The language card shows neither the constructor call form nor a match
statement.

### What "invents a symbol" actually is

| sub-class | `prompt_only` | `language_card` | `repair_loop` |
|---|---|---|---|
| a bare output/print builtin (`print`, `write`) | 10 | 6 | 3 |
| a core-namespace function that does not exist | 3 | 12 | 7 |
| a receiver method on a value that has no such method | 7 | 3 | 1 |
| a free function that does not exist | 4 | 0 | 0 |

Most-invented names across all 90 candidates: `print` (11), `write` (8),
`Int.parse` (8 — the real name is `String.parse_int`), `List.of` (3),
and a nine-strong `get_*` JSON-accessor family (`Json.get_string` /
`get_int` / `get_bool` / `get_field`, plus receiver spellings such as
`value.get_int()` — the real names are `Json.field_string` / `field_int` /
`field_bool` / `field`), `Channel.split`,
`Channel.take_receiver`, `Map.get_or`, `Map.entries`, `String.split_once`,
`String.find`, `Console.write_line`, `IO.println`, `List.fold_result`.

Note the shape of the shift between modes. `prompt_only` invents *bare
functions* (`print(x)`); `language_card` invents *plausible namespaced
functions* (`Int.parse`, `Json.get_string`). The card teaches the
`Namespace.function(label: value)` shape — which is real progress — but gives no
list of which functions exist, so the model produces well-formed calls to
functions that are not there. Only 5/30, 3/30 and 4/30 candidates used the real
`Output.write`.

## Did the language card fix anything?

Partly, and it is worth being precise about where.

**Clearly improved (the card says something and the model obeys it):**

- **Return-type mismatch: 13 → 3 candidates.** The card's canonical-call
  example uses explicit `return`, and the models copy it. This is the single
  largest win in the data.
- **Rust `::` path syntax: 6 → 0 candidates.** The card's example uses
  `Namespace.function(...)` and that is enough to kill `::` entirely.
- **`await` misuse: 3 → 1 candidate.**
- Compile rate 9/30 → 11/30; on generation tasks 0/20 → 1/20.

**Unchanged:** hallucinated syntax (19 → 16, and *brace struct literals went up*,
8 → 9), invented symbols (15 → 14), omitted argument labels (4 → 4).

**Made worse:** wrong call-site data effect (5 → 7) and invented argument labels
(0 → 3). Both are the same phenomenon: the card tells the model that `mut` and
`take` are explicit and that arguments are named, so the model starts writing
effects and labels everywhere — including where they do not belong
(`Stream.next(stream: mut stream)` when the parameter is `read`,
`List.try_fold(init:, op:)` when the parameters are `initial` and `folder`).
The card creates the *intent* to be explicit without supplying the *facts*
needed to be correct, and the result is confident, wrong specificity.

Two classes appear **only** with the card: `RS0005` (2 candidates pasted the
`.rssi` interface text into the implementation file) and `RS0702` (2 candidates).
The first is a direct artifact of AGENT.md discussing `.rssi` files without ever
saying that interfaces are supplied to the compiler, not copied into the program.

## What the repair loop fixed, and what it could not

Within `repair_loop`, 10 of the 30 candidates already compiled on turn 1.
Repair converted 5 more: **3 at turn 2** (async-stream-consume,
resource-across-await, task-group-cancel) and **2 at turn 3**
(resource-scope-host, retry-bounded), for 15/30 compiling at the end. On the
twenty generation tasks that is 5/20, against 1/20 for `language_card` and
0/20 for `prompt_only` — different draws, so read it as a level, not a delta.

Aggregating over every repairing candidate, by whether a class present at turn 1
was gone by the final turn:

| class | cleared by repair | persisted to the last turn |
|---|---|---|
| hallucinated syntax (RS0015) | **11** | 5 |
| wrong data effect (RS0202/RS0308) | **9** | 3 |
| omits argument labels (RS0201/RS0204) | **8** | 0 |
| invents a symbol (RS0206) | 6 | **9** |
| return-type mismatch (RS0208) | 4 | 0 |
| branch-as-value (RS0209) | 4 | 1 |
| non-exhaustive match (RS0021) | 4 | 0 |
| invalid assignment (RS0311) | 4 | 0 |
| local across await (RS0031) | 2 | 0 |
| invented argument label (RS0203) | 2 | 0 |
| unknown binding (RS0026) | 1 | 2 |
| protocol not satisfied (RS1301) | 0 | 1 |

The pattern is sharp and it is the most actionable fact in this report:

> **Diagnostics that name the correct edit get fixed. Diagnostics that only name
> the error do not.**

`RS0201`/`RS0204` ("argument must be named", fix: "Write the argument as `name:
value`") were cleared 8 times and persisted **zero** times. `RS0202` carries the
required effect in its message ("must use `take`") and was cleared 9 times.
`RS0206` says a call "does not resolve" and offers no alternative — it was
cleared 6 times and **persisted 9 times**, making it the *only* class that is
larger after three repair turns than any other. Six candidates were still
calling `print` or `Int.parse` on the third turn after being told twice that
those do not resolve; with nothing to substitute, the model re-invents.

Repair also **introduces** errors: 2 new wrong-effect errors, and one each of
invented field, invented label, omitted label, branch-as-value and argument-type
mismatch. Fixing one diagnostic by rewriting a region reliably breaks something
else, which is an argument for instance-level machine-applicable edits over
"here is the error, resend the file".

## Compiles but still wrong

Four candidates passed `rss check` and still failed the scorer, all by dropping
a canonical spelling the task requires:

- `read-mut-take` (`prompt_only`): lost `suffix: suffix`, having written the
  default `read` effect explicitly at the call site.
- `receiver-method` (`language_card`, `repair_loop`): replaced `buffer.inspect()`
  with the free-call form. The receiver-call shorthand survives review only if
  the model knows it is canonical.
- `task-group-cancel` (`repair_loop`): the repair turn removed
  `Task.cancellation_token()` to make the file compile.

A checker-only oracle cannot see these. Any "did the model write idiomatic
RSScript" claim needs the invariant layer, not just `rss check`.

## Implications for the generation oracle

Ranked by candidates removed, with the counts each rests on.

### 1. Ship the callable core-API surface, not a list of filenames — 15 / 14 / 9 candidates

`RS0206` is the largest class that survives everything: 15 candidates in
`prompt_only`, 14 with the card, still **9** after three repair turns, and the
only class that *dominates* the persisted column (9 persisted vs 6 cleared).
The card today says "35 platform-neutral core interface files" and links to
them; it contains **zero callable signatures**. Models fill that vacuum with
`print` (11), `write` (8), `Int.parse` (8) and a nine-call `get_*` JSON-accessor
family.

The oracle must answer *"what can I call?"* Concretely: `rss generate` should
inject the signature list for the namespaces in scope (at minimum `Output`,
`String`, `Int`, `List`, `Map`, `Json`, `Option`, `Result`), and `RS0206` must
carry a `did_you_mean` candidate list in `--json`. `String.parse_int` is a
one-edit distance from `Int.parse`; the compiler knows this and currently says
nothing.

### 2. Put the five canonical surface forms in the card, with examples — 19 / 16 candidates

RS0015 is the biggest first-attempt class and the card moved it only 19 → 16.
The card is not wrong; it is silent on the exact forms models get wrong. Five
lines of example would cover the measured distribution:

1. constructor call `T(field: value)`, **never** `T { field: value }` (8 + 9 + 3 candidates);
2. match arms are `Pattern => { … }` blocks, **not** `Pattern => expr,` (8 + 7);
3. `task_group`, `with` and `select` are **statements**, not expressions (3 + 3);
4. no tuple destructuring in `for` (1 + 1);
5. `let mut x = …`, never `mut x: T = …` (0 + 1 + 1).

The card already proves this works: its one `Namespace.function(...)` example
eliminated `::` syntax outright (6 → 0).

### 3. Make every diagnostic carry the replacement, not just the complaint — 8 vs 0, 9 vs 3

The repair data separates the two kinds of diagnostic cleanly. Classes whose
message names the edit were cleared and never persisted (`RS0201`/`RS0204`: 8
cleared, 0 persisted; `RS0202`: 9 cleared, 3 persisted). The class whose message
names only the failure persisted nine times. The oracle's value is therefore
mostly in `fixes[]`, not in `summary`. Any diagnostic reachable by generation
that currently has `applicability: manual` and no concrete replacement text is a
gap, and `RS0206` is the top of that list.

### 4. Answer "which effect does this parameter want?" at the call site — 5 / 7 / 4 candidates, and rising with the card

Wrong `mut`/`take` at the call site is the one class the language card made
*worse* (5 → 7), joined by invented argument labels (0 → 3). Telling a model
that effects and labels are explicit, without telling it which effect and which
label, converts silence into confident error. The oracle must expose the
resolved signature at the call site — parameter names in order, each with its
declared effect — so the model is choosing from facts rather than guessing.
`RS0202`'s message already does this well and its repair record proves it works;
the generation path needs the same information *before* the error.

### 5. Say that interfaces are supplied, and score the canonical spelling — 2 candidates + 4 invariant-only failures

Two `language_card` candidates pasted the `.rssi` text into the `.rss` file
(RS0005); AGENT.md discusses `.rssi` files without ever saying they are passed
to the compiler rather than copied. One sentence fixes this.

Separately, four candidates across the three modes compiled cleanly and still
failed, by dropping receiver-call shorthand, writing the default `read` at a
call site, or deleting `Task.cancellation_token()` during repair. `rss check`
cannot see any of these. If the oracle's contract is "generated RSScript is
idiomatic", it needs a canonical-spelling check — the `agent-eval` invariant
layer generalised into `rss check` as warnings — or the oracle will certify
programs that compile and that a human reviewer would send back.

## Reproducing

```bash
python3 tools/collect-model-samples.py --model sonnet --jobs 5 --timeout 400
for mode in prompt_only language_card repair_loop; do
  cargo run -p rsscript-xtask -- agent-eval \
    --tasks evals/tasks \
    --candidates "evals/samples/sonnet/$mode" \
    --output "evals/samples/sonnet/$mode/report.v1.json"
done
python3 tools/analyze-model-samples.py --model sonnet
python3 tools/analyze-model-samples.py --model sonnet --reuse --compare
```

Samples, per-turn transcripts and the three `report.v1.json` files are committed
under `evals/samples/sonnet/` (1.4 MB), so every count here can be re-derived
without model access. Re-running the collector will produce different samples:
the model is not deterministic and these are single draws.

## After syntax sugar and scorer change (2026-09-15)

The same 90 committed samples, re-scored with the parser that accepts brace
struct literals and comma-terminated match arms, and with the scorer that keeps
`status` on compiling plus structural invariants. **No new samples were
collected**: every candidate file is byte-identical to the one measured above,
so each difference below is caused by the tooling and by nothing else.

| mode | compiles (30) | scorer pass (30) | RS0015 candidates | RS0206 candidates | canonical spelling (30) |
|---|---|---|---|---|---|
| `prompt_only` | 9 → **9** | 8 → **9** | 19 → **16** | 15 → **16** | — → 4 |
| `language_card` | 11 → **11** | 10 → **11** | 16 → **10** | 14 → **15** | — → 5 |
| `repair_loop` | 15 → **15** | 13 → **14** | 5 → **3** | 9 → **9** | — → 7 |

Three findings, and the third is the uncomfortable one.

**The scorer change converted exactly the candidates it was meant to.** Pass
count rises by one in each mode, and the three are precisely the
canonical-spelling-only failures named in "Compiles but still wrong":
`read-mut-take` in `prompt_only` (wrote the default `read` at the call site) and
`receiver-method` in `language_card` and `repair_loop` (used the qualified call
instead of receiver-call shorthand). The fourth, `task-group-cancel` in
`repair_loop`, still fails, and should: the repair turn deleted
`Task.cancellation_token()`, which is a structural loss, not a spelling. Both
facts are now visible at once — that candidate reports `status: fail` with
`canonical_spelling.canonical: true`.

**The sugar removes the hallucinated-syntax error it was measured against.**
RS0015 falls from 40 candidate-appearances across the three modes to 29, an 11
candidate drop, and it is concentrated exactly where the construct breakdown
predicted: `language_card`, where brace literals were the single most common
construct, improves most (16 → 10).

**But not one additional candidate compiles.** Accepting the two forms does not
convert a single failing candidate into a passing one, because it *unmasks* the
errors underneath. A brace struct literal used to abort a region before the
checker reached it; now the region type-checks and reports what was always wrong
with it. Four of the eleven candidates that lost RS0015 gained a different code
in its place — `result-error-mapping` gained RS0201/RS0204/RS1001,
`json-config-validate` gained RS0203/RS0204, `protocol-dyn-dispatch` gained
RS0207 — and RS0206 *rises* slightly (38 → 40 across all 90) for the same
reason: `resource-scope-host` now parses far enough in two modes for its
invented calls to be resolved and rejected.

This is the honest reading of the sugar: it converts a syntax error into the
semantic error the candidate really had. That is a strict improvement for the
repair loop, since RS0201/RS0202/RS0204 are the classes the measurement shows
being cleared and never persisting, while RS0015 says only "unsupported
expression". It is *not* a first-attempt pass-rate win, and nothing here
supports claiming one. The pass-rate lever the data still points at is the one
RS0206 marks: telling the model which functions exist.

Re-derive these numbers without model access:

```bash
for mode in prompt_only language_card repair_loop; do
  cargo run -p rsscript-xtask -- agent-eval \
    --tasks evals/tasks \
    --candidates "evals/samples/sonnet/$mode" \
    --output "evals/samples/sonnet/$mode/report.2026-09-15.v1.json"
done
```

The three `report.2026-09-15.v1.json` files are committed beside the original
`report.v1.json`, which is left untouched so the tables earlier in this report
stay checkable against the scoring that produced them.

## Fresh samples after oracle changes (2026-09-15)

The previous section re-scored the *old* samples with the new tooling and found
that the syntax sugar unmasked semantic errors without compiling one extra
candidate. That left the oracle changes themselves — the canonical surface
forms, the 450-signature index, the `RS0206` did-you-mean — unmeasured, because
every sample predated them. This section is a fresh draw against the current
tree: 30 tasks x 3 modes, one sample each, collected with

```bash
python3 tools/collect-model-samples.py --model sonnet --jobs 5 --timeout 400 \
  --out evals/samples/sonnet-2026-09-15
```

`claude -p --model sonnet --output-format text --allowedTools ""` run from an
empty temporary directory, exactly as before. Both sample sets below are scored
by one `agent-eval` build and one `rss check` binary, so every difference is the
samples, not the tooling. These are different draws at n = 1 per task per mode;
a difference of one or two candidates is not a result.

Two things went wrong during collection and both matter for reading the numbers.
Three `prompt_only` replies were discarded by the runner because the model
fenced the program as ```` ```rust ```` or ```` ```rescript ````; the fence
regex accepted only `rsscript`/`rss`. All three were ordinary failing
candidates, so the defect had been quietly inflating pass rates, and the
September baseline was collected with the same defect. They were re-collected
after the fix. Separately, a model session limit interrupted `repair_loop`: five
tasks produced no candidate and were re-collected after the limit reset, and two
more (`result-error-mapping`, `retains-declaration`) lost their third repair
turn. Both had already failed turn 2 and are scored as failures, so they cost
the mode at most two conversions.

### Pass rates

| set / mode | compiles | scorer pass | generation compile | generation pass | repair/review pass | canonical spelling |
|---|---|---|---|---|---|---|
| September `prompt_only` | 9/30 | 9/30 | 0/20 | 0/20 | 9/10 | 4 |
| September `language_card` | 11/30 | 11/30 | 1/20 | 1/20 | 10/10 | 5 |
| September `repair_loop` | 15/30 | 14/30 | 5/20 | 5/20 | 9/10 | 7 |
| **fresh `prompt_only`** | **6/30** | **6/30** | **0/20** | **0/20** | 6/10 | 4 |
| **fresh `language_card`** | **11/30** | **11/30** | **3/20** | **3/20** | 8/10 | 5 |
| **fresh `repair_loop`** | **17/30** | **17/30** | **8/20** | **8/20** | 9/10 | 5 |

The one number that moved beyond plausible noise is the generation half of
`repair_loop`: **5/20 to 8/20**, with the whole mode at 17/30 against 14/30.
`language_card` went 1/20 to 3/20 on the generation tasks while staying at 11/30
overall, because it lost two of the ten easy repair/review tasks in this draw.
`prompt_only` went *down*, 9/30 to 6/30, entirely on the repair/review half
(9/10 to 6/10) — that half has no card and no diagnostics, so nothing in this
work could have affected it, and the fence fix restored three failing candidates
that the September run would also have dropped. Read `prompt_only` as draw
variance on a mode the changes cannot reach.

### Failure classes, September to fresh

Candidates showing the class at least once, out of 30 per mode.

| class | `prompt_only` | `language_card` | `repair_loop` |
|---|---|---|---|
| hallucinated syntax (RS0015) | 16 -> 18 | 10 -> **6** | 3 -> **0** |
| invents a symbol (RS0206) | 16 -> 17 | 15 -> 16 | 9 -> **6** |
| wrong call-site effect (RS0202/RS0308) | 6 -> 8 | 7 -> **8** | 4 -> **8** |
| — RS0202 alone | 6 -> 6 | 4 -> 5 | 3 -> 5 |
| — RS0308 alone | 2 -> 3 | 4 -> **8** | 3 -> **6** |
| omits argument labels (RS0201/RS0204) | 6 -> 6 | 5 -> 3 | 1 -> 4 |
| invented argument label (RS0203) | 0 -> 0 | 4 -> 3 | 1 -> 3 |
| return-type mismatch (RS0208) | 13 -> 12 | 3 -> 4 | 0 -> 0 |
| branch used as a value (RS0209) | 3 -> 4 | 6 -> 4 | 1 -> 2 |
| string `+` (RS1001) | 4 -> 5 | 4 -> **0** | 1 -> **0** |

**The canonical surface forms work.** RS0015 falls 10 -> 6 with the card and
3 -> 0 after repair, and the sub-class breakdown shows what is left:
no Rust `::` paths, no tuple destructuring in `for`, no invented `as Dyn<P>`
cast — only `task_group`-as-an-expression (2), `mut` on a `with ... as` binding
(1) and a four-candidate tail. RS1001 (`+` for string concatenation) went 4 -> 0
with the card too. In `prompt_only`, which never sees the card, RS0015 went the
other way (16 -> 18). That contrast is the cleanest evidence in this report that
the card's example table is doing the work rather than the model having changed.

**The signature index did not stop invention, but the did-you-mean made it
repairable.** As a first-attempt class RS0206 is flat — 15 -> 16 candidates with
the card, still the largest class there, and the most-invented names are the
same family as before (`print` 4, `List.of` 3, `List.length` 3, `Int.parse` 3,
`Io.println` 2, `List.size` 2, `Json.get_int` 2). Pointing at a 450-signature
index does not make a model read it. What changed is what happens next.

### Did the model follow the suggestions?

Restricting to `repair_loop` candidates that hit RS0206 on turn 1:

| | September (no did-you-mean) | fresh (did-you-mean) |
|---|---|---|
| candidates with RS0206 at turn 1 | 15 | 12 |
| RS0206 gone by the final turn | 6 | **10** |
| final source calls a name the fix would have named | 5 / 15 | **12 / 12** |

Every one of the twelve ends up calling `Output.write`, `String.parse_int`,
`List.new`, `List.try_fold`, `Json.field` or `Channel.receiver` — the exact
targets in the alias table — against five of fifteen before. The class-level
counts agree from the other side: across repair turns RS0206 was **cleared 10
times and persisted 2**, where the September run had it cleared 6 and persisted
9, making it the one class larger in the persisted column than any other. That
inversion is the single clearest oracle effect in the data.

The honest caveat is that those target names are also simply the correct API
names, so a candidate could reach them without being told. 5/15 -> 12/12
alongside 6-cleared/9-persisted -> 10-cleared/2-persisted is a large joint shift,
but it is 30 candidates and it is one draw.

A second caveat is instrumentation, and it is fixed rather than argued: this
run's transcripts recorded only a list of codes per turn, so "was a suggestion
shown and declined?" could only be answered indirectly. The collector now keeps
each turn's source (`turn<N>.rss`) and the exact diagnostic rows that turn was
shown, fix applicability and replacement text included, so the question is
directly answerable from the committed samples in future runs.

### What repair cleared, and what it could not

Comparing each repairing candidate's first and last turn:

| | cleared | persisted | introduced |
|---|---|---|---|
| RS0206 invents a symbol | **10** | 2 | 2 |
| RS0207 argument type mismatch | 6 | 2 | 1 |
| RS0015 hallucinated syntax | 6 | 0 | 0 |
| RS0308 `take` of a non-local | 4 | **4** | 1 |
| RS0202 wrong call-site effect | 4 | 1 | **3** |
| RS0204 omits a label | 4 | 0 | 2 |
| RS0203 invented label | 4 | 0 | 1 |
| RS0208 / RS0209 / RS1001 | 3 / 3 / 3 | 0 | 0 / 1 / 0 |

RS0206 has swapped places with the effect classes. The class that now dominates
the persisted column is **RS0308, `take` requires a local value** — and RS0202
is the class repair most often *introduces*.

### The class that now dominates: `take` without `local`

RS0308 is 34 instances across 17 candidate-appearances in the fresh set, more
than any other code, and it reads the same way every time:

```
`take` requires a local value.
  > let mut report: Report = build_report(title: take "Weekly Status")
  > finish_report(report: take report)
  > let batch: Batch<Reading> = make_batch(label: take label, items: take items)
  fixes: [('manual', 'Pass a local value with `take`, or use `read`/`mut` for managed values.')]
```

Every instance is `take` applied to a `let` binding or to a literal, and every
instance carries exactly one fix, which is `manual` and names no edit. By the
report's own rule — *diagnostics that name the correct edit get fixed;
diagnostics that only name the error do not* — this is precisely the shape that
persists, and it does.

The card was complicit in producing it. It says `mut` and `take` are written
explicitly at the call site, and its surface-forms table teaches `let mut total:
Int = 0` as *the* binding form. It never mentions `local`, which is the binding
form a `take` actually requires: the verified reference solutions spell it
`local title = "daily"` and then `build_report(title: take title)`. A model
following the card exactly writes `let title = ...` and is then told by RS0202
that the argument "must use `take`" — which walks it straight into RS0308. Two
diagnostics, each correct, compose into a trap.

`RS0308` itself is emitted from ownership analysis, outside the files this work
owns, so the edit-carrying fix it needs is left to that owner; the smallest
change available here is the card, and it now states the rule and shows both
spellings.

### What was implemented, and the count behind each

1. **A correct machine-applicable fix for RS0202** — 12 of the 32 RS0202
   instances in the fresh set are the "wrong effect written" shape
   (`uses `take` but the parameter is `read``), not the "effect missing" shape.
   For all 12, the old fix inserted the required keyword *in front of* the
   keyword already there, producing `take mut value` or `read mut "x"`, neither
   of which parses. `rss fix --write` was turning a type error into a syntax
   error. The fix now replaces a wrong keyword and deletes one in front of a
   `read` parameter, where omission is canonical.

2. **A did-you-mean for RS0203** — 28 RS0203 instances in the fresh set, and not
   one is a typo. They are other languages' names for the same slot: `text` for
   `value` (12), `separator` for `delimiter` (3), `a`/`b` for `left`/`right`
   (4), `pattern` for `needle`, `end` for `len`, `list` for `parts`. Edit
   distance reaches none of them; *position* reaches all of them, because the
   model orders the arguments correctly and only misnames them. When the call is
   otherwise a complete named call, the parameter in the argument's own position
   is now offered with a machine-applicable rename. This covers 28 of 28
   instances in the fresh set and 9 of 9 in the held-out September set, every
   one correctly, and each rename clears two errors, since a wrong label is
   charged as both RS0203 and RS0204. The repair data is what makes this the
   right target: RS0203/RS0204 appear at *turn 2* in ten of the nineteen
   repairing candidates — the did-you-mean fixes the callee name, and the model
   then invents labels for the function it has just been handed.

3. **Resolved parameter facts at the call cursor** — RS0202/RS0308 together are
   8 candidates in every mode, and the top class in `repair_loop`. `rss generate
   continuations` now returns, for each callable candidate, every parameter's
   name, type, the effect the call site must supply, whether that effect has to
   be written at all, and whether the label may be dropped (mirroring the call
   checker's own positional rule), so the call can be written from facts instead
   of from the card's instruction to be explicit.

4. **The card's `take`/`local` rule** — 34 RS0308 instances, above.

5. **Collector fixes** — the fence regex, the per-turn transcripts, and `--rss`
   to pin the checker for a whole run.

### Reproducing

```bash
python3 tools/collect-model-samples.py --model sonnet --jobs 5 --timeout 400 \
  --rss ./target/debug/rss --out evals/samples/sonnet-2026-09-15
for mode in prompt_only language_card repair_loop; do
  cargo run -p rsscript-xtask -- agent-eval --tasks evals/tasks \
    --candidates "evals/samples/sonnet-2026-09-15/$mode" \
    --output "evals/samples/sonnet-2026-09-15/$mode/report.v1.json"
done
python3 tools/analyze-model-samples.py --model sonnet-2026-09-15 --rss ./target/debug/rss
```

### Measured after the changes: `repair_loop` re-run (2026-09-15b)

The changes above are only worth what they measure, so `repair_loop` was run
once more against the tree that contains them — 30 tasks, up to three turns,
one draw — into `evals/samples/sonnet-2026-09-15b/repair_loop/`:

```bash
python3 tools/collect-model-samples.py --model sonnet --mode repair_loop \
  --jobs 5 --timeout 400 --rss ./target/debug/rss \
  --out evals/samples/sonnet-2026-09-15b
```

| `repair_loop` | compiles | scorer pass | on the 20 generation tasks | clean on turn 1 | converted by repair |
|---|---|---|---|---|---|
| September | 15/30 | 14/30 | 5/20 | 10 | 5 |
| fresh (15) | 17/30 | 17/30 | 8/20 | 11 | 6 |
| **post-change (15b)** | **20/30** | **20/30** | **11/20** | 11 | **9** |

**Every point of the gain is in the repair loop, not the first attempt.** Both
runs produce exactly 11 candidates that compile on turn 1; repair converts 6 in
the fresh run and 9 in the re-run. That is what should happen: four of the five
changes are diagnostic-side and cannot touch turn 1, and it means the difference
is not a better first draw.

Errors left in the final candidates, counted as instances:

| code | fresh (15) | post-change (15b) |
|---|---|---|
| RS0308 `take` of a non-local | 12 | **2** |
| RS0204 missing argument | 11 | **4** |
| RS0203 invented label | 10 | 6 |
| RS0206 invents a symbol | 13 | 9 |
| RS0202 wrong call-site effect | 6 | **1** |

RS0308 is the one the card change targeted and it is the one that moved most:
12 instances over 6 candidates down to 2 over 2, and across repair turns it goes
from **persisting 4 times** — the largest persisted class in the fresh run — to
persisting **zero**. `take-mut-chain` is the clean illustration: in the fresh run
it reported RS0308 on all three turns and failed; here it writes `local title =
"Daily Report"` and `local report = build_report(title: take title)` on turn 1,
exactly the spelling the new surface-form rows show. Seven of the thirty
candidates use a `local` binding, against three before.

RS0202 falls from 6 instances to 1, which is the fix-edit change: the wrong-
effect shape now carries an edit that replaces or deletes the keyword instead of
inserting a second one in front of it.

### Were the machine-applicable fixes actually taken?

This run's transcripts keep each turn's source and the exact rows that turn was
shown, so the question is now answered directly rather than inferred. Counting
every machine-applicable replacement offered on turn N and asking whether it
appears in the source the model produced for turn N+1:

| code | offered | present next turn |
|---|---|---|
| RS0206 rename callee | 27 | 24 |
| RS0203 rename argument label | 10 | **10** |
| RS0202 match data effect | 2 | **2** |
| **total** | **39** | **36** |

Thirty-six of thirty-nine. The three misses are all RS0206 and all in two
candidates (`Channel.sender`/`Channel.receiver` in producer-consumer-bounded,
`String.trim` in noescape-filter-callback) where the model restructured the
region rather than renaming inside it.

The causal chain the RS0203 work was aimed at is visible turn by turn in the
transcripts. `async-stream-consume`: turn 1 calls `Stream.fromList`, is told
"Did you mean `Stream.from_list`?"; turn 2 takes it and then invents the label
`list`, is told "Did you mean `items`?"; turn 3 is clean. Without the second
suggestion that candidate spends its last turn guessing, which is what it did in
the fresh run.

### What is left

Ten candidates still fail. The remaining errors are thinly spread — RS0206 9,
RS0203 6, RS0204 4, RS0209 3, RS0308 2, RS1301 2 — with no single class
dominating for the first time in this report's three measurements. RS0206 is
still the largest, and it now resists at the *ranking* level rather than at the
message level: only three of its nine surviving instances carry a
machine-applicable rename at all.

The six that do not are a concrete list, and two of them are ranking defects
rather than genuine dead ends. `List.length` gets nothing because the real name
is `List.len`, three edits away with a two-edit budget for a six-character name
— and `List.length` was invented three times in the fresh `language_card` set,
so it earns a place in the alias table. `Console.print` likewise: the table
carries `Console.write_line` for `Output.write` but not `Console.print`. The
other four (`Receiver.receive`, a chained `read Channel.receiver().unwrap`
twice) have no near-miss and nothing should be offered.

One mis-suggestion is also visible and worth recording rather than hiding:
`Json.as_array` is offered `Json.is_array`, one edit away and semantically a
different function — a predicate where an accessor was wanted. Edit distance
buys reach at the cost of occasional confident nonsense, which is the same
trade-off the positional rule for `RS0203` deliberately avoids by refusing to
guess when the shape does not settle it.

`RS0206`'s alias table lives in `crates/rsscript-semantics/src/symbols.rs`,
outside the files this work owns, so those two entries and the mis-rank are
recorded here for that owner. Extending an alias table is a treadmill in any
case; the durable form of the fix is getting the signature index in front of the
model before it writes the call, which is what the parameter facts in `rss
generate continuations` now carry and what no measurement here exercises,
because the collector prompts a model rather than driving a generation
session.

n = 1 per task, one draw, 30 candidates. 17/30 to 20/30 and 8/20 to 11/20 are
three-candidate moves in the direction the mechanism predicts, with a
class-level account of which three and why; they are not a rate.

## 50 tasks, two models (2026-09-19)

Every measurement above rests on one twenty-task generation set drawn before the
compiler gained callback parameters, closures with implicit captures, protocols
executing through `Dyn<P>`, list and variant patterns, index assignment,
`let ... else`, RS0016–RS0020, the RS0203/RS0206 did-you-mean, and the
`local`/`take` rows in the card. Twenty more generation tasks were added, shaped
like the work a host actually hands an agent, and the whole fifty-task corpus
was sampled twice: once with `sonnet` and once with `haiku`.

```bash
python3 tools/collect-model-samples.py --model sonnet --jobs 10 --timeout 400 \
  --rss <pinned rss> --out evals/samples/sonnet-2026-09-19
python3 tools/collect-model-samples.py --model haiku  --jobs 10 --timeout 400 \
  --rss <pinned rss> --out evals/samples/haiku-2026-09-19
for model in sonnet haiku; do for mode in prompt_only language_card repair_loop; do
  cargo run -p rsscript-xtask -- agent-eval --tasks evals/tasks \
    --candidates "evals/samples/$model-2026-09-19/$mode" \
    --output "evals/samples/$model-2026-09-19/$mode/report.v1.json"
done; done
```

Both CLI aliases were accepted as written; neither needed a substitute. 300
candidates, one draw per task per mode, `claude -p --output-format text
--allowedTools ""` from an empty temporary directory, all scored by one pinned
`rss` binary and one `agent-eval` build. A session limit interrupted the haiku
`repair_loop`: eleven tasks produced nothing and six lost a turn mid-loop. All
seventeen were deleted and re-collected after the limit reset, so every
committed candidate had its full three-turn budget and no transcript carries a
turn error.

**n = 1 per task per mode.** Fifty samples per mode ranks classes that occur in
ten or more candidates; it does not support reading a two-candidate difference
as a rate.

### Pass rates

`old20` is the September generation set, for comparison with its 0 / 1 / 5 and
3 / 8 / 11 of 20 history. `new20` is the set added today. `repair10` is the ten
original repair/review fixtures.

| model / mode | old20 | new20 | repair10 | overall | compiles | canonical spelling |
|---|---|---|---|---|---|---|
| sonnet `prompt_only` | **0/20** | **0/20** | 10/10 | 10/50 | 10/50 | 8 |
| sonnet `language_card` | **3/20** | **2/20** | 10/10 | 15/50 | 15/50 | 15 |
| sonnet `repair_loop` | **15/20** | **11/20** | 10/10 | 36/50 | 37/50 | 18 |
| haiku `prompt_only` | **0/20** | **0/20** | 8/10 | 8/50 | 8/50 | 6 |
| haiku `language_card` | **1/20** | **2/20** | 9/10 | 12/50 | 13/50 | 6 |
| haiku `repair_loop` | **7/20** | **8/20** | 9/10 | 24/50 | 25/50 | 7 |

Three things in that table.

**The old20 trend continues and is now large.** On the September set,
`repair_loop` has gone 5 → 8 → 11 → **15** of 20 across four measurements, each
after a round of oracle work, while `prompt_only` has stayed at 0/20 in every
one of them. Nothing in this project has ever moved the first attempt; every
gain has been in the repair loop, and this draw is the largest of them.

**The new twenty are slightly harder than the old twenty, and not differently
hard.** `new20` tracks `old20` within a few candidates in five of the six
mode-sets. The gap in sonnet `repair_loop` (15/20 against 11/20) is the largest
and it has a named cause below: three of the new tasks are the only ones in the
corpus that require a closure literal, and the card does not contain one.

**The model gap is entirely in the repair loop.** sonnet and haiku are within
two candidates of each other on the first attempt in both prompted modes
(10 vs 8, 15 vs 12). After three repair turns they are 36 vs 24. Both models
start in the same place; one of them uses the diagnostics and the other does
not, which is a capability difference and not a language one.

### Failure classes

Candidates showing the class at least once, out of 50 per cell.

| class | s `prompt_only` | s `language_card` | s `repair_loop` | h `prompt_only` | h `language_card` | h `repair_loop` |
|---|---|---|---|---|---|---|
| RS0206 invents a symbol | **29** | **28** | 3 | **34** | **31** | 9 |
| RS0015 hallucinated syntax | 18 | 8 | 3 | **28** | **23** | 5 |
| RS0201/RS0203/RS0204 labels | 16 | 9 | 4 | 13 | 17 | 7 |
| RS0208 return-type mismatch | 15 | 6 | 1 | **36** | 5 | 5 |
| RS0209 branch used as a value | 9 | 8 | 1 | 4 | 5 | 5 |
| RS0202/RS0308 call-site effect | 7 | 8 | 2 | 6 | 8 | 5 |
| RS1001 string `+` | 12 | **14** | 0 | 6 | 2 | 3 |
| RS0016–RS0020 | 0 | 0 | 0 | 0 | 0 | 0 |

**RS0016–RS0020 never fire.** Not once in 300 candidates. `let … else`
(RS0020), loop control outside a loop (RS0016), read-before-assign (RS0017),
unresolved import (RS0018) and private-use (RS0019) are all checks nothing in
this corpus reaches — including the task written specifically to require
`let … else`, whose failures are RS0206 and RS0203 instead. These five codes
are not part of the generation failure surface and should not be budgeted for.

Classes present now that no earlier measurement recorded, as
candidate-appearances over all 300: RS0034 binding type cannot be inferred (5),
RS0601 `fresh` return not clean (4), RS0210 operator type mismatch (4), RS0301
managed-to-local (3), **RS1004 surface reference attempt (3)**, and one each of
RS0313, RS0037 and RS0706. RS1004 is the only interesting one: `&T` / `&mut T`
written by haiku, which is the Rust spelling of the thing RSScript spells with
`read`/`mut`, and it belongs to the same family as the `::` paths the card
already removed.

### Did the model take the fixes it was offered?

Counting every machine-applicable replacement offered on turn N and asking
whether that exact text appears in the source the model produced for turn N+1:

| code | sonnet offered | sonnet present next turn | haiku offered | haiku present next turn |
|---|---|---|---|---|
| RS0206 rename callee | 63 | 55 | 72 | 53 |
| RS0203 rename argument label | 54 | **54** | 67 | 63 |
| RS0202 match data effect | 12 | **12** | 15 | **15** |
| **total** | **129** | **121 (94%)** | **154** | **131 (85%)** |

Both models take almost everything the compiler hands them, and the nine-point
gap between them is entirely RS0206: sonnet applies 87% of callee renames,
haiku 74%. This is the cleanest statement of the difference between the two
models in the whole report — the weaker model is not worse at reading the
suggestion, it is worse at not rewriting the surrounding region while it does.

Per-class repair outcomes agree. Comparing each repairing candidate's first and
last turn (sonnet n = 34, haiku n = 39), the classes with a machine-applicable
fix behave as the earlier reports predicted:

| code | sonnet cleared / persisted / introduced | haiku cleared / persisted / introduced |
|---|---|---|
| RS0206 | 24 / 3 / 0 | 22 / 9 / 0 |
| RS0203 | 8 / 0 / 3 | 15 / 4 / 0 |
| RS0204 | 8 / 0 / 3 | 12 / 4 / 2 |
| RS0015 | 6 / 3 / 0 | 19 / 4 / 1 |
| RS1001 | 12 / 0 / 0 | 2 / 1 / 2 |
| RS0207 | 6 / 0 / 1 | 1 / 1 / **5** |
| RS0202/RS0308 | 7 / 1 / 1 | 5 / 2 / 3 |

RS0207 (argument type mismatch) is the one class haiku *introduces* more often
than it clears, five times against one. Every instance is the same shape:
`Output.write(message: count)` where `count` is an `Int`. RS0207 names the
expected type and offers no edit, and the repair loop spends turns re-deriving
`String.from_int`.

### Which task shapes fail for both models

Thirteen of the fifty tasks pass in none of the three modes for either model.
The final `repair_loop` diagnostic of each is the most informative view:

| task | sonnet final | haiku final |
|---|---|---|
| closure-capture-local | RS0015, RS0206 | RS0206 |
| noescape-filter-callback | RS0015 | RS0207 |
| protocol-dyn-dispatch | RS0028, RS0201, RS0206, RS1301 | RS0201, RS0204, RS1301 |
| protocol-select-impl | RS0201, RS0203, RS0204 | RS0208, RS1301 |
| retains-declaration | RS0207 | RS0207 |
| json-array-field-total | RS0209 | RS0206, RS0208 |
| nested-json-array-sum | RS0206 | RS0206, RS0207, RS0209, RS1001 |
| option-chain-defaults | invariant (`Option.unwrap_or`) | RS0206 |
| result-question-mapping | RS0203, RS0204 | RS0209 |
| tuple-destructure-caller | RS0203, RS0204 | RS0207 |
| select-deadline-cancel | RS0308 | RS0015, RS0024, RS0202, RS0206, RS0208 |
| task-group-select | RS0208 | RS0308 |
| telemetry-cancel-pipeline | RS0202 | RS0202, RS1001 |

These are **language and library problems, not capability problems**, and they
group into four shapes:

1. **Closures** (closure-capture-local, noescape-filter-callback,
   callback-helper-twice). Both models write `fn(x: T) -> U { ... }` where
   RSScript wants `|x| { ... }`, and then RS0206 on the call because the binding
   never became a closure. The card contains no closure literal anywhere. This
   is the only RS0015 sub-class that survives sonnet's repair loop — three
   candidates, all three of them this.
2. **Protocols** (protocol-dyn-dispatch, protocol-select-impl). RS1301 in both
   models in both tasks after three turns. The card says nothing about
   `protocol`, `impl P for T`, or `Dyn.from<P, T>`.
3. **Structured concurrency** (select-deadline-cancel, task-group-select,
   telemetry-cancel-pipeline). The card's one row says `task_group`, `with` and
   `select` are statements; it does not show a `select` arm, an `async let`, or
   which channel operation takes a `take`.
4. **JSON and `Option`** (json-array-field-total, nested-json-array-sum,
   option-chain-defaults). RS0206 on invented accessor names, again.

The complement is short and tells the other half. Fourteen tasks pass for
exactly one model, and thirteen of those are sonnet-only: async-stream-consume,
csv-record-transform, json-config-validate, let-else-early-return,
list-index-assign, list-try-fold-validate, option-defaults-chain,
producer-consumer-bounded, resource-across-await, result-error-mapping,
session-with-cleanup, string-builder-report and task-group-cancel. Every one of
those is reachable, and the thing sonnet does that haiku does not is finish the
repair loop without rewriting a working region. Only callback-helper-twice goes
the other way, and only in one mode.

### The constructs, counted directly

Scanning the candidate sources rather than the diagnostics, since one bad
construct can be charged to several codes:

| construct | s `p_o` | s `l_c` | s `r_l` | h `p_o` | h `l_c` | h `r_l` |
|---|---|---|---|---|---|---|
| string `+` / `++` concatenation | **18** | **14** | 1 | 10 | 3 | 1 |
| labelled or called variant: `Ok(value:`, `None()`, `Ok(())` | 3 | 0 | 0 | 4 | **11** | 1 |
| Rust `::` path | 4 | 0 | 0 | **11** | 0 | 0 |
| `with X = producer {` instead of `with producer as X {` | 3 | 3 | 0 | 3 | 3 | 0 |
| closure `fn(x: T) -> U { }` instead of `|x| { }` | 1 | 3 | **3** | 2 | 3 | 0 |
| `var` binding | 4 | 0 | 0 | 4 | 0 | 0 |

The rows the card already covers go to zero with the card in both models —
`::`, `var`, brace struct literals. The rows it does not cover do not. That is
the same result the September card change produced and it is the reason the
implications below are ranked the way they are.

### Implications, ranked by count

1. **Put the callable signatures in the prompt, not a link to them — 29 / 28 /
   34 / 31 candidates.** RS0206 is the largest class in all four prompted cells
   and is the only class that is large in *both* models. The card links a
   450-signature index; pointing at an index does not make a model read it. The
   smallest change that would address it is `rss generate` injecting the
   signature list for the namespaces a task's prompt mentions. **This is
   tooling, not language design**, and it is the same recommendation the
   September report made and nothing has yet implemented.
2. **Show a closure literal in the card — 9 candidates, and 3 of 3 of what
   survives sonnet's repair loop.** Every RS0015 sonnet still has after three
   turns is `fn(x: T) -> U { }` for `|x| { }`. One surface-form row. Tooling.
3. **Say that strings are joined by a call — 26 candidates, 16 of them with the
   card.** RS1001 is the largest card-addressable class by count and the card is
   silent on it. One surface-form row; **implemented below, with a measurement.**
   Tooling.
4. **Give RS0207 an edit — 5 introduced by haiku's repair loop against 1
   cleared.** Every instance is an `Int` passed where a `String` is wanted, and
   `String.from_int(value: x)` is mechanical. RS0207's fix is `manual` and names
   no replacement, which by this report's own rule is the shape that persists.
   **Tooling, but it lives in the checker**, which this work does not own.
5. **Show the `with producer as name { }` binding form — 12 candidates, 6 with
   the card.** It is exactly the three `with`-resource tasks in five of the six
   sets: when a task needs `with`, roughly half the candidates write
   `with name = producer { }`. One surface-form row. Tooling.
6. **Show `protocol` / `impl` / `Dyn.from` — 4 candidates, but 2 of the 13
   both-fail tasks.** Low count, high concentration: the protocol tasks fail for
   both models in every mode. Tooling, but a larger card change than one row.
7. **Decide whether `Ok(value: x)` and `None()` should parse — 15 candidates,
   11 of them haiku with the card.** Models write variant constructors with a
   field label or with empty parens. The parser rejects both and `rss fmt`
   normalises neither. **This one is a language-design decision, not tooling**:
   either the parser accepts them as surface sugar the way it already accepts
   brace struct literals and comma-terminated match arms, or the card names them
   as a wrong form. The sugar precedent says accepting them converts a syntax
   error into the semantic error underneath, which the 2026-09-15 section showed
   is a strict improvement for the repair loop and not a first-attempt win.
8. **Decide whether `+` on two strings should mean concatenation — 26
   candidates.** Item 3 is the documentation half of this; the other half is
   that RS1001 exists at all. Operator overloading is refused by design, so this
   is a **language-design decision** and it is already settled in the negative;
   it is listed here only because the count is large enough that it will keep
   appearing.
9. **RS0016–RS0020 need no work — 0 candidates.** Recorded so the next round
   does not budget for them.

### What was changed here, and what it measured

Only one change, the highest-count one that is a wording change rather than a
compiler change: **the canonical surface forms table now has a string
concatenation row** (`String.concat(left: head, right: tail)`, not
`head + tail`). The card had rows for every other construct models import from
a neighbouring language and none for this one.

`language_card` was re-collected for sonnet against the new card, one draw,
same fifty tasks, same pinned checker:

```bash
python3 tools/collect-model-samples.py --model sonnet --mode language_card \
  --jobs 10 --timeout 400 --rss <pinned rss> \
  --out evals/samples/sonnet-2026-09-19b
```

| sonnet `language_card` | RS1001 candidates | RS1001 instances | candidates writing `+` between strings | scorer pass | compiles |
|---|---|---|---|---|---|
| before, with the seven-row table | 14 | 14 | 14 | 15/50 | 15/50 |
| after, with the string row | **2** | **2** | **0** | 16/50 | 16/50 |

The construct the row names is gone: no candidate in the new draw concatenates
two strings with `+`. The two remaining RS1001 are a different error wearing the
same code — `total = total + reading` where `reading` is an unhandled
`Result<Int, JsonError>`, which is a missing `?`, not a string. Measured on the
construct the change targets, this is 14 → 0.

The pass rate moves 15/50 to 16/50, which is one candidate at n = 1 and is not
a result. That is the expected shape and matches every earlier card change in
this report: naming a form removes the form, and removing a syntax error
unmasks the semantic error beneath it rather than converting a failure into a
pass. RS0203/RS0204 rise from 8/9 to 10/10 in the new draw for exactly that
reason — regions that used to abort on RS1001 now get far enough to have their
argument labels checked.

The honest caveat is that these are two different draws of a non-deterministic
model at n = 1 per task. 14 → 0 on a single named construct, in the mode whose
prompt is the only thing that changed, is a large enough move to attribute; the
one-candidate pass difference is not.

### Reproducing

```bash
for model in sonnet haiku; do for mode in prompt_only language_card repair_loop; do
  cargo run -p rsscript-xtask -- agent-eval --tasks evals/tasks \
    --candidates "evals/samples/$model-2026-09-19/$mode" \
    --output "evals/samples/$model-2026-09-19/$mode/report.v1.json"
done; done
cargo run -p rsscript-xtask -- agent-eval --tasks evals/tasks \
  --candidates evals/samples/sonnet-2026-09-19b/language_card \
  --output evals/samples/sonnet-2026-09-19b/language_card/report.v1.json
python3 tools/analyze-model-samples.py --model sonnet-2026-09-19 --rss ./target/debug/rss
python3 tools/analyze-model-samples.py --model haiku-2026-09-19  --rss ./target/debug/rss
```

Candidates, per-turn sources and per-turn transcripts for all 350 samples are
committed, so every count above is re-derivable without model access. The
`analysis.json` files the analyzer writes are not committed: they are pure
derivation and 2.2 MB of it.
