# RSScript AI Generation Feedback — Design Specification v0.1 Final

**Status:** v0.1 final / implementation-planning specification  
**Scope:** agent-facing constrained generation, fast checked interpretation, sandboxed behavioral feedback, backend parity, and review-first integration for RSScript.

This document defines the RSScript-specific feedback loop for AI-generated code. It combines constrained generation and fast interpretation into one design for making generated RSScript faster to produce, faster to validate, and easier to review.

The design has two main components:

1. A generation oracle that exposes parser, checker, type, effect, signature, and project-symbol knowledge over incomplete RSScript programs.
2. An interpreter that executes checked RSScript directly for fast behavioral feedback before final backend validation.

The goal is not to make RSScript implicit, magical, or autocomplete-driven. The goal is to make correct, explicit RSScript cheap to generate while preserving the existing review-first model.

---

## 1. Why

RSScript has almost no model pretraining prior. A prompt-sized guide can teach the language, but it cannot reliably prevent common generated-code failures:

- missing named arguments
- missing `read` / `mut` / `take`
- invalid receiver methods
- unbound names
- wrong error type after `?`
- slow behavioral feedback because `rss run` pays the Rust compiler cost

The intended loop is:

```text
AGENT.md                 raises the model's prior
generation oracle         prevents many invalid prefixes while code is emitted
rss check                 verifies type/effect contracts
rss eval / interpreter    gives ms-level behavioral feedback
rss run / Rust backend    remains the final execution authority
review / REIR             remains the product boundary
```

This is not autocomplete for humans. It is an agent-facing compiler oracle and a fast runtime probe. The goal is to remove low-value generate/check/repair cycles without weakening RSScript's explicitness or review boundary.

---

## 2. Design Principles

RSScript should make correct code cheap to generate without making behavior implicit.

```text
Prompting teaches.
Constrained generation steers.
Checking proves the contract.
The interpreter probes behavior quickly.
The Rust backend defines final behavior.
Review remains the bottleneck.
```

Constrained generation must not become hidden inference. It may offer legal next tokens, names, parameter labels, variants, expected types, and required effects, but the emitted source is still ordinary RSScript. The source remains reviewable without knowing the generation machinery.

The interpreter must not become a second semantics. It executes a supported subset of checked HIR for fast feedback and is continuously checked against the lowered Rust backend.

---

## 3. System Invariants

The following invariants are non-negotiable:

1. The oracle never emits source transformations invisible in RSScript source.
2. `Dead` must be sound; `Incomplete` may be conservative.
3. `rss_eval` only executes checked HIR.
4. Agent-facing eval never uses `RealHost` by default.
5. Unsupported interpreter constructs fail closed with diagnostics.
6. The Rust backend is the semantic authority.
7. Generated artifacts have freshness guards against parser/runtime ABI drift.
8. Review/REIR remains the product boundary; generation and eval only reduce noise.
9. Interpreter results are probes, not final correctness guarantees.
10. Real host execution is always explicit and policy-controlled.
11. Budget exhaustion, partial project context, or incomplete semantic domains degrade to `Incomplete`, not unsound `Dead`.

---

## 4. Two Pieces, One Loop

### 4.1 Generation Oracle

The oracle exposes the checker over an incomplete program. It answers two questions:

```text
Can this prefix still become valid RSScript?
What RSScript tokens/names are legal next?
```

It works at the RSScript-token frontier, not directly at the model's BPE token level. A model adapter can later turn legal RSScript continuations into a logit mask, but the first useful version is an agent tool.

The oracle should understand:

- lexical prefixes
- parser states
- incomplete expressions
- incomplete call sites
- in-scope locals and parameters
- imports and package symbols
- core functions and types
- receiver types
- method signatures
- named arguments
- required effects
- expected expression types
- sum variants for known scrutinee types

### 4.2 Interpreter

The interpreter executes checked RSScript without lowering to a temporary Rust package and invoking `cargo run`.

```text
rss check   type/effect feedback in ms
rss eval    behavioral feedback in ms
rss run     backend-authoritative execution in seconds
```

It is a tree-walker over checked HIR/user code and dispatches built-in calls through the same runtime crate used by lowered Rust.

The interpreter exists to accelerate feedback. It does not replace the checker, the Rust backend, or review.

---

## 5. Generation Oracle API

Public module:

```rust
pub mod generate;
```

The oracle must run against a loaded compiler/project context, not against an isolated text buffer. Prefix validity, semantic continuations, and `Dead` soundness depend on the same package, import, core, and symbol visibility rules used by `rss check`.

```rust
#[derive(Debug)]
pub struct GenerateContext<'a> {
    pub file: &'a str,
    pub partial_source: &'a str,
    pub project: &'a ProjectContext,
    pub symbol_completeness: SymbolCompleteness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolCompleteness {
    Complete,
    Partial,
}

#[derive(Debug, Clone)]
pub struct ContinuationOptions {
    pub max_returned_names: usize,
    pub max_probe_candidates: usize,
    pub include_snippets: bool,
}

pub fn prefix_status(ctx: &GenerateContext<'_>) -> PrefixStatus;

pub fn valid_continuations(
    ctx: &GenerateContext<'_>,
    options: ContinuationOptions,
) -> Continuations;
```

Convenience wrappers are acceptable only inside a fully loaded compiler session:

```rust
pub fn prefix_status_in_session(
    session: &CompilerSession,
    file: &str,
    partial_source: &str,
) -> PrefixStatus;

pub fn valid_continuations_in_session(
    session: &CompilerSession,
    file: &str,
    partial_source: &str,
    options: ContinuationOptions,
) -> Continuations;
```

The `file` argument identifies a file inside that session. It must not imply that `Dead` can be proven from a single standalone text buffer.

### 5.1 Prefix Status

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixStatus {
    Complete,
    Incomplete,
    Dead { at: Span, reason: String },
}
```

Meaning:

- `Complete`: the whole top-level RSScript program is complete and passes the supported checks.
- `Incomplete`: the source is truncated, the local construct is unfinished, or the oracle cannot soundly prove that the prefix is dead.
- `Dead`: the source has committed to something invalid that cannot be fixed by appending more tokens.

`Complete` is program-level. It does not mean “this local expression may stop.” A prefix that is syntactically valid at a local grammar frontier but still inside an unfinished function body, block, call, match, or file is still `Incomplete`.

`Continuations.may_stop` is local. It means the current syntactic slot may be ended, not that the whole file is complete.

The first version should be conservative. Treat ambiguous truncation as `Incomplete`; only return `Dead` for clear committed errors such as unknown callee, unknown argument, missing required effect after a completed argument, or receiver method that cannot exist.

### 5.2 Dead Soundness and Symbol Completeness

`Dead` is allowed to under-report; it must never over-report.

```text
If PrefixStatus::Dead is returned, no valid RSScript token stream appended to
this exact prefix can make the program valid in the current project context.
```

Deadness is context-sensitive. The oracle may return `Dead` for semantic errors only when the relevant semantic domain is complete.

Examples of domains that must be complete before returning `Dead`:

```text
unknown callee:
  all visible locals, params, imports, package symbols, and core symbols

unknown receiver method:
  receiver type and all visible methods/extensions for that type

unknown argument label:
  resolved callee signature and all remaining parameter labels

unknown variant:
  resolved scrutinee type and all variants for that type

wrong required effect:
  resolved parameter signature and committed argument expression
```

If the project context, import graph, package index, core index, receiver type, or callee signature is incomplete, the oracle must return `Incomplete` rather than `Dead`.

A trailing partial token keeps the prefix `Incomplete` only if it is a prefix of at least one legal token in the current lexical, syntactic, or semantic domain. If it is not a prefix of any legal token, and appending further token text cannot make it legal, `Dead` is sound.

Examples:

```rss
Path.exi
```

This is `Incomplete` if `exists` is a valid method on `Path`.

```rss
File.read_bytes(pa
```

This is `Incomplete` if `path` is a valid remaining argument label.

```rss
Path.nonexistent
```

This may be `Dead` only if receiver type resolution is complete and no valid method for that receiver has `nonexistent` as a prefix.

```rss
File.read_bytes(path: "x")
```

This may be `Dead` only if the callee signature is resolved and the argument expression has committed without the required effect.

```rss
unknown_fn(
```

This may be `Dead` only if visible symbol resolution is complete.

If symbol completeness is `Partial`, unknown-name diagnostics may still be reported as ordinary diagnostics, but `prefix_status` must degrade to `Incomplete`.

### 5.3 Continuations

```rust
#[derive(Debug, Clone)]
pub struct Continuations {
    pub keywords: Vec<&'static str>,
    pub symbols: Vec<&'static str>,
    pub literals: Vec<LiteralClass>,
    pub names: Vec<Completion>,
    pub expected_type: Option<ExpectedType>,
    pub may_stop: bool,
    pub truncated: bool,
    pub total_name_candidates: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub insert_text: String,
    pub replace: TextRange,
    pub kind: CompletionKind,
    pub signature: Option<String>,
    pub required_effect: Option<Effect>,
    pub result_type: Option<TypeRef>,
    pub commit: CommitBehavior,
}

#[derive(Debug, Clone)]
pub struct ExpectedType {
    pub display: String,
    pub type_id: Option<TypeId>,
    pub accepts: Vec<TypeId>,
}

#[derive(Debug, Clone)]
pub struct TypeRef {
    pub display: String,
    pub type_id: Option<TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeId {
    pub canonical: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitBehavior {
    InsertName,
    ReplacePartialToken,
    InsertArgumentWithEffect,
    InsertSnippet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    Local,
    Param,
    Type,
    Function,
    Method,
    ArgName,
    Variant,
}
```

The high-value completions are semantic, not UI-oriented:

- after `Path.`: only valid `Path` methods
- after `path.`: methods for the inferred receiver type
- after `File.read_bytes(`: only remaining parameter names
- after `path:`: the required effect, usually `read`
- in expression position: in-scope locals/params and valid literals
- after `match value {`: valid variants for the scrutinee type where known

This should be exposed as an agent tool named something like `rss_generate`, not as human autocomplete. Human-facing completion is explicitly not a goal.

`Continuations.expected_type` describes what the current expression context wants.

`Completion.result_type` describes what a candidate produces.

`may_stop` is a local grammar property. It does not imply `PrefixStatus::Complete`.

`truncated` means the returned candidate list is intentionally capped by `ContinuationOptions`. Truncation is allowed for usability and performance, but truncated candidate enumeration must never be used to prove `Dead`.

`total_name_candidates` is best-effort metadata. It may be omitted when computing the full count would require expensive exhaustive enumeration.

### 5.4 Completion Fields

`text` and `insert_text` are intentionally separate.

```rss
Path.from_
```

A method completion may return:

```text
text=from_string
insert_text=from_string
replace=<range of from_>
commit=ReplacePartialToken
```

The applied result is:

```rss
Path.from_string
```

After:

```rss
Path.from_string(
```

An argument completion may return:

```text
text=value
insert_text="value: read "
replace=<empty range at cursor>
commit=InsertArgumentWithEffect
```

The applied result is:

```rss
Path.from_string(value: read 
```

For expression-position candidates, `required_effect` means the effect that must be applied at the call site or argument site. It does not mean the returned value itself carries that effect at runtime.

### 5.5 Continuation Application Contract

To apply a completion, replace:

```text
partial_source[completion.replace.start..completion.replace.end]
```

with:

```text
completion.insert_text
```

The resulting source must be either a valid prefix or a complete checked program according to `prefix_status`.

Agents must not append `insert_text` manually when `replace` is non-empty. The edit range is authoritative.

The oracle should prefer source edits that keep the emitted program ordinary RSScript. Snippet-like completions are allowed only when the inserted source is explicit and reviewable.

### 5.6 Incremental Handle

```rust
pub struct Generator {
    // lexer/parser/checker frontier
}

impl Generator {
    pub fn new(session: Arc<CompilerSession>, file: FileId, initial_source: String) -> Self;
    pub fn push(&mut self, token: &str) -> PrefixStatus;
    pub fn peek(&self, token: &str) -> PrefixStatus;
    pub fn continuations(&self, options: ContinuationOptions) -> Continuations;
    pub fn checkpoint(&self) -> GeneratorState;
    pub fn restore(&mut self, state: GeneratorState);
}
```

The stateless API is authoritative for the early implementation. The `Generator` handle is the production shape for speculative decoding, checkpoint/restore, and backtracking.

`Generator` owns the mutable source prefix and frontier state. It keeps a shared compiler session plus a stable file identity instead of storing a borrowed `GenerateContext`; otherwise checkpoint/restore and long-lived speculative decoding would have unclear ownership.

P0/P1 may reparse prefixes. P5 should keep an incremental frontier.

### 5.7 Oracle Performance Contract

The P1 oracle must not implement continuations as “try every visible symbol through a full parser/checker pass.”

P1 may use generate-and-test, but it must be bounded and context-pruned.

Candidate construction must be staged:

```text
1. Determine syntactic slot.
2. Determine semantic domain.
3. Apply partial-token prefix filter.
4. Apply expected-type filter where available.
5. Apply receiver/callee/signature filter where available.
6. Rank or bucket candidates.
7. Probe only the bounded candidate set.
```

The oracle should avoid adding irrelevant global names to the probe set. Examples:

```text
after receiver dot:
  only methods valid for the resolved receiver type

inside call argument label position:
  only remaining argument labels for the resolved callee

after an argument label requiring an effect:
  only legal effect keywords and expressions compatible with the parameter type

in type position:
  types only

in expression position:
  locals, params, literals, variants, constructors, and functions compatible
  with the expected type where known
```

P1 must define a probe budget:

```text
max_returned_names:
  maximum number of name completions returned to the agent

max_probe_candidates:
  maximum number of candidate edits that may be parser/checker-probed
```

If the semantic domain exceeds the budget, the oracle should:

```text
return the best bounded continuation set
set truncated = true
set total_name_candidates where cheap to compute
avoid using the truncated set to prove Dead
return Incomplete rather than Dead when exhaustive proof is required
```

Generate-and-test is therefore a filtering and validation strategy, not an unbounded `N candidates × full reparse` algorithm.

P1 should cache the following per prefix or parser frontier:

```text
lexed token frontier
parser recovery state
scope lookup result
receiver type
callee signature
expected type
visible method set
remaining argument labels
```

P5 incremental generation improves this further, but P1 must already have bounded candidate enumeration and must not rely on P5 for basic performance safety.

---

## 6. Generation Layers

```text
L0 Lexical
   Is the current partial token a valid prefix of a keyword, identifier, number,
   or string?

L1 Syntactic
   Which token kinds can grammatically appear next?

L2 Semantic
   Which concrete names, methods, variants, parameter labels, and effect
   keywords are valid in this scope and type context?
```

L0/L1 reduce syntax noise. L2 is where RSScript has leverage over a plain CFG: it knows `.rssi` signatures, receiver types, required effects, expected types, and in-scope names.

The oracle should not duplicate long-term language semantics. It should reuse the parser/checker wherever possible and rely on generated artifacts where duplication would otherwise drift.

---

## 7. Generation MVP Strategy

Do not start with a full BPE logit-mask implementation.

Start with bounded, context-pruned generate-and-test over a small candidate set:

```text
candidate set =
  keywords valid for the syntactic slot
  symbols valid for the syntactic slot
  literal classes valid for the expected type
  in-scope names from symbol_index after scope filtering
  core/package function names after domain and expected-type filtering
  method names from the resolved receiver type
  parameter labels from the resolved callee signature
  required effect keywords from the parameter signature
  variants from known sum types
```

For each candidate in the bounded probe set, tentatively apply the candidate edit and run parser/checker validation. Keep candidates that do not introduce a committed error.

This reuses the checker as the authority and avoids a second grammar in the first version.

The MVP must not attempt unbounded exhaustive probing in large scopes. If the candidate domain is too large, it returns a truncated candidate list and degrades proof-oriented status to `Incomplete` where needed.

Later, generate a decode grammar from the same source of truth as the parser and editor grammar. The generated grammar must have a freshness-guard test.

### 7.1 Oracle Acceptance Suite

P0/P1 are not complete without golden prefix tests.

The suite must cover:

```text
receiver methods:
  offer valid methods after a typed receiver
  do not offer invalid methods
  preserve partial receiver-method prefixes
  do not prove Dead from incomplete receiver/method context

call arguments:
  offer remaining named args only
  offer required effect after an arg label
  reject committed missing/wrong effects
  do not prove Dead from incomplete callee signatures

prefix status:
  never return Dead for a legal partial token prefix
  return Dead for impossible partial tokens only when domain is complete
  return Incomplete for unmatched delimiters
  return Incomplete under partial symbol context
  return Complete only for a full top-level checked program

typing:
  include expected type in expression position
  include result type for completions where known
  offer valid variants for known match scrutinee types

application:
  applying completion.replace + completion.insert_text yields a valid prefix
  completions do not require hidden source transformations

performance:
  large visible scopes respect max_probe_candidates
  truncation is reported explicitly
  truncated candidate sets are not used for Dead proof
```

Representative negative prefixes:

```rss
Path.nonexistent
File.read_bytes(path: "x")
unknown_fn(
match option_value { Ok(
```

Representative incomplete prefixes:

```rss
Path.exi
File.read_bytes(pa
Path.from_string(value: rea
match option_value {
```

---

## 8. Worked Generation Example

Prefix:

```rss
fn main() -> Result<Unit, FileError> {
    let path = Path.
```

Continuations:

```text
names:
  from_string  Method  Path.from_string(value: read String) -> fresh Path
  exists       Method  Path.exists(path: read Path) -> Bool
  ...
may_stop: false
truncated: false
```

`Path.nonexistent` is not offered.

After:

```rss
Path.from_string(
```

Continuations:

```text
names:
  value  ArgName  required_effect=read
```

The applied completion is:

```rss
Path.from_string(value: read 
```

The agent is steered toward:

```rss
Path.from_string(value: read "README.md")
```

The two common RSScript errors, missing argument labels and missing effects, become hard to emit in generated code.

---

## 9. Interpreter API

Public module:

```rust
pub mod interp;
```

Public surface:

```rust
pub struct EvalOptions {
    pub host: HostMode,
    pub entrypoint: String,
    pub args: Vec<String>,
}

pub enum HostMode {
    Real,
    Sandbox(SandboxConfig),
    Denied,
}

pub struct EvalResult {
    pub status: EvalStatus,
    pub value: Option<EvalValue>,
    pub stdout: String,
    pub stderr: String,
    pub logs: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub execution: ExecutionState,
}

pub enum EvalStatus {
    Returned,
    CheckFailed,
    Unsupported,
    RuntimeFault,
    HostDenied,
    ResourceLimitExceeded,
}

pub enum ExecutionState {
    NotStarted,
    Completed,
    Partial,
}

pub fn eval_file(path: &Path, options: EvalOptions) -> EvalResult;
pub fn eval_source(file: &str, source: &str, options: EvalOptions) -> EvalResult;
```

`eval_source` and `eval_file` must check the program before execution. If checking fails, they return `EvalStatus::CheckFailed` and do not execute user code.

The `status` field is authoritative. `value: Option<EvalValue>` must not be used to infer execution state.

Status/value conventions:

```text
Returned:
  value = Some(returned value)
  execution = Completed

CheckFailed:
  value = None
  execution = NotStarted

Unsupported:
  value = None
  execution = NotStarted when discovered by preflight
  execution = Partial only if support is discovered dynamically and prior
              effects already occurred

RuntimeFault:
  value = None
  execution = Partial unless the fault occurs before user execution

HostDenied:
  value = None
  execution = Partial if prior effects occurred, otherwise NotStarted

ResourceLimitExceeded:
  value = None
  execution = Partial
```

A successful `Unit` return is represented as:

```rust
EvalResult {
    status: EvalStatus::Returned,
    value: Some(EvalValue::Unit),
    execution: ExecutionState::Completed,
    ...
}
```

`EvalStatus` does not include backend divergence. Divergence is a parity-harness result, not a normal eval result.

CLI:

```text
rss eval --pure <file>      # P2 pure-only local eval
rss eval <file>             # P3+ sandbox-backed eval once host support exists
rss run --interp <file>     # optional compatibility route
rss test                    # may use interpreter by default once parity is good
```

Agent tool:

```text
rss_eval(path, entrypoint, input, host=sandbox)
```

Agent-facing eval must default to sandbox mode and must not silently fall back to real host execution.

---

## 10. Interpreter Value Model

```rust
enum Value {
    Unit,
    Int(i64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(IndexMap<ValueKey, Value>),
    Struct(StructValue),
    Variant { tag: String, payload: Option<Box<Value>> },
    Managed(Rc<RefCell<Object>>),
    Closure(Rc<ClosureValue>),
    Native(NativeHandle),
}
```

Operational rules:

```text
read x     pass an immutable view / clone shared handle
mut x      mutate the stored binding/object
take x     move the value; the source binding is dead
manage x   wrap a local value into Managed
fresh x    no runtime operation; freshness is statically checked
?          on Err/None, return immediately from the enclosing function
local x    observable result only; local performance optimizations are ignored
```

The interpreter trusts the checker for legality. It does not re-run the ownership/effect checker, but it must still enforce runtime consequences of checked ownership and effect operations:

- `take` must make the source binding unavailable.
- `mut` must mutate the correct binding/object.
- `read` must not expose mutation capability.
- `?` must release resources consistently with backend behavior.
- observable effects must match backend behavior under the same host.

---

## 11. Intrinsic Dispatch

Built-ins must use the same runtime crate as lowered Rust.

```text
RSScript call
  -> checked HIR
  -> runtime_abi lookup
  -> marshal Value arguments
  -> call rsscript_runtime function
  -> marshal return Value
```

The interpreter dispatcher is a finite mapping from `.rssi` signatures and `runtime_abi.rs`.

The pure smoke-test MVP may handwrite a tiny dispatcher for core types. Broad stdlib coverage must not rely on handwritten marshalling. P4 must generate the dispatcher from the authoritative runtime ABI table plus `.rssi` signatures, with a freshness-guard test.

Generated dispatch should validate:

- callee identity
- parameter count
- parameter names
- required effects
- argument value marshalling
- return value marshalling
- host capability requirements
- diagnostic mapping

Generated dispatcher artifacts must fail tests when `runtime_abi.rs`, `.rssi` signatures, or generated marshalling tables drift.

---

## 12. Host Boundary

The interpreter must not let agent probes freely touch the machine.

```rust
trait Host {
    fn fs(&self) -> &dyn FsHost;
    fn clock(&self) -> &dyn ClockHost;
    fn random(&self) -> &dyn RandomHost;
    fn net(&self) -> &dyn NetHost;
    fn process(&self) -> &dyn ProcessHost;
    fn env(&self) -> &dyn EnvHost;
}
```

Modes:

- `RealHost`: pass-through to runtime intrinsics; used for parity and explicit local execution.
- `SandboxHost`: in-memory or confined FS, denied network, denied process, fixed clock, seeded random, controlled environment.
- `DeniedHost`: all host capabilities are denied.

P2 pure interpreter may provide `DeniedHost` or equivalent default-deny stubs for all host capabilities. Pure core intrinsics should not need host access. If a P2 program reaches a host-dependent intrinsic, the interpreter should fail closed with `Unsupported` or `HostDenied`, depending on whether the intrinsic is outside P2 coverage or explicitly denied by host policy.

The code-agent should default to `SandboxHost` once agent-facing eval exists. Real host must be explicit in tool policy.

### 12.1 Agent-Facing Safety Contract

`rss_eval` can exist locally before the sandbox is complete, but it must not be exposed as an agent tool with real host access.

```text
P2 pure interpreter:
  no fs/net/process/env/clock/random host access
  only pure core intrinsics
  DeniedHost or default-deny stubs

P2 local CLI:
  rss eval --pure for explicit developer use

P3 agent-facing eval:
  sandbox host required
  real host forbidden by default tool policy
```

Agent-facing eval may only use `SandboxHost` unless a tool policy explicitly grants real host access for a trusted local run.

### 12.2 Sandbox Resource Limits

Sandboxing is not only about filesystem and network access. Agent-facing eval must also enforce resource limits.

```text
Sandbox limits:
  max evaluation steps / instructions
  max wall time
  max call depth
  max heap/value size
  max stdout bytes
  max stderr bytes
  max log bytes
  max filesystem bytes
  max filesystem file count
  max diagnostics emitted
```

Resource-limit failures return:

```rust
EvalStatus::ResourceLimitExceeded
```

and emit:

```text
RS1206 sandbox resource limit exceeded
```

Resource limits must fail closed. The interpreter must not continue running after a hard sandbox limit is exceeded.

---

## 13. Interpreter Supported Subset

### 13.1 P2 Pure Interpreter MVP

P2 supports pure checked HIR execution:

- scalars: `Unit`, `Bool`, `Int`, `String`
- `List`
- `Map`
- `Option`
- `Result`
- structs
- sum variants
- `if`
- `match`
- `for`
- `while`
- `return`
- `break`
- `continue`
- user functions
- closures only where already checked and noescape
- core pure intrinsics:
  - `String`
  - `List`
  - `Map`
  - `Json`
  - `Assert`
  - `Log`
  - basic `Result` / `Option`

P2 should avoid host-dependent APIs except where the host dependency is explicitly mocked or denied.

Before executing an entrypoint, P2 should preflight the reachable checked HIR for unsupported constructs and unsupported intrinsics where practical. Preflight-discovered unsupported constructs return `Unsupported` with `execution = NotStarted`.

### 13.2 P3 Sandbox and Deterministic Eval

P3 adds agent-facing eval under sandbox policy:

- in-memory or confined filesystem
- denied process by default
- denied network by default
- fixed clock
- seeded random
- controlled environment
- resource limits
- deterministic eval/test fixtures

### 13.3 P6 Interpreter Coverage Expansion

P6 expands interpreter coverage:

- `with`
- resources
- `ResourcePool`
- broader file/path APIs under host policy
- resource lifecycle parity
- broader stdlib support through generated dispatcher

### 13.4 Async Coverage

Async support belongs in expanded interpreter coverage after core parity is stable:

- async/await subset over `async_runtime`
- `Timer`
- `Channel`
- `Stream.next`
- `await for` where already supported by HIR

Unsupported constructs must emit an explicit diagnostic. They must not silently fall back to guessed behavior.

---

## 14. Interpreter Invariants

The interpreter executes checked HIR, but the runtime boundary still needs hard operational rules:

1. Native dispatch must not expose mutation capability for a parameter checked as `read`.
2. `mut` and `take` must be enforced at the interpreter binding/object layer, even though the checker already proved they are legal.
3. `Managed` aliasing must mirror the runtime's `Rc<RefCell<...>>` behavior, including dynamic borrow/runtime diagnostics where applicable.
4. `Native` handles are opaque. They can only be observed or mutated through checked intrinsics and host policy.
5. `ValueKey` supports only stable, hashable, equality-stable values.
6. `Managed`, `Native`, `Closure`, and other unstable identity values are forbidden as map keys unless a future spec defines canonicalization explicitly.
7. Resource lifecycle events are observable for parity.
8. `with`, pool leases, and early `?` returns must release or discard resources exactly as the backend does.
9. The interpreter may ignore local performance optimizations.
10. The interpreter may not ignore observable resource, mutation, error, log, or host effects.
11. Map iteration order is observable when code observes it through logs, output, returned lists, or host effects.
12. v0.1 interpreter parity does not define new RSScript language semantics for `Map` iteration.
13. Map-iteration parity fixtures are unsupported until the interpreter and lowered Rust backend share an explicit ordering policy.
14. If the language later specifies insertion order, the backend must use an equivalent insertion-ordered map before those fixtures count as supported.
15. If the language specifies map iteration as unordered, parity fixtures must canonicalize map observations rather than asserting concrete iteration order.

This matters because:

```rss
for k in map {
    Log.info(value: read k)
}
```

has observable log order.

Current Rust lowering may use a hash map backend. Until backend and interpreter ordering are aligned, tests that depend on concrete map iteration order must be marked unsupported for strict parity.

---

## 15. Diagnostics

Interpreter diagnostics should use RSScript spans directly. This is better than the Rust backend path, which needs source-map remapping.

```text
RS1201 runtime fault
RS1202 unsupported interpreter construct
RS1203 sandbox host denial
RS1204 intrinsic marshalling failure
RS1205 interpreter/backend parity divergence
RS1206 sandbox resource limit exceeded
```

The diagnostic JSON shape should match existing checker diagnostics so the agent can consume `rss_check`, `rss_eval`, and `rss_run` uniformly.

Diagnostics should include:

- code
- severity
- primary span
- message
- structured reason/category
- host mode where relevant
- whether execution occurred
- whether execution was partial

For parity, diagnostics should be compared in normalized form:

```text
normalized diagnostic comparison:
  diagnostic code
  severity
  primary source span after source-map normalization
  structured reason/category
```

User-visible message text may have separate snapshots, but semantic parity should not depend on exact prose.

---

## 16. Parity

The Rust backend remains authoritative.

Parity harness:

```text
for each supported fixture:
  run with interpreter under fixed host
  run lowered Rust under equivalent fixed host
  compare:
    return value
    stdout / stderr
    logs according to ordering policy
    normalized diagnostics
    filesystem diff under fixed host
    env access trace
    network/process denial trace
    resource lifecycle events
    panic/error boundary behavior
```

If results diverge, the interpreter is wrong unless the fixture declares a permitted schedule-dependent ordering difference.

If a construct is unsupported, the interpreter emits an unsupported diagnostic and the fixture is not counted as a parity success.

Parity should be fixture-driven and explicit. A fixture should declare:

- required interpreter feature set
- host mode
- input files
- environment values
- clock seed/time
- random seed
- scheduler mode, if async
- ordering policy for logs/traces
- expected stdout/stderr/logs
- expected return value
- expected filesystem diff
- expected diagnostics

The parity harness is the primary defense against interpreter drift.

### 16.1 Parity Result

Backend divergence is reported by the parity harness, not by ordinary eval.

```rust
pub struct ParityResult {
    pub status: ParityStatus,
    pub interpreter: EvalResult,
    pub backend: BackendRunResult,
    pub diagnostics: Vec<Diagnostic>,
}

pub enum ParityStatus {
    Match,
    Diverged,
    IneligibleUnsupported,
    IneligibleNondeterministic,
}
```

`ParityStatus::Diverged` emits `RS1205 interpreter/backend parity divergence`.

### 16.2 Ordered and Unordered Observations

Not every observable event has the same ordering guarantee.

The parity harness must classify observations:

```text
strictly ordered:
  stdout bytes
  stderr bytes
  logs emitted by a single sequential task
  resource lifecycle events with direct happens-before relation

happens-before ordered:
  events across async tasks where the program establishes ordering through
  await, channel send/receive, join, or other synchronization

unordered / schedule-dependent:
  logs or side effects from concurrent tasks without a program-level ordering
  guarantee
```

Strictly ordered observations must match exactly.

Happens-before ordered observations must preserve the same causal order, but unrelated events may interleave differently.

Unordered schedule-dependent observations must be compared as a multiset, canonicalized trace, or fixture-specific predicate.

### 16.3 Async Parity

Async parity must not require the interpreter tree-walker to reproduce the exact byte-level scheduling of the lowered Rust runtime unless both are running under an explicitly deterministic scheduler.

For async fixtures, parity must declare one of:

```text
deterministic scheduler:
  interpreter and backend use equivalent deterministic scheduling

schedule-insensitive assertion:
  fixture compares only values/events whose order is specified by the program

explicit nondeterminism:
  fixture is not eligible for strict parity
```

P6 async support is not complete until async parity has a scheduler or trace-equivalence model that makes tests satisfiable.

---

## 17. Agent Integration

The code-agent should use the tools in this order:

```text
1. read context and AGENT.md
2. generate a small function or file section
3. rss_generate for local legal continuations when uncertain
4. rss_check for contract validity
5. rss_eval for behavior under sandbox
6. rss_run only for final backend validation or unsupported interpreter cases
7. review / REIR for the final change boundary
```

The agent should prefer smaller edits because the generation oracle and interpreter both work best on tight scopes. Large rewrites should be decomposed into functions with check/eval after each one.

The agent should treat oracle and eval output as feedback, not as proof of correctness. Final changes still require checker/backend validation and review.

Agent behavior rules:

```text
Prefer explicit source over clever generation.
Use rss_generate when uncertain about syntax, names, labels, effects, or variants.
Use rss_check after each meaningful generated unit.
Use rss_eval for fast sandboxed behavior checks.
Use rss_run for backend-authoritative final validation.
Do not use RealHost unless explicitly permitted by tool policy.
Do not bypass review / REIR.
```

---

## 18. Implementation Plan

### P0 Prefix Oracle Foundation

Deliverables:

```text
token frontier / partial token model
GenerateContext
ProjectContext integration
SymbolCompleteness handling
prefix_status
Dead soundness rules
parser recovery / hole representation
golden prefix tests
```

P0 is complete when:

- `Complete` only returns for a full top-level checked program.
- `Incomplete` handles truncation and unmatched delimiters conservatively.
- `Dead` is sound.
- partial token prefixes are preserved.
- partial symbol context degrades to `Incomplete`.
- golden tests cover positive, incomplete, and dead prefixes.

### P1 Semantic Continuations MVP

Deliverables:

```text
valid_continuations
receiver method names
argument labels and required effects
in-scope locals
expected type metadata
completion result type metadata
completion application contract
bounded candidate enumeration
scope/domain pruning
partial-token prefix filtering
expected-type filtering where available
probe budget
truncated continuation metadata
agent tool: rss_generate
```

P1 is complete when:

- valid receiver methods are offered.
- invalid receiver methods are not offered when the domain is complete.
- remaining named arguments are offered.
- required effects are offered.
- completion edits apply cleanly.
- expected type and result type metadata are available where known.
- candidate probing is bounded.
- large scopes do not cause unbounded `N × full reparse` behavior.
- truncation is explicit.
- budget exhaustion degrades to `Incomplete`, not unsound `Dead`.
- `rss_generate` is useful to an agent without human autocomplete UI assumptions.

### P2 Pure Interpreter MVP

Deliverables:

```text
checked HIR tree walker
scalar/List/Map/Result/Option/control flow/user fn support
String/List/Json/Assert/Log intrinsics
EvalStatus/value convention
ExecutionState
stderr capture
DeniedHost or default-deny host stubs
map iteration parity policy
CLI: rss eval --pure
```

P2 is complete when:

- pure checked programs execute without Rust lowering.
- unsupported constructs fail closed.
- unsupported constructs are preflighted where practical.
- check failures do not execute.
- runtime faults have diagnostics.
- local pure-only CLI usage is possible.
- host-dependent APIs fail closed.
- eval status/value conventions are implemented.
- map iteration behavior is either matched with backend or unsupported for parity fixtures.
- no agent-facing real host eval is exposed.

### P3 Sandbox and Agent-Facing Eval

Deliverables:

```text
in-memory/confined FS
denied process/net by default
fixed clock and seeded random
controlled environment
resource limits
agent tool: rss_eval
deterministic eval/test fixtures
```

P3 is complete when:

- agent-facing eval requires sandbox host by default.
- real host access is policy-gated.
- resource limits are enforced.
- host denial emits diagnostics.
- deterministic fixtures are reproducible.

### P4 Runtime ABI Generated Dispatcher

Deliverables:

```text
generated marshalling from runtime_abi + .rssi
freshness guard
parity fixtures for supported intrinsics
host capability annotations
diagnostic mapping
```

P4 is complete when:

- broad stdlib coverage no longer depends on handwritten marshalling.
- ABI/signature drift fails tests.
- interpreter intrinsic calls use generated dispatcher metadata.
- supported intrinsic fixtures pass parity.

### P5 Incremental Generator

Deliverables:

```text
Generator handle
checkpoint/restore
incremental lexer/parser/checker frontier
candidate scoring metadata
speculative decoding support
```

P5 is complete when:

- generator state can be checkpointed and restored.
- candidate probing does not require full reparsing in common cases.
- speculative decoding can use the oracle efficiently.

### P6 Interpreter Coverage Expansion

Deliverables:

```text
resources
ResourcePool
broader stdlib
broader file/path APIs under host policy
resource lifecycle parity
async parity policy
deterministic scheduler or trace-equivalence model
schedule-aware log comparison
async subset where stable
```

P6 is complete when:

- expanded constructs are either supported with parity or rejected with explicit diagnostics.
- resource lifecycle behavior matches backend behavior.
- supported stdlib surface has generated dispatcher coverage.
- async fixtures do not rely on impossible exact runtime scheduling parity.
- concurrent observations are compared according to declared ordering guarantees.

### P7 Generated Decode Grammar

Deliverables:

```text
decode grammar generated from parser/keyword source
freshness-guard tests
snapshot tests
editor grammar alignment where applicable
```

P7 is complete when:

- decode grammar is generated, not manually duplicated.
- grammar drift fails tests.
- grammar can support later model-runtime adapters.

### P8 Model-Runtime Adapters

Deliverables:

```text
BPE token healing
logit masks for llama.cpp/vLLM/etc.
speculative decoding integration
adapter-level tests
```

P8 is complete when:

- legal RSScript continuations can be translated into model-token constraints.
- token healing handles BPE boundary issues.
- adapters remain downstream of the RSScript-token oracle.

---

## 19. Non-Goals

- No human autocomplete.
- No hidden type/effect inference in emitted source.
- No correctness guarantee beyond checker/eval/review.
- No replacement for Rust backend execution authority.
- No performance benchmarking through the interpreter.
- No unrestricted real-host execution in agent default mode.
- No second hand-written language semantics.
- No broad stdlib interpreter coverage before generated ABI marshalling.
- No silent fallback from unsupported interpreter constructs to guessed behavior.
- No unbounded candidate probing in the oracle MVP.
- No `Dead` proof from partial project context or truncated candidate lists.
- No strict async ordering parity for programs with schedule-dependent behavior.

---

## 20. Risks

### Incomplete Semantic Pruning

Some invalid programs require non-local context.

Mitigation:

```text
conservative Incomplete
sound Dead only
later backtracking
final rss check
```

### Oracle Candidate Explosion

Generate-and-test can become expensive if every visible name, package function, and method is probed with a full parser/checker pass.

Mitigation:

```text
context-first candidate domains
partial-token prefix filters
expected-type filters
receiver/callee-specific lookup
probe budgets
truncated continuation metadata
cached parser/checker frontier data
budget exhaustion degrades to Incomplete
P5 incremental generator for deeper optimization
```

### Unsound Dead from Incomplete Symbol Context

`Dead` can be unsound if the oracle does not have complete import, package, core, or receiver-method information.

Mitigation:

```text
GenerateContext carries ProjectContext
SymbolCompleteness is explicit
Dead requires complete semantic domain
partial symbol information degrades to Incomplete
unknown-name Dead is forbidden under partial symbol context
```

### Parser Expectation Drift

A separate decode grammar can diverge from the real parser.

Mitigation:

```text
generate grammar from parser/keyword source
snapshot generated grammar
freshness-guard tests
```

### Interpreter Drift

Tree-walker behavior can diverge from lowered Rust.

Mitigation:

```text
parity harness
backend wins
unsupported constructs fail closed
```

### Marshalling Bugs

The intrinsic boundary is the largest implementation risk.

Mitigation:

```text
generate dispatcher from runtime_abi.rs and .rssi
freshness-guard tests
fixture-level parity
handwritten dispatcher only for tiny smoke tests
```

### Sandbox False Confidence

Sandbox behavior can differ from real host.

Mitigation:

```text
label host mode in diagnostics
use fixed deterministic host for parity
perform final backend validation where needed
do not treat sandbox eval as final correctness proof
```

### Resource Exhaustion

Agent-facing eval can encounter infinite loops, deep recursion, huge logs, or large values.

Mitigation:

```text
step limits
wall-time limits
call-depth limits
heap/value limits
stdout/stderr/log limits
filesystem limits
fail-closed diagnostics
```

### Async Scheduling Drift

The interpreter tree-walker and lowered Rust backend may schedule async tasks differently. Exact log or side-effect ordering may be impossible to match for unconstrained concurrent programs.

Mitigation:

```text
classify observations by ordering guarantee
use deterministic scheduler where strict parity is required
compare schedule-dependent events as multisets or canonical traces
exclude explicitly nondeterministic fixtures from strict parity
document async parity eligibility
```

### Map Iteration Drift

Interpreter `Map(IndexMap<...>)` may preserve insertion order while the backend map implementation may not, or vice versa.

Mitigation:

```text
specify RSScript map iteration order for interpreter-supported fixtures
ensure interpreter and backend use equivalent map ordering for supported parity fixtures
mark map-iteration fixtures unsupported until backend order matches
canonicalize map observations if a future spec makes order unspecified
```

---

## 21. Summary

RSScript should not rely on prompts alone.

```text
AGENT.md gives the model the rules.
The generation oracle keeps it on valid paths.
The interpreter lets it test behavior quickly.
The checker and backend keep the language honest.
The review map keeps the product boundary explicit.
```

This is the RSScript-specific AI loop: generated source remains ordinary, explicit, and reviewable, while the compiler and runtime make invalid or untested drafts much cheaper to eliminate.

The design preserves the core RSScript model:

```text
explicit source
checked contracts
fast sandbox probes
backend authority
review-first product boundary
```

The oracle and interpreter are not shortcuts around RSScript's discipline. They are mechanisms for making that discipline cheaper for agents to follow.
