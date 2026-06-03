# TODO — working list (check off / delete as you go)

_Ephemeral. Not a roadmap; delete items when done, delete the file when empty._
_Every item says **why** — if you can't fill in a why, it doesn't belong here._

## Parked (recorded so we don't re-derive; not now)
- Int extras (`to_hex`/`from_hex`, bit ops) stay deferred until a self-hosted stage actually needs them. _Why:_ RSS source has no hex literals today; don't add speculatively — thinness.
- **Stream / lazy Iter** = the deferred performance path. Blueprint: a lazy iterator is `struct Iter[X] { next: () -> Option<X> }`, combinators wrap the next-closure → lazy fusion, zero intermediates. _Why parked / why noted:_ it's the "Stream for performance" we split off from the eager Pipeline; record the design so it's ready, **with the review-first caveats**: it's mutate-on-read / consume-once (hidden-state hazard), and prefer explicit loops over callback-iteration (even mature lazy-iterator libraries are moving away from it).
- Keep the `try_` surface **small** (don't generate try_ variants; let pipeline's `FalliblePipeline` cover the common case). _Why:_ error-polymorphism (inferring fallibility from the callback) would collapse the `try_` surface, but RSS pays explicit fallibility with duplication, so the price must stay bounded (Article II).
- `read` omittable-default decision (§2A.3, recorded); pattern extras (`|`, range, `as`/`@`); **map-pattern via a `get(Self,K)->Option<V>` protocol** (defers protocol-dispatched patterns — much heavier semantics than struct/sum, and the interpreter doesn't need it); shared Iter protocol to collapse ×collections. _Why parked:_ not on the dogfooding critical path; revisit when a real program demands them.
