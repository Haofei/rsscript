# tinygrad Port Feedback

Remaining items from the tinygrad RSScript/modern-c port — each removes a real
source-port workaround. Completed items are deleted as they land; see git history
for their write-ups. Done so far: generated namespace isolation (+ `use … as`
aliasing, qualified `module.fn`/`.Type`/value access, `use module.*` glob,
qualified variant patterns), pattern matching (variants/options/constants/tuples/
lists), tuples & destructuring, `#lower_name` escape hatch, source-qualified
symbol inventory, type-associated constants & static methods, value-semantics
clone/derives, Option ergonomics (`?`-on-Option + combinators), default
parameters (Copy and non-Copy), type aliases (generic + non-generic, expanded
at every comparison site), reserved `__rss_`/`__rsscript_` namespaces for
compiler-generated helpers (Python-style dunders like `__hash__` stay legal), and
struct field defaults (`name: T = expr`, filled at
construction on both backends), and diagnostic provenance (remapped backend
errors name the RSS source symbol and its lowered Rust symbol).

Verified already-met against their written acceptance (closed without new code):
- **Method/property getters** — modeled as zero-argument methods
  (`fn Box.value(self: read Box) -> Int`) called via receiver-call shorthand
  (`read b.value()`); they lower predictably, borrow `self`, and keep member
  identity in the symbol inventory.
- **Container / string-bytes coverage** — broad `List`/`Map`/`Set`/`String`/
  `Bytes` operations exist, and any unsupported op fails with a precise
  op+type diagnostic (`RS0206: call to \`List.frobnicate\` does not resolve`).
  Specific further ops are added on demand when the port names them.
- **External/FFI ergonomics** — `native fn` declares external boundaries
  compactly, is checked, and appears in the symbol inventory with provenance.

## Remaining

- [ ] **Callable: named function values.** Passing a *named function* as a
  callback value is rejected (`RS0026: unknown value binding`); only inline
  closures work. Allow an identifier that names a top-level function to be passed
  where a `Fn(...)` / `noescape Fn(...)` parameter is expected, checked against the
  function's signature and lowered as a function value.
  _Why:_ tinygrad passes named matcher/rewrite functions; without this the port
  wraps each in a closure or struct.
