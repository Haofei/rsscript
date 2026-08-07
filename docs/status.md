# Status

The platform-neutral language cut is active:

- legacy file headers, generic declaration effects, source implementation-origin
  markers, and source unsafe boundaries are removed;
- retention is a structured declaration field;
- dynamic protocol dispatch uses `Dyn<P>`;
- default core interfaces contain no host service modules;
- external package functions lower through `CallExternal` and are resolved by an
  execution-time registry;
- provider runtime values and callables are owned by the safe provider API; the
  native ABI adapts that model instead of defining it;
- execution/deployment policy types and legacy API facade are removed;
- neutral package analysis uses `rsscript.package_analysis.v1`; optional review
  output uses the distinct `rsscript.package_review.v1` schema; workspace
  snapshots now derive analysis directly from captured semantic inputs instead
  of converting a risk-oriented package review.

The `rsscript-compiler` implementation still contains analyzer orchestration,
package tooling, VM, and AOT lowering while those
remaining boundaries are migrated. Stable embedders use the small
`rsscript-sdk` façade instead of those implementation modules; the compiler
does not depend back on that façade.
Native plugin loading and guarded child-process execution have been removed
from the compiler; the CLI composition root owns its bounded AOT subprocesses.
The experimental Rust AOT lowering path no longer special-cases filesystem,
process, network, wall-clock, or OS-handle types; external host calls remain
provider boundaries instead of mapping back to the retired runtime façade.
Its generated-code ABI likewise exposes only pure `Duration` arithmetic, not
ambient clock reads, script deadlines, or timer sleeps. Monotonic deadlines stay
in the explicit `host` control module for bounding an execution.
The compiler now consumes runtime `core` only; VM
compatibility intrinsics for filesystem and process access fail with an explicit
provider-required error instead of reaching the OS. Disabled hosts fail before
build, spawn, or dynamic loading when those integrations are requested.
REIR conversion lives in the one-way `integrations/rsscript-review-reir`
adapter and is absent from normal compiler dependencies, public compiler APIs,
CLI package output, and package metadata writes. The retired policy-oriented
examples and action have been removed. The LSP now consumes the document-oriented
`rsscript-language-service` API with the frontend-only feature set. Its dependency
closure contains no runtime, provider, bytecode, JIT, native ABI, process guard,
HTTP, WebSocket, or REIR package. The lexer, parser, source AST, syntax
desugarings, spans, and
bounded parse budget are now owned by the independent `rsscript-syntax` and
`rsscript-work-budget` crates. Structural types, type interning, substitution,
parameter effects, package-wide semantic type facts, Typed HIR, call binding, and
HIR construction are now owned by `rsscript-semantics`. The platform-neutral core
and standard-package interface sources are owned by the data-only
`rsscript-interface-catalog`. These are re-exported through the compatibility
façade while the remaining checks are migrated. `rsscript-lowering` now owns the
provider-neutral `ExecutableIr` gate; the VM and Rust AOT path both receive this
checked representation, and JIT remains downstream of the VM unit. Runtime-core
now compiles without filesystem, environment, process, network, entropy, or
temporary-directory modules and its default feature set is network-free. The
concrete filesystem, environment, process, HTTP, time, entropy, logging, and CLI
implementations now live in independent `providers/` composition packages.
`rsscript.bytecode.v1` is bounded and checked in; the VM is constructed only
after artifact, checksum, import, control-flow, function, and register
verification. `rss run` uses that verified VM by default. `rss build` and
`rss inspect` expose bytecode and neutral analysis. The primary
`embedded-report-pipeline` demo runs identical artifact bytes with memory and
filesystem providers. The roadmap now prioritizes conformance and boundary
hardening over new language, JIT, self-hosting, or package-system scope.
