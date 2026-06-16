# Cross-isolate message API — design & feasibility (spec §20.1-B / §20.2-3)

Status: **message-channel core implemented; multi-isolate execution still future.**
The reviewable essence — the static **message payload contract** — has landed:
`Channel.message<T>` creates a channel whose payload `T` is statically verified
cross-isolate-transferable (`is_cross_isolate_transferable`; v1 allows Copy
scalars, `String`, `Bytes`), rejecting managed/container payloads at check time
(`RS0036`). It reuses the bounded-channel runtime, so it runs today at VM↔compiled
parity and is the forward-compatible transport for real isolates. The broader
payload contract and true multi-heap isolate-spawn remain blocked on foundational
runtime work (the spec gates that on "the isolate model maturing first"). This
note records the investigation and the smallest sound plan for the remaining work.

## Goal (from the spec)

Typed send/receive channels **between isolates**: payloads are owned/Copy data or
values moved with `take`; **managed handles never cross**; single ownership is
enforced statically (not by runtime convention); `take`-based move is the
no-shared-alias transfer path (zero-copy when representation permits).

## Why true multi-heap isolates aren't a small change

The static payload contract above is built, but a genuine *second isolate* is not:
the runtime is **strictly single-isolate**. Concurrency is cooperative tasks on
one thread sharing **one heap**:

- `crates/runtime/src/channel.rs` is an explicit "single-isolate MPSC" using
  `Rc<RefCell<ChannelState<T>>>` (no `Send`/`Sync`, no `Arc<Mutex>`).
- The reg-VM scheduler (`reg_vm/mod.rs`: `tasks`, `ready_queue`, `Wait`,
  `satisfy_waiters`) drives many tasks, but they share the same `Rc` pool —
  `task_group` children are isolate-**local**.
- There is no construct that creates a second execution context with its own heap;
  a "second isolate" would still alias the same managed `Rc`s.

So "managed handles never cross isolates" has no boundary to be enforced *at*
today — there is only one heap.

## Smallest sound slice (the implemented core, plus the remaining steps)

Reuse the cooperative scheduler; make the isolate boundary a **static + runtime
value-transfer contract** rather than a separate OS thread/heap. Soundness comes
from restricting payloads so a value transfer is a genuine deep copy with no
shared `Rc`. Steps 1, 3, and 4 below shipped as the v1 message-channel core; the
broadened classification (step 2) and the dedicated mailbox surface remain open.

1. **Type classification — `is_cross_isolate_transferable(T)`** (`checks/local.rs`)
   — _v1 landed._ Allowed today: Copy scalars, `String`, and `Bytes` (self-contained
   values with no managed handle). Denied: `List`/`Map`/`Set`, closures, any managed
   handle, resources, views, generic `T`. This is the reviewable essence: no `Rc`
   can cross, so a clone/move is alias-free.

2. **Broaden the classification** _(remaining)_ — extend transferability to owned
   `struct`/`sum` values whose fields are all transferable, and to owned containers
   via deep-snapshot. Keeps the same no-`Rc`-crosses guarantee.

3. **Static enforcement** — _landed._ At the send site the checker infers the
   payload type and rejects a non-transferable `T` with the stable diagnostic
   `RS0036` (mirrors `take` validation). This is the primary defense and is fully
   testable without multi-threading.

4. **Execution** — _landed (reusing the bounded-channel runtime)._ The v1 surface
   is `Channel.message<T>`, kept on the in-isolate channel runtime so endpoints run
   as cooperative tasks and transfer reuses the existing channel value-move (VM
   clones `VmValue`, Rust moves `T`). Because the payload has no `Rc`, this is
   alias-free on the shared heap — sound for the "single ownership / no shared
   managed handle" guarantee. A future structured isolate-spawn would run the two
   endpoints with disjoint heaps; a defensive runtime scan rejecting any
   `Rc`-bearing value remains a useful safety net for edge cases.

5. **Parity + tests** — `parity_message_channel_roundtrip` already covers the v1
   core: Copy/`String`/`Bytes` payloads cross, managed payloads are rejected at
   check time (`RS0036`), and it mixes with in-isolate channels at parity.

## Larger, deferred option

True multi-thread / multi-heap isolates (`Arc<Mutex>`, `Send + Sync`, a
thread-multiplexing scheduler) is a multi-quarter refactor that intentionally
breaks the single-thread-simplicity design. Out of scope; the slice above delivers
the spec's *static* guarantee without it.

## Decision

The static contract (steps 1+3) is the high-value, sound, bounded core, and it is
**built**: `Channel.message<T>` enforces `is_cross_isolate_transferable` with
`RS0036` and runs at parity (step 4). The remaining work is broadening the payload
classification (step 2) and a structured isolate-spawn with disjoint heaps; pick
those up when prioritized. Until then the single-isolate model stands and managed
handles simply cannot be sent anywhere.
