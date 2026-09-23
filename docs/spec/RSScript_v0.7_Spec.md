# RSScript Language Specification

## 1. Scope and invariants

This specification defines RSScript's syntax, types, ownership, lifetime and
resource rules, and structured asynchronous control flow. Operating-system APIs,
host permissions, deployment authorization, risk policy, and sandbox behavior
lie outside it and are defined by the host that embeds the language.

The following invariant is normative:

> Program validity, type checking, semantic lowering, and generated code are
> independent of host permissions, deployment grants, runner policy, provider
> selection, and operating-system services.

Parsing and compilation therefore take exactly two inputs: the source and the
interfaces it declares. External implementation selection happens after
compilation.

Each construct has one spelling. The language keeps no compatibility aliases:
a removed spelling is rejected, not accepted as a synonym.

Examples are complete files. Each is labelled **Accepted**, or
**Rejected — `CODE`** with the diagnostic it emits, and
`crates/rsscript-sdk/tests/spec_examples.rs` checks every one; an
`rsscript-interface` block is a companion interface for the examples after it
in its section. [RSScript Semantics v0.7](RSScript_Semantics_v0.7.md) states
the same conventions and carries the detail this document leaves out.

## 2. Files and declarations

`.rss` files are implementation files. Every ordinary function declared in an
implementation file has a body. `.rssi` files are interfaces; their functions are
bodyless declarations. A bodyless interface function is an external symbol, not
a distinct function kind in the language AST.

```rsscript-interface
struct Image {
    width: Int,
    height: Int
}

pub fn Image.resize(image: take Image, width: Int) -> fresh Image
```

**Accepted** — an implementation file checked against that interface

```rsscript
module image.pipeline

pub fn thumbnail(image: take Image) -> fresh Image {
    return Image.resize(image: take image, width: 128)
}

fn main() -> Unit {
    local image = Image(width: 640, height: 480)
    let small = thumbnail(image: take image)
    Output.write(message: Int.to_string(value: small.width))
    return Unit
}
```

Top-level file feature and profile declarations are not grammar productions.
Implementation-origin markers are not declaration modifiers.

`pub` is a module boundary, not a hint. A declaration without `pub` is visible
only inside the module that declares it: another module may neither import it
nor name it through a module-qualified path, and a glob import binds only the
declaring module's `pub` names. Interface (`.rssi`) declarations are exempt —
an interface is a host contract whose surface is governed by the package
contract rather than by module visibility.

## 3. Types and protocol dispatch

The core scalar and structural types include `Unit`, `Bool`, `Int`, `Float`,
`String`, structs, classes, sum types, generics, resources, collections, and
function types.

Protocols define explicit method contracts. `Dyn<P>` is the dynamic protocol
dispatch type for protocol `P`; it does not represent permission or authority.

**Accepted**

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

fn render_dynamic(value: read Dyn<Render>) -> fresh String {
    return Render.render(self: value)
}
```

## 4. Data effects and ownership

Parameters have a closed data-effect set:

- `read`: the call may observe but not mutate or consume the argument;
- `mut`: the call may mutate and writes propagate to the caller;
- `take`: the call consumes the argument and later use is invalid.

These are type/ownership semantics, not host effects.

`local` creates an exclusive local value. `manage` moves a valid local graph into
managed storage. The checker rejects use after move, conflicting places,
managed-to-local leakage, and illegal escape. These constructs need no file-level
enablement.

`fresh T` states that a returned value does not alias caller-visible mutable
state. `noescape Fn(...)` prevents a callback from escaping its call. `owned`
records owned function/value forms where specified by the type system.

## 5. Structured retention

Retention is the only source declaration contract in this family because it
affects escape checking. It is represented directly on a function declaration.

**Accepted**

```rsscript
class Cache {
    values: List<String>
}

fn Cache.put(cache: mut Cache, value: String) -> Unit
    retains(value)
{
    List.push(list: mut cache.values, value: value)
    return Unit
}

fn main() -> Unit {
    let cache = Cache(values: [])
    Cache.put(cache: mut cache, value: "entry")
    return Unit
}
```

Each retained name must identify a declared non-Copy parameter. The AST stores
retained parameter names, and HIR resolves them to parameter identity. Arbitrary
string declaration effects and source purity assertions do not exist. Purity and
parallelism are inferred from validated source or supplied by provider metadata.

## 6. Resources

`resource` values have linear lifetime rules. `with` introduces a bounded
resource scope and guarantees cleanup on every exit. Resources may not escape
their scope unless the type and ownership rules explicitly permit the transfer.
Handle and weak-reference rules continue to govern managed object graphs.

A resource slot is host-owned: it is acquired at the external boundary and
released by the runtime. An implementation file may *declare* a `resource` type,
its fields, and its `drop` body, but may not construct one — every resource is
produced by a bodyless function declared in an interface, which is where the
host's cleanup contract exists.

```rsscript-interface
resource Connection {
    id: Int
}

pub fn Connection.open(address: String) -> Connection

pub fn Connection.send(connection: mut Connection, message: String) -> Unit
```

**Accepted** — the interface produces the resource; `with` bounds its lifetime

```rsscript
fn greet(address: String) -> Unit {
    with Connection.open(address: address) as connection {
        Connection.send(connection: mut connection, message: "hello")
    }
    return Unit
}
```

**Rejected — `RS0702`** — an implementation file cannot construct a resource

```rsscript
resource Handle {
    id: Int
}

fn make(id: Int) -> Handle {
    return Handle(id: id)
}
```

## 7. Asynchronous control flow

`async fn`, `await`, `task_group`, `async let`, `select`, channels,
cancellation, and streams are language/runtime-core constructs. They require no
file header. Task lifetimes remain structured and cancellation propagates through
the owning scope.

Wall-clock timers, sockets, asynchronous files, and subprocesses are host
services and must arrive through explicit packages/providers.

## 8. Core interfaces

The default single-file environment exposes only platform-neutral deterministic
interfaces. It does not implicitly expose files, directories, environment
variables, HTTP, sockets, processes, temporary directories, wall clocks, system
randomness, logging, command-line arguments, or OS handles.

Pure path manipulation may be provided only with specified cross-platform lexical
semantics. Ambient path queries and I/O belong to a host filesystem package.
Durations may be pure values; reading a clock belongs to a host time package.
Deterministic PRNGs require an explicit seed; system entropy belongs to a host
random package.

The generated [core-interface catalog](../generated/core-interfaces.md) names
the exact current interface files. Its companion
[language card](../generated/language-card.md) is a non-normative, source-backed
quick reference for lexical keywords and diagnostics; this specification and
the parser remain authoritative.

## 9. External symbols and bindings

A bodyless `.rssi` function introduces an external symbol. Package binding
metadata maps the symbol to a provider implementation:

```toml
schema = "rsscript.bindings.v1"

[[function]]
symbol = "host.net.http.get"
provider = "rsscript_host_http"
entry = "http_get"
review_effects = ["network.client"]
```

The optional `review_effects` field belongs to binding metadata and is not parsed
as RSScript. Linking diagnoses missing providers, duplicate bindings, and ABI
mismatches. The frontend does not diagnose host authorization.

VM lowering uses an external-call instruction containing a stable symbol/binding
identity. At execution, an external-function registry resolves that identity.
Execution control contains cancellation, deadline, budgets, output bounds, and
trace context only.

## 10. Packages and analysis artifacts

Package build-selection features may exist in `rsspkg.toml`; they are unrelated
to language syntax or host permission. The platform-neutral analysis artifact
uses schema `rsscript.package_analysis.v1` and contains diagnostics, exports,
semantic summaries, retention facts, resource facts, async facts, and external
symbols. It contains no host grants.

Binding/provider review is optional and consumes the validated call graph plus
binding metadata. Review tooling and deployment policy must not influence validation or
lowering.

## 11. Execution

Execution limits include step, memory, intrinsic-call, host-call, output,
recursion, cancellation, and deadline controls. Limits protect availability and
embedding stability. They are not proof of isolation and must not be described
as language authority or a sandbox.

Compilation produces a platform-neutral executable. The compile API has no host,
deployment, or permission argument. A runner executes the same artifact with an
`ExternalFunctionRegistry` and `ExecutionControl`/limits.

`CallExternal` contains stable symbol identity, arguments, destination, and
mutation write-back positions. An unresolved symbol is a link/execution error.
Provider choice must not alter parsing, validation, HIR, lowering, or the compiled
artifact.

Execution control includes cancellation, deadline, cumulative allocation budget,
exact reachable-value live-memory limit, external-call budget, output bounds,
and recursion bounds. The live metric deduplicates shared heap nodes and subtracts
unreachable values at instruction boundaries; it excludes allocator metadata,
generated code, and Provider-owned memory, so it is deterministic across hosts.
Execution control does not include a language authority model. Resource slots are opaque and provider-owned at the
external boundary; cleanup occurs on normal return, error, cancellation, and
deadline exit.

The VM core executes arithmetic, comparison, collections, strings, control flow,
closures, type construction, resources, structured scheduler primitives, and
external calls. Operating-system behavior is supplied by explicit providers.

### Entry point

An entry point is the function named `main`. It must declare **either no
parameters, or exactly one parameter whose type is `List<String>`**, which
receives the program arguments; any other arity, or a single parameter of any
other type, is rejected before the first instruction runs
(`crates/rsscript-vm/src/reg_vm/scheduler.rs::run_program`). The return type is
unconstrained: the runner returns `main`'s value, and the execution report
carries it as a typed wire value when the declared return type parses as one and
omits it otherwise
(`crates/rsscript-vm/src/reg_vm/executable.rs::main_result_wire_value`). An
artifact with no `main` is a link error. The front end imposes no signature
constraint on `main` and accepts a file with none, so this is the runner's half
of the contract.

### Evaluation and arithmetic

- A call's arguments are evaluated left to right **as written at the call site**,
  not in parameter-declaration order: the receiver first, then every explicit
  argument in source order whatever parameter it names, then each omitted
  defaulted parameter's default expression in declaration order
  (`crates/rsscript-semantics/src/call_binding.rs`; lowering consumes the
  resulting `evaluation_index`).
- `+`, `-`, `*`, `/`, `%` on `Int` are **checked**: overflow, division by zero,
  and modulo by zero are language-level runtime errors, never wrapping and never
  a host panic (`crates/rsscript-vm/src/reg_vm/value_ops.rs`). `Int` is 64-bit
  signed. Wrapping arithmetic is opt-in through `Math.wrapping_*`.
- `Float` arithmetic is IEEE-754 and does not trap: overflow yields an infinity
  and `0.0 / 0.0` yields `NaN`. `%` on `Float` is a runtime error.

### Structured concurrency and cleanup

These rules affect program meaning and are checked independently of provider
choice or runtime limits:

- `async let` is legal only inside its lexical `task_group`.
- Every named child handle is consumed by exactly one `await`; it cannot be
  awaited before declaration, awaited twice, or referenced after group exit.
- `async let _` creates a scoped background child. The group drains it before
  the scope can return, so no child silently outlives its parent.
- `Task.cancellation_token()` requires a lexical task-group owner. Cancellation
  propagates through that owner instead of creating an unstructured global task.
- A `resource` and a live `local` value cannot cross an `await`. A local may be
  moved with `take` into the awaited operation when its signature permits it.
- A parameter marked `retains(param)` is an escape even when the call is async;
  local/resource values cannot be hidden behind that suspension boundary.
- Normal return, `?` propagation, provider error, deadline, and cancellation
  converge on the same resource-slot cleanup path. A provider-owned resource is
  released according to its declared cleanup contract.
- Cancelled channel send/receive operations do not publish a partial transfer;
  channel closure remains observable through the ordinary result contract.
- `select` tie-breaking is by source order: among the arms that have finished
  when the `select` resolves, the earliest-written arm wins, deterministically
  and independently of completion timing, task ids, and hashing
  (`crates/rsscript-vm/src/reg_vm/scheduler.rs::resolve_wait`). Source order
  only decides between arms that are already ready; it is not a priority over
  arms that are not.
- Tasks start in creation order from a FIFO ready queue and run one at a time.
  When one event makes several parked tasks runnable, they resume in creation
  order, deterministically
  (`crates/rsscript-vm/src/reg_vm/scheduler.rs::satisfy_waiters`). No fairness
  bound is promised: a task runs until it suspends or completes.

Provider descriptors additionally state whether an external function is
cooperative, abort-safe, or not cancellation-aware, and whether it may block.
The runtime validates the semantic signature and ABI before executing the first
instruction; deployment metadata cannot weaken these language rules.

Conformance anchors:

| Rule group | Anchor |
| --- | --- |
| `async let` scoping and exactly-once handle consumption | `crates/rsscript-semantics/src/task_groups.rs` (module tests) |
| `await` placement, values live across an `await`, `Task.cancellation_token()` ownership | `crates/rsscript-semantics/src/await_placement.rs` (module tests) |
| resource escape from a `with` scope, and resource producers | `crates/rsscript-semantics/src/resource_flow.rs`, `crates/rsscript-semantics/src/resource_producers.rs` |
| structured task lifecycle at runtime (created / completed / cancelled / peak-live counts) | `crates/rsscript-sdk/src/tests.rs::execution_usage_reports_structured_task_lifecycle` |
| scheduler cancellation | `crates/rsscript-vm/src/reg_vm/scheduler.rs` (module tests) |
| `select` tie-breaking | `crates/rsscript-vm/src/reg_vm/scheduler.rs::a_select_with_several_finished_arms_picks_the_earliest_written_one`, `crates/rsscript-sdk/src/tests.rs::select_tie_breaking_picks_the_first_written_ready_arm` |
| wake order after an event | `crates/rsscript-vm/src/reg_vm/scheduler.rs::one_event_wakes_parked_tasks_in_creation_order` |
| the entry-point signature | `crates/rsscript-vm/src/reg_vm/scheduler.rs::run_program` (implementation; no dedicated test yet) |
| checked `Int` arithmetic | `crates/rsscript-vm/src/reg_vm/value_ops.rs::eval_numeric_binary` |
| call-argument evaluation order | `crates/rsscript-semantics/src/call_binding.rs` (module tests) |
| exactly-once resource cleanup on every terminal path | `crates/rsscript-sdk/tests/execution_state_corpus.rs` |

The optional native JIT consumes an in-process, non-serialized IR and is released
in lockstep with the VM. It is not an Artifact format and carries no independent
compatibility promise. Its only FFI boundary is the explicitly versioned call
frame described in [`native-jit-contract.md`](native-jit-contract.md).

## See also

[RSScript Semantics v0.7](RSScript_Semantics_v0.7.md) is the source-backed
companion reference: it derives each rule in this specification from the
implementing file in `crates/rsscript-syntax` and `crates/rsscript-semantics`,
gives a verified accepted and rejected example for every rule, states explicitly
where behaviour is unspecified, and indexes every `RS` diagnostic code to the
rule it enforces.
