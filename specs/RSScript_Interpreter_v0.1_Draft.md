# RSScript Interpreter — Design Draft v0.1

*Status: draft / design sketch. A fast, behavior-faithful interpreter for the
agent feedback loop, `rss test`, and the constrained-generation runtime layer.
It does **not** replace the Rust backend (Constitution Article VII): the lowered
Rust remains the execution authority; the interpreter must observably agree with
it on the supported subset, and differential testing keeps it honest.*

## 1. Why

Today execution is: **lower to a Rust package → `cargo run` (full rustc) →
parse runtime diagnostics.** That is the right *production* path and the wrong
*feedback* path — an agent (or a human) editing one function pays seconds of
rustc per try.

```text
rss check   type/effect feedback in ms          (have it)
rss run     behavioral feedback in seconds       (rustc tax)
rss eval    behavioral feedback in ms            (this draft)
```

MoonBit's roadmap names exactly this: a fast interpreter for **runtime** feedback
to the AI, on top of the type feedback the checker already gives. The agent's
`rss_check` answers "does it type-check?"; the interpreter answers "does it
*do the right thing?*" — without waiting for a compile.

This serves four consumers:

```text
1. the code-agent inner loop   sub-second "did my function return the right value?"
2. rss test                    run RSS tests without compiling each
3. constrained generation      runtime validation of a generated snippet (companion draft)
4. a REPL / playground          try.rsscript, like try.moonbitlang
```

## 2. The key leverage: reuse `rsscript-runtime` wholesale

The runtime crate `rsscript-runtime` already implements every stdlib intrinsic
(`fs`, `process`, `collections`, `managed`, `async_runtime`, `resource_pool`,
`string_helpers`, `encoding`, …) and the managed value model (`managed.rs`,
`Rc<RefCell<…>>`, per [[managed-is-not-send]]). The lowered Rust **calls these
same functions**, dispatched through the table in `src/runtime_abi.rs`:

```rust
runtime_intrinsic("Assert", "equal", "rsscript_runtime::assert_equal")
runtime_intrinsic("File",   "read_string", "rsscript_runtime::fs_read_string")
// ... one row per Type.method
```

So the interpreter is **not** a second implementation of the language. It is a
tree-walker over user code that, on every built-in call, dispatches through the
*same* `runtime_abi` table into the *same* runtime functions the Rust backend
uses. Behavioral parity is structural, not aspirational: the stdlib is shared.

The only genuinely new code is: (a) evaluating user-defined functions, control
flow, and the effect/ownership operators, and (b) marshalling interpreter
`Value`s across the intrinsic boundary.

## 3. What it interprets

The **post-analysis program** the checker already builds — the HIR
(`Hir::from_syntax`), or the AST with spans as a fallback. The HIR is preferred
because it is desugared (receiver-call shorthand expanded to canonical
`Type.method(self: ...)`, etc.), so the interpreter handles fewer cases.

Crucially, **the interpreter trusts the checker.** Effects, ownership, freshness,
and exhaustiveness are already enforced before interpretation. The interpreter
does not re-verify them; it executes operationally, assuming valid input. It is
not a borrow checker — it is an evaluator.

## 4. Value model

```rust
enum Value {
    Unit,
    Int(i64),
    Bool(bool),
    // String / Bytes / Buffer wrap the runtime's own types so intrinsics take them directly.
    Str(rsscript_runtime::RsString),
    Bytes(rsscript_runtime::Bytes),

    // Managed reference types: class instances, and List/Map (managed collections).
    // Shared, interior-mutable — mirrors the lowering's ARC model.
    Managed(Rc<RefCell<Object>>),

    // Value structs and sealed sum variants (incl. built-in Result/Option).
    Struct(StructValue),
    Variant { tag: VariantId, payload: Box<Value> }, // Ok/Err/Some/None are variants

    // First-class function value (closures capture an Env).
    Closure(Rc<ClosureValue>),

    // Opaque native handles (File, HttpResponse, DbConnection, ...) hold the
    // runtime's actual handle type, so intrinsics accept them unchanged.
    Native(NativeHandle),
}
```

Operational meaning of the effect/binding operators (all statically pre-checked,
so these are pure execution rules):

```text
read x     share: clone the Rc / pass an immutable view
mut x      mutate through the RefCell
take x     move the value out; the source binding is now dead
manage x   wrap a local value into a Managed(Rc) — the one-way local→managed move
fresh ...  return the freshly built value (no special runtime effect)
local x    treated as an owned Value; the performance semantics of `local`
           (buffer reuse, in-place move) are NOT observable, so the interpreter
           need only reproduce the *result*, not the optimization.
?          on Err/None, return it from the enclosing fn immediately
```

That `local` is behaviorally invisible is a real simplification: the interpreter
ignores the fast-path machinery and still agrees with the backend on results.

## 5. Execution

A standard tree-walker:

```text
Env            lexical scopes: name -> Value, plus a call-frame stack.
call dispatch  Type.method ->
                 user-defined: look up FunctionDecl by dotted name, bind params, run body.
                 intrinsic:    look up runtime_abi -> marshal args -> call rsscript_runtime fn -> marshal result.
control flow   if / while / loop / for(List<T>) / match(exhaustive) / with / break / continue / return.
errors         Result/Option are ordinary values. A runtime fault (Assert failure,
               index out of range, intrinsic error) is caught and surfaced as a
               structured runtime diagnostic, never an interpreter-process panic.
resources      `with R as r { }` and ResourcePool: acquire via runtime, run body,
               release on scope exit (incl. on `?` early return) — reusing
               resource_pool.rs.
async          single-isolate cooperative: an `await <async call>` is driven to
               completion by a minimal executor over async_runtime.rs. v0.x may
               poll-to-completion synchronously since the model is single-isolate.
```

### 5.1 Diagnostics are *better* here than via lowered Rust

The lowered-Rust path emits rustc/runtime errors that must be remapped back to
RSScript through source maps (`parse_runtime_diagnostics`, `RustSourceMapEntry`).
The interpreter walks the AST/HIR directly, so a runtime fault already **has the
RSScript span** — no source map. It emits the same `Diagnostic` shape (stable
code, span, JSON) the agent already consumes, e.g. `RS1201` runtime diagnostic,
so feedback is identical in form whether the agent runs `rss eval` or `rss run`.

## 6. The host boundary (sandbox + determinism)

Intrinsics that touch the world (`fs`, `process`, `socket`, `clock`, `random`,
`env`) go through a `Host` trait instead of calling the OS directly:

```rust
trait Host {
    fn fs(&self) -> &dyn FsHost;        // real FS, or in-memory
    fn clock(&self) -> &dyn ClockHost;  // real time, or fixed
    fn random(&self) -> &dyn RandomHost; // real, or seeded
    fn net(&self) -> &dyn NetHost;      // real, or denied/mocked
    fn process(&self) -> &dyn ProcessHost;
}
```

- **Real host**: thin pass-through to the existing `rsscript-runtime` intrinsics
  — behavioral parity with the Rust backend.
- **Sandbox host**: in-memory FS, denied network, fixed clock, seeded RNG. Makes
  the agent's quick runs **safe and deterministic**, and makes `rss test` and
  parity tests reproducible. This is what lets the code-agent behaviorally probe
  generated code without touching the real machine.

## 7. Parity guarantee (the trust mechanism)

The interpreter is only useful if "passes in the interpreter" implies "passes in
the backend." Enforced the same way RSScript enforces every generated artifact —
a freshness/parity test (cf. the generated TextMate grammar guard):

```text
tests/interp_parity.rs:
  for each program in examples/scripts/**:
      run via interpreter      -> (return value, Log output, diagnostics)
      run via lowered Rust      -> (return value, Log output, diagnostics)
      assert observably equal (under a fixed sandbox Host for determinism)
```

A divergence is a bug in the interpreter, by definition (Article VII: the backend
is the authority). Unsupported constructs are not silently wrong: per §3.3 the
interpreter emits an explicit "not interpretable in v0.x" diagnostic and the
parity harness skips them, never compares wrong output.

## 8. Reuse map — what exists vs. what is new

```text
PIECE                       REUSES (today)                          NEW WORK
stdlib intrinsics           rsscript-runtime (all modules)          none (call them)
intrinsic dispatch table    src/runtime_abi.rs                      none (read it)
managed value model         runtime/managed.rs (Rc<RefCell>)        wrap in Value
checked program             hir.rs / syntax::ast (+ analyzer)       none (consume it)
runtime diagnostics shape   Diagnostic / parse_runtime_diagnostics  emit from spans directly
value <-> intrinsic marshalling   —                                 the bulk of the work (see below)
tree-walk evaluator               —                                 new crate `interp/`
host/sandbox boundary             runtime fs/process/... fns        new trait + in-memory impls
```

The bulk of the work is **marshalling** `Value` ↔ the Rust types each intrinsic
expects (`&str`, `i64`, `Vec<u8>`, runtime handle types). The RSScript→Rust type
mapping is a fixed, finite set, so this dispatcher can be **generated from the
same two sources of truth** — `runtime_abi.rs` (the Rust path) + the `.rssi`
signature (the RSScript types) — exactly as `tmLanguage.json` is generated from
`KEYWORDS`, with a freshness-guard test. "Generate the glue from the single
source of truth" is becoming RSScript's signature discipline; the interpreter
dispatcher is the next instance.

## 9. Phased plan

```text
P0  Tree-walk MVP for the executable subset: scalars, String/List/Map, control
    flow, Result/Option/?, match, user fns, Log/Assert. Real host only.
    Parity test green on examples/scripts/core. Wins: ms behavioral feedback.
P1  Resources (with / ResourcePool), broader stdlib coverage, runtime diagnostics
    with native RSScript spans (RS1201).
P2  Sandbox Host: in-memory FS, denied net, fixed clock, seeded RNG. Determinism
    for tests + safe agent probing.
P3  Restricted async/await via a minimal cooperative executor over async_runtime.
P4  Wire-ups: `rss eval <file>` and `rss run --interp`; make `rss test` use the
    interpreter by default; add an `rss_eval` tool to the code-agent; REPL.
P5  Generated marshalling dispatcher from runtime_abi + .rssi (replaces hand
    marshalling from P0–P3), with a freshness-guard test.
```

## 10. Non-goals / risks

- **Not a second semantics (Article VII).** The Rust backend defines behavior;
  the interpreter mirrors it. Any divergence is an interpreter bug caught by the
  parity harness — never a reason to change the language.
- **`local` optimizations are not modeled**, only their observable result —
  intentional and safe, since the optimization is unobservable.
- **Determinism** (HashMap iteration order, time, RNG, process output) must be
  pinned through the Host, or parity and agent reproducibility break.
- **Marshalling correctness** is the main hazard surface; generating it (P5) from
  the authoritative table is the mitigation.
- **Coverage honesty**: an unsupported construct must diagnose, not guess (§3.3).
  The interpreter advertises its supported subset and refuses the rest loudly.
- **Performance feedback** is *not* provided — the interpreter answers "what does
  it do," never "how fast." Benchmarks remain a backend concern.
```
