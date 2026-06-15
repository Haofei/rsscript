# tinygrad Port Feedback

Remaining items from the tinygrad RSScript/modern-c port — each removes a real
source-port workaround. Completed items are deleted as they land; see git history
for their write-ups. Done so far: generated namespace isolation (+ `use … as`
aliasing, qualified `module.fn`/`.Type`/value access, `use module.*` glob,
qualified variant patterns), pattern matching (variants/options/constants/tuples/
lists), tuples & destructuring, `#lower_name` escape hatch, source-qualified
symbol inventory, type-associated constants & static methods, value-semantics
clone/derives, Option ergonomics (`?`-on-Option + combinators), default
parameters (Copy and non-Copy), and type aliases (generic + non-generic, expanded
at every comparison site).

## Must unblock awkward valid ports

- [ ] **Method/property lowering ergonomics.** Provide a simple way to model
  Python-style getter properties and method-like computed fields.
  _Why:_ tinygrad has many small property methods where the body is simple but the
  RSS surface needs repetitive wrappers.
  _Acceptance:_ getter-style members lower predictably, can borrow `self`, and
  preserve member identity for diagnostics and inventory.

## Reduce manual translation volume

- [ ] **Container operation coverage.** Fill remaining `List`/`Map`/`Set`/
  `Bytes`/`Buffer`/tuple/optional gaps (`len`, `append`, `pop`, index, slice,
  membership, iteration, clone/copy).
  _Why:_ tinygrad code is dense with these; every missing builtin becomes a
  hand-written helper.
  _Acceptance:_ supported ops compile and lower to idiomatic Rust, or fail with an
  error naming the unsupported operation and type.

- [ ] **Callable and limited closure support.** Beyond `noescape` callbacks: pass
  *named functions* as values and support richer checked captures for callbacks
  and rewrite rules.
  _Why:_ tinygrad uses lambdas/callback matchers; without this the port must
  defunctionalize logic into many structs.
  _Acceptance:_ RSS can pass named functions and simple closures to higher-order
  helpers with checked capture ownership.

- [ ] **Construction helpers.** (Default parameters — Copy and non-Copy — are
  done.) Provide first-class defaulted-constructor / builder-style helpers so
  Python-default-heavy constructor APIs don't inflate into overload/wrapper sets.
  _Acceptance:_ defaulted construction lowers deterministically and appears in the
  symbol inventory without changing the call ABI unexpectedly.

- [ ] **String and bytes utility coverage.** Fill gaps for split/join/search,
  prefix/suffix checks, formatting, byte conversion, and cheap slicing.
  _Why:_ device/runtime/autogen and file-path code are string/bytes-heavy.
  _Acceptance:_ common string/bytes operations compile without local helper shims,
  or fail with precise unsupported-operation diagnostics.

## Tooling and diagnostics

- [ ] **RSS-to-Rust diagnostic provenance.** Source maps exist; carry the source
  span, source symbol, and lowered Rust symbol all the way through a *backend*
  Rust error.
  _Why:_ when Rust fails, the port currently has to infer which RSS declaration
  produced the bad item.
  _Acceptance:_ a Rust backend error reports the RSS file, source span, source
  symbol, and lowered Rust symbol.

- [ ] **Generated/internal helper namespace.** Keep compiler-generated helpers
  compiler-reserved/mangled so they never collide with user module/type/member
  names.
  _Why:_ helper names should never consume source-level names or force a port to
  avoid otherwise valid identifiers. (Module isolation + `#lower_name` cover
  user-symbol collisions; this is the generated-helper side.)

- [ ] **External/FFI declaration ergonomics.** Declare copied runtime/autogen and
  device boundaries compactly — `native fn` exists, but make whole boundaries easy
  to bind without large wrapper files.
  _Acceptance:_ external functions/types can be declared compactly, checked, and
  included in the symbol inventory with provenance.
