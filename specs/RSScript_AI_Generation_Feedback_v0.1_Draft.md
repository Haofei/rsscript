# RSScript AI Generation Feedback — Design Draft v0.1

*Status: draft / design sketch. This document combines the constrained-generation
and interpreter drafts into one design for making AI-generated RSScript both
faster to produce and faster to validate.*

## 1. Why

RSScript has almost no model pretraining prior. A prompt-sized guide can teach
the language, but it cannot reliably prevent the common failures:

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

This is not autocomplete for humans. It is an agent-facing compiler oracle and a
fast runtime probe. The goal is to remove low-value generate/check/repair cycles
without weakening RSScript's review-first model.

## 2. Design Principle

RSScript should make correct code cheap to generate without making behavior
implicit.

```text
Prompting teaches.
Constrained generation steers.
Checking proves the contract.
The interpreter probes behavior quickly.
The Rust backend defines final behavior.
Review remains the bottleneck.
```

Constrained generation must not become hidden inference. It may offer legal next
tokens, names, parameter labels, and required effects, but the emitted source is
still ordinary RSScript. The source remains reviewable without knowing the
generation machinery.

The interpreter must not become a second semantics. It executes a supported
subset for fast feedback and is continuously checked against the lowered Rust
backend.

## 3. Two Pieces, One Loop

### 3.1 Generation Oracle

The oracle exposes the checker over an incomplete program. It answers two
questions:

```text
Can this prefix still become valid RSScript?
What RSScript tokens/names are legal next?
```

It works at the RSScript-token frontier, not directly at the model's BPE token
level. A model adapter can later turn legal RSScript continuations into a logit
mask, but the first useful version is an agent tool.

### 3.2 Interpreter

The interpreter executes checked RSScript without lowering to a temporary Rust
package and invoking `cargo run`.

```text
rss check   type/effect feedback in ms
rss eval    behavioral feedback in ms
rss run     backend-authoritative execution in seconds
```

It is a tree-walker over checked HIR/user code and dispatches built-in calls
through the same runtime crate used by lowered Rust.

## 4. Generation Oracle API

Public module:

```rust
pub mod generate;
```

### 4.1 Prefix Status

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixStatus {
    Complete,
    Incomplete,
    Dead { at: Span, reason: String },
}

pub fn prefix_status(file: &str, partial_source: &str) -> PrefixStatus;
```

Meaning:

- `Complete`: the source is complete and passes the supported checks.
- `Incomplete`: the source is truncated but still on a path to validity.
- `Dead`: the source has committed to something invalid that cannot be fixed by
  appending more tokens.

The first version should be conservative. Treat ambiguous truncation as
`Incomplete`; only return `Dead` for clear committed errors such as unknown
callee, unknown argument, missing required effect after a completed argument, or
receiver method that cannot exist.

### 4.2 Continuations

```rust
#[derive(Debug, Clone)]
pub struct Continuations {
    pub keywords: Vec<&'static str>,
    pub symbols: Vec<&'static str>,
    pub literals: Vec<LiteralClass>,
    pub names: Vec<Completion>,
    pub expected_type: Option<String>,
    pub may_stop: bool,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub kind: CompletionKind,
    pub signature: Option<String>,
    pub required_effect: Option<&'static str>,
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

pub fn valid_continuations(file: &str, partial_source: &str) -> Continuations;
```

The high-value completions are semantic, not UI-oriented:

- after `Path.`: only valid `Path` methods
- after `path.`: methods for the inferred receiver type
- after `File.read_bytes(`: only remaining parameter names
- after `path:`: the required effect, usually `read`
- in expression position: in-scope locals/params and valid literals
- after `match value {`: valid variants for the scrutinee type where known

This should be exposed as an agent tool named something like `rss_generate`, not
as human autocomplete. Human-facing completion is explicitly not a goal.

### 4.3 Incremental Handle

```rust
pub struct Generator {
    // lexer/parser/checker frontier
}

impl Generator {
    pub fn new(file: &str) -> Self;
    pub fn push(&mut self, token: &str) -> PrefixStatus;
    pub fn peek(&self, token: &str) -> PrefixStatus;
    pub fn continuations(&self) -> Continuations;
    pub fn checkpoint(&self) -> GeneratorState;
    pub fn restore(&mut self, state: GeneratorState);
}
```

This is the production shape for speculative decoding and backtracking. P0 can
reparse prefixes; P2 should keep an incremental frontier.

## 5. Generation Layers

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

L0/L1 reduce syntax noise. L2 is where RSScript has leverage over a plain CFG:
it knows `.rssi` signatures, receiver types, required effects, and in-scope
names.

## 6. Generation MVP Strategy

Do not start with a full BPE logit-mask implementation.

Start with generate-and-test over a small candidate set:

```text
candidate set =
  keywords
  symbols
  literal classes
  in-scope names from symbol_index
  core/package function names
  method names from receiver type
  parameter labels from callee signature
  required effect keywords from parameter signature
```

For each candidate, tentatively run parser/checker on `prefix + candidate` and
keep candidates that do not introduce a committed error. This reuses the checker
as the authority and avoids a second grammar in the first version.

Later, generate a decode grammar from the same source of truth as the parser and
editor grammar. The generated grammar must have a freshness-guard test.

## 7. Worked Generation Example

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

The agent is steered toward:

```rss
Path.from_string(value: read "README.md")
```

The two common RSScript errors, missing argument labels and missing effects,
become hard to emit in generated code.

## 8. Interpreter API

Public surface:

```rust
pub mod interp;

pub struct EvalOptions {
    pub host: HostMode,
    pub entrypoint: String,
    pub args: Vec<String>,
}

pub enum HostMode {
    Real,
    Sandbox(SandboxConfig),
}

pub struct EvalResult {
    pub value: Option<EvalValue>,
    pub stdout: String,
    pub logs: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn eval_file(path: &Path, options: EvalOptions) -> EvalResult;
pub fn eval_source(file: &str, source: &str, options: EvalOptions) -> EvalResult;
```

CLI:

```text
rss eval <file>
rss run --interp <file>     # optional compatibility route
rss test                    # may use interpreter by default once parity is good
```

Agent tool:

```text
rss_eval(path, entrypoint, input, host=sandbox)
```

## 9. Interpreter Value Model

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

The interpreter trusts the checker. It does not re-run the ownership/effect
rules; it executes a checked program.

## 10. Intrinsic Dispatch

Built-ins must use the same runtime crate as lowered Rust.

```text
RSScript call
  -> checked HIR
  -> runtime_abi lookup
  -> marshal Value arguments
  -> call rsscript_runtime function
  -> marshal return Value
```

The interpreter dispatcher is a finite mapping from `.rssi` signatures and
`runtime_abi.rs`. P0 can handwrite a small dispatcher for core types. P5 should
generate the dispatcher from the authoritative table plus `.rssi` signatures,
with a freshness-guard test.

## 11. Host Boundary

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

- `RealHost`: pass-through to runtime intrinsics; used for parity and explicit
  local execution.
- `SandboxHost`: in-memory or confined FS, denied network, denied process, fixed
  clock, seeded random, controlled environment.

The code-agent should default to `SandboxHost`. Real host must be explicit in
tool policy.

## 12. Interpreter Supported Subset

P0 supported:

- scalars: `Unit`, `Bool`, `Int`, `String`
- `List`, `Map`, `Option`, `Result`
- structs and sum variants
- `if`, `match`, `for`, `while`
- `return`, `break`, `continue`
- user functions
- closures only where already checked and noescape
- core intrinsics: `String`, `List`, `Map`, `Json`, `Assert`, `Log`, basic
  `Result` / `Option`

P1 supported:

- `with`
- resources
- `ResourcePool`
- broader file/path APIs under host policy

P2 supported:

- sandbox host
- deterministic tests

P3 supported:

- async/await subset over `async_runtime`
- `Timer`, `Channel`, `Stream.next`, `await for` where already supported by HIR

Unsupported constructs must emit an explicit diagnostic. They must not silently
fall back to guessed behavior.

## 13. Diagnostics

Interpreter diagnostics should use RSScript spans directly. This is better than
the Rust backend path, which needs source-map remapping.

```text
RS1201 runtime fault
RS1202 unsupported interpreter construct
RS1203 sandbox host denial
RS1204 intrinsic marshalling failure
RS1205 interpreter/backend parity divergence
```

The diagnostic JSON shape should match existing checker diagnostics so the agent
can consume `rss_check`, `rss_eval`, and `rss_run` uniformly.

## 14. Parity

The Rust backend remains authoritative.

Parity harness:

```text
for each supported fixture:
  run with interpreter under fixed host
  run lowered Rust under equivalent fixed host
  compare:
    return value
    logs
    stdout
    diagnostics
```

If results diverge, the interpreter is wrong. If a construct is unsupported, the
interpreter emits an unsupported diagnostic and the fixture is not counted as a
parity success.

## 15. Agent Integration

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

The agent should prefer smaller edits because the generation oracle and
interpreter both work best on tight scopes. Large rewrites should be decomposed
into functions with check/eval after each one.

## 16. Phased Plan

```text
P0 Generation oracle, syntax + core semantic MVP
   prefix_status
   valid_continuations
   receiver method names
   argument labels and required effects
   agent tool: rss_generate

P1 Interpreter MVP
   checked HIR tree walker
   scalar/List/Map/Result/Option/control flow/user fn support
   String/List/Json/Assert/Log intrinsics
   CLI: rss eval
   agent tool: rss_eval

P2 Sandbox host
   in-memory/confined FS
   denied process/net by default
   fixed clock and seeded random
   deterministic eval/test fixtures

P3 Incremental generator
   Generator handle
   checkpoint/restore
   candidate scoring metadata

P4 Interpreter coverage
   resources
   ResourcePool
   broader stdlib
   async subset

P5 Generated artifacts
   decode grammar generated from parser/keyword source
   interpreter marshalling generated from runtime_abi + .rssi
   freshness-guard tests

P6 Model-runtime adapters
   BPE token healing
   logit masks for llama.cpp/vLLM/etc.
   speculative decoding integration
```

## 17. Non-Goals

- No human autocomplete.
- No hidden type/effect inference in emitted source.
- No correctness guarantee beyond checker/eval/review.
- No replacement for Rust backend execution authority.
- No performance benchmarking through the interpreter.
- No unrestricted real-host execution in agent default mode.

## 18. Risks

- **Incomplete semantic pruning**: some invalid programs require non-local
  context. Mitigation: conservative `Incomplete`, later backtracking, and final
  `rss check`.
- **Parser expectation drift**: a separate decode grammar can diverge.
  Mitigation: generate grammar from parser/keyword source and snapshot it.
- **Interpreter drift**: tree-walker behavior can diverge from lowered Rust.
  Mitigation: parity harness; backend wins.
- **Marshalling bugs**: intrinsic boundary is the largest implementation risk.
  Mitigation: generate dispatcher from `runtime_abi.rs` and `.rssi`.
- **Sandbox false confidence**: sandbox behavior can differ from real host.
  Mitigation: label host mode in diagnostics and final validation.

## 19. Summary

RSScript should not rely on prompts alone.

```text
AGENT.md gives the model the rules.
The generation oracle keeps it on valid paths.
The interpreter lets it test behavior quickly.
The checker and backend keep the language honest.
The review map keeps the product boundary explicit.
```

This is the RSScript-specific AI loop: generated source remains ordinary,
explicit, and reviewable, while the compiler and runtime make invalid or
untested drafts much cheaper to eliminate.
