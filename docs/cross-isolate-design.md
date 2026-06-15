# Cross-isolate message API — design & feasibility (spec §20.1-B / §20.2-3)

Status: **not implemented — blocked on foundational runtime work** (the spec gates
it on "the isolate model maturing first"). This note records the investigation and
the smallest sound implementation plan so the work can be picked up directly.

## Goal (from the spec)

Typed send/receive channels **between isolates**: payloads are owned/Copy data or
values moved with `take`; **managed handles never cross**; single ownership is
enforced statically (not by runtime convention); `take`-based move is the
no-shared-alias transfer path (zero-copy when representation permits).

## Why it isn't a small change today

The runtime is **strictly single-isolate**. Concurrency is cooperative tasks on
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

## Smallest sound slice (recommended, ~1–2 weeks; holds VM↔compiled parity)

Reuse the cooperative scheduler; make the isolate boundary a **static + runtime
value-transfer contract** rather than a separate OS thread/heap. Soundness comes
from restricting payloads so a value transfer is a genuine deep copy with no
shared `Rc`.

1. **Type classification — `is_cross_isolate_safe(T)`** (`checks/local.rs`).
   Allowed: Copy scalars, and owned `struct`/`sum` values whose fields are all
   cross-isolate-safe. Denied: `List`/`Map`/`Set`, closures, any managed handle,
   resources, views, generic `T`. This is the reviewable essence: no `Rc` can
   cross, so a clone/move is alias-free.

2. **Surface** — a distinct mailbox API (e.g. `Mailbox<T>` + `Mailbox.pair()`,
   `Outbox.send(value: take T)`, `Inbox.recv()`), feature-gated, with
   `send` requiring `take` of a cross-isolate-safe `T`. Kept separate from the
   in-isolate `Channel<T>` so existing channel semantics are unchanged.

3. **Static enforcement** — at `Outbox.send`, infer the payload type and reject
   non-cross-isolate-safe `T` with a stable diagnostic (mirrors `take`
   validation). This is the primary defense and is fully testable without
   multi-threading.

4. **Execution** — run the two endpoints as cooperative tasks; transfer reuses the
   existing channel value-move (VM clones `VmValue`, Rust moves `T`). Because the
   payload has no `Rc`, this is alias-free on the shared heap — sound for the
   "single ownership / no shared managed handle" guarantee. Add a defensive
   runtime scan rejecting any `Rc`-bearing value (safety net for edge cases).

5. **Parity + tests** — `tests/vm_eval_parity/async_concurrency.rs`: Copy payloads
   cross; managed payloads rejected at check time; ordering preserved; mixing with
   in-isolate channels.

## Larger, deferred option

True multi-thread / multi-heap isolates (`Arc<Mutex>`, `Send + Sync`, a
thread-multiplexing scheduler) is a multi-quarter refactor that intentionally
breaks the single-thread-simplicity design. Out of scope; the slice above delivers
the spec's *static* guarantee without it.

## Decision

Recorded, not built. The static contract (step 1+3) is the high-value, sound,
bounded core; steps 2/4/5 add the executable surface. Pick up when the mailbox
surface is prioritized; until then the single-isolate model stands and managed
handles simply cannot be sent anywhere.
