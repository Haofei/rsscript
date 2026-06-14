# tinygrad Port Feedback

These items came from the tinygrad RSScript/modern-c port. They are not abstract
language wish-list items: each one removes a current source-port workaround or
manual wrapper.

## Must unblock awkward valid ports

- [ ] **Generated namespace isolation.** Preserve source/module/type/member
  identity through checking and lowering, then emit globally unique Rust symbols.
  _Why:_ valid tinygrad names such as `helpers.count`, `device.count`,
  `helpers.T`, and `tensor.T` currently collide because RSS lowers too many
  things into one Rust item namespace. The port has to invent names like
  `helpers_count` and map them back in tooling.
  _Acceptance:_ two modules can define the same final name; a module-level value,
  type alias, struct, method, and generated helper with related names cannot
  collide after Rust lowering; diagnostics report the RSS source symbol, not only
  the lowered Rust name.

- [ ] **Stable source-qualified symbol identity.** Store and expose a symbol's
  module path, source qualified name, kind, visibility, and lowered backend name.
  _Why:_ port tooling should compare `helpers.py::count` to
  `helpers.rss::count`, not guess from bare names.
  _Acceptance:_ RSS can emit a checked symbol inventory suitable for portman:
  `module`, `qualname`, `kind`, `source_span`, `lowered_name`.

- [ ] **Class/static member support.** Model class attributes, associated
  constants, and static methods directly.
  _Why:_ tinygrad uses symbols such as `Tensor.train`, `Device.DEFAULT`,
  `dtypes.float`, `UOp.const`, and `UPat.var` as class/static members. Porting
  them as free functions or dummy globals loses structure and creates name
  conflicts.
  _Acceptance:_ RSS supports declaring and lowering type-associated values and
  methods without requiring separate global wrappers.

- [ ] **Type aliases and generic aliases without value placeholders.** Add
  source-visible type alias declarations that do not create value namespace
  entries unless explicitly requested.
  _Why:_ Python `TypeVar` and alias-like symbols currently become dummy constants
  in the port just so coverage tooling can see them.
  _Acceptance:_ aliases can be generic, can reference imported types, and appear
  in the symbol inventory as type-level symbols.

- [ ] **Method/property lowering ergonomics.** Provide a simple way to model
  Python-style getter properties and method-like computed fields.
  _Why:_ tinygrad has many small property methods where the implementation is
  simple but the RSS surface needs repetitive wrappers.
  _Acceptance:_ getter-style members lower predictably, can borrow `self`, and
  preserve member identity for diagnostics and inventory.

## Reduce manual translation volume

- [ ] **Container operation coverage.** Make common `List`, `Map`, `Set`,
  `Bytes`, `Buffer`, tuple, and optional operations straightforward and
  consistently checked.
  _Why:_ tinygrad code is dense with `len`, `append`, `pop`, indexing, slicing,
  membership, iteration, and clone/copy patterns. Every missing builtin creates a
  hand-written helper.
  _Acceptance:_ supported operations either compile and lower to idiomatic Rust
  or fail with an error that names the unsupported operation and type.

- [ ] **Clone/copy consistency for builtin value types.** Continue the
  `List`/`Bytes`/`Buffer` `.clone()` direction and make the checker/lowerer agree
  exactly on every builtin clone target.
  _Why:_ the tinygrad port previously hit Rust `E0425` because the checker
  accepted a builtin clone that lowering emitted as a missing helper.
  _Acceptance:_ checker-approved builtin clones always lower to valid Rust, and
  checker-rejected clones state the supported alternatives.

- [ ] **Option/null ergonomics.** Add concise checked forms for option tests,
  unwrap-with-default, early return on none, and optional field access.
  _Why:_ tinygrad has many maybe-present values; verbose option plumbing makes
  direct ports drift away from the source.
  _Acceptance:_ common option patterns require no custom helpers and preserve
  ownership/borrow rules clearly.

- [x] **Pattern matching over variants, options, constants, tuples, and lists.**
  _Why:_ the UOp/UPat rewrite system is pattern-heavy. Lowering it as nested
  conditionals would be slow to write and hard to audit.
  _Acceptance:_ match arms can bind values, match enum/variant tags, constants,
  tuples, and optional values; exhaustiveness or explicit fallback is checked.
  _Done:_ variants, options, constants, tuple, and list patterns now match at
  VM/compiled parity. List slice patterns support `[]`, fixed-length `[a, b]`,
  head/rest `[first, ..rest]`, tail `[..init, last]`, and middle-rest
  `[a, ..mid, z]` (element bindings come out owned; the rest binding is a
  `List<T>`); the VM lowers them to a length test plus `ListGet`/`List.slice`,
  the compiled backend to native Rust slice patterns with owned rebindings.
  Exhaustiveness understands list lengths (`[]` + `[x, ..rest]` is exhaustive).
  Plain (non-variant) generic struct patterns and generic field/binding types
  resolve their element types from the scrutinee's arguments. See
  `tests/fixtures/pass/list_patterns.rss` and the `parity_list_match` test.

- [ ] **Callable and limited closure support.** Support function values and
  simple captures well enough for callbacks and rewrite rules.
  _Why:_ tinygrad uses lambdas/callback-like matchers. Without callable support,
  the port must manually defunctionalize logic into many structs.
  _Acceptance:_ RSS can pass named functions and simple closures to higher-order
  helpers with checked capture ownership.

- [ ] **Default arguments and construction helpers.** Provide a first-class way
  to express common defaulted constructor/helper parameters.
  _Why:_ tinygrad APIs use many Python defaults. Manual overload/wrapper sets
  inflate the port.
  _Acceptance:_ defaulted parameters lower deterministically and appear in docs
  or symbol inventory without changing the call ABI unexpectedly.

- [x] **Tuple and destructuring ergonomics.** Make multiple-return and unpacking
  patterns concise.
  _Why:_ tinygrad frequently returns and unpacks grouped values.
  _Acceptance:_ tuple literals, tuple returns, and local destructuring work with
  ownership and borrowing tracked normally.
  _Done:_ tuples desugar to synthetic `__TupleN` generic structs at parse time —
  literals `(a, b)`, types `(Int, String)`, `.itemN` access, tuple patterns in
  `match`, and `let (a, b) = expr` destructuring (`_` skips an element), all at
  VM/compiled parity and round-tripped by the formatter. See
  `tests/fixtures/pass/tuples.rss` and the `parity_tuple_*` parity tests.

- [ ] **String and bytes utility coverage.** Fill gaps for split/join/search,
  prefix/suffix checks, formatting, byte conversion, and cheap slicing where
  supported.
  _Why:_ device/runtime/autogen and file-path code are string/bytes-heavy.
  _Acceptance:_ common string/bytes operations compile without local helper
  shims, or fail with precise unsupported-operation diagnostics.

## Tooling and diagnostics

- [ ] **RSS-to-Rust diagnostic provenance.** Carry source spans and symbol
  identity into generated Rust and back through compiler errors.
  _Why:_ when Rust fails, the port currently has to infer which RSS declaration
  produced the bad item.
  _Acceptance:_ a Rust backend error reports the RSS file, source span, source
  symbol, and lowered Rust symbol.

- [x] **Explicit lowered-name escape hatch.** Add an attribute for rare cases
  where a declaration needs a pinned backend name.
  _Why:_ useful during compiler transition and for FFI/autogen boundaries.
  _Acceptance:_ something like `#[lower_name = "helpers__count"]` is checked for
  uniqueness and reflected in the symbol inventory.
  _Done:_ functions accept `#lower_name("...")` (following the existing
  `#deprecated("...")` attribute convention). The pin is validated (RS0035: must
  be a valid Rust identifier and unique across functions' final backend names),
  honored by the lowerer at the definition and every call site (via a per-run
  override consulted by the canonical name-lowering helpers), reflected in the
  symbol inventory's `lowered_name`, and round-tripped by the formatter. See
  `tests/fixtures/pass/lower_name_attribute.rss`, `fixtures/fail/lower-name-*`,
  and `lower_name_pin_renames_definition_and_call_sites`.

- [ ] **Generated/internal helper namespace.** Keep compiler-generated helpers
  separate from user declarations.
  _Why:_ helper names should never consume source-level names or force a port to
  avoid otherwise valid identifiers.
  _Acceptance:_ generated helper names are always compiler-reserved/mangled and
  never collide with user-visible module/type/member names.

- [ ] **External/FFI declaration ergonomics.** Make copied runtime/autogen and
  device boundaries easy to declare without large wrapper files.
  _Why:_ tinygrad runtime/autogen should mostly be copied or bound, not manually
  reimplemented.
  _Acceptance:_ external functions/types can be declared compactly, checked, and
  included in the symbol inventory with provenance.
