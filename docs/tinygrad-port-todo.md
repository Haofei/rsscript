# tinygrad Port Feedback

**No open items** — every workaround the tinygrad RSScript/modern-c port reported
has been addressed. This file is kept as a record of what landed; see git history
for each feature's write-up.

## Done

- Generated namespace isolation (+ `use … as` aliasing, qualified
  `module.fn`/`.Type`/value access, `use module.*` glob, qualified variant
  patterns).
- Pattern matching (variants / options / constants / tuples / lists).
- Tuples & destructuring.
- `#lower_name` backend-name escape hatch.
- Source-qualified symbol inventory.
- Type-associated constants & static methods.
- Value-semantics clone / derives.
- Option ergonomics (`?`-on-`Option` + combinators).
- Default parameters (Copy and non-Copy).
- Type aliases (generic + non-generic, expanded at every comparison site).
- Reserved `__rss_`/`__rsscript_` namespaces for compiler-generated helpers
  (Python-style dunders like `__hash__` stay legal).
- Struct field defaults (`name: T = expr`, filled at construction on both
  backends).
- Diagnostic provenance (remapped backend errors name the RSS source symbol and
  its lowered Rust symbol).
- Named function values (a named function passed where a `Fn(...)` is expected
  desugars to a forwarding closure, identical on both backends).

## Verified already-met (closed without new code)

- **Method/property getters** — zero-argument methods
  (`fn Box.value(self: read Box) -> Int`) called via receiver-call shorthand
  (`read b.value()`); lower predictably, borrow `self`, keep member identity.
- **Container / string-bytes coverage** — broad `List`/`Map`/`Set`/`String`/
  `Bytes` operations exist; any unsupported op fails with a precise op+type
  diagnostic (`RS0206`). Specific further ops are added on demand.
- **External/FFI ergonomics** — `native fn` declares external boundaries
  compactly, is checked, and appears in the symbol inventory with provenance.
