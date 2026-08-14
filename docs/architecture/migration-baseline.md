# Architecture migration baseline

This document freezes the behavior and dependency baseline used while RSScript
moves semantic ownership, executable IR, bytecode code generation, and VM
responsibilities. It is a migration guardrail, not a release plan.

## Scope freeze

Until the migration exit criteria below are met, Core accepts correctness,
security-boundary, conformance, diagnostics, and measured-regression changes.
Core does not add language syntax, qualifiers, public intrinsics, official
Providers, JIT tiers or speculation, AOT/native surfaces, package publishing,
language-level policy, or a registry.

The authoritative package maturity inventory is
[`workspace-tiers.toml`](workspace-tiers.toml). Architecture tests require every
workspace package to occur in exactly one tier and require only Core,
applications, and the reference runner to be root default members.

## Migration invariants

The following are mechanical exit conditions, not architectural aspirations:

1. Syntax does not depend on semantics, runtime, Providers, or review.
2. Semantic validation does not depend on a runtime, concrete Provider, review,
   JIT, or AOT implementation.
3. HIR remains source-shaped; the future MIR is typed, owned, CFG-shaped, has no
   syntax dependency, has no unresolved symbol identity, and has no `Unknown`
   execution node.
4. Compiler code generation does not depend on the VM interpreter.
5. The VM accepts only a verifier-created program and does not depend on syntax,
   HIR, semantic databases, package loading, or compiler orchestration.
6. The SDK exposes an explicit reviewed façade; it must not acquire new root
   glob exports from implementation crates.
7. Provider replacement cannot alter compiled Artifact bytes.
8. Analysis, Artifact, and semantic diff carry the same snapshot/module
   identity.
9. Existing and replacement execution paths remain differential-tested until
   the old path is deleted.
10. Experiments consume stable Core contracts and cannot add state to Core VM
    program types.

## Behavior preservation baseline

| Contract | Existing guard | Migration rule |
| --- | --- | --- |
| Source diagnostics | `static`, semantic property, hostile and fuzz corpora | Diagnostic code/span digest changes require an intentional fixture update |
| Source to Artifact | schema contracts and `migration_baseline` | Canonical bundle digest changes require an intentional fixture update |
| Artifact verification | bytecode properties, malformed corpus and fuzz targets | Unverified bytes never enter execution |
| VM behavior | runtime, VM parity, differential and soak suites | New and old lowering paths must produce equivalent reports |
| Cancellation and budgets | hostile, JIT acceptance, runtime and Core metrics | Termination reason and cleanup behavior cannot regress |
| Provider boundary | Provider conformance and replacement demo | Signature mismatch fails before execution |
| Runtime telemetry | execution report schema and Core metrics | Telemetry remains observational and redacted by policy |
| Determinism | package/schema tests and canonical Artifact encoding | Same snapshot must produce byte-identical bundle bytes |

`benchmarks/core/slo.v1.json` remains the performance regression envelope. It is
not a release gate and does not justify JIT expansion; it protects check,
compile, verify, execute, Provider-call, cancellation, and Artifact-size
baselines during internal refactoring.

## Current asset ownership

| Asset | Current owner | Migration disposition |
| --- | --- | --- |
| Parser/CST/AST | `rsscript-syntax` | Keep |
| Immutable snapshots, semantic database and validation phase types | `rsscript-semantics` | Migrated; compiler only assembles them through the analyzer boundary |
| Analyzer orchestration and most checks | `rsscript-compiler` | Move remaining semantic checks and queries to `rsscript-semantics` |
| Typed HIR model | `rsscript-semantics` | Keep source-shaped |
| Owned executable IR | `rsscript-exec-ir` | Transitional; replace source-shaped nodes with typed CFG MIR |
| HIR projection | `rsscript-lowering` | Evolve into HIR-to-MIR lowering |
| VM bytecode emission | `rsscript-vm` | Move to a codegen boundary after MIR exists |
| Artifact Bundle schema/integrity | `rsscript-artifact` | Keep independent of SDK; evolve through versioned typed sections |
| Bytecode envelope/verifier | `rsscript-bytecode` | Keep; evolve through a versioned typed wire model |
| Interpreter, limits, scheduler | `rsscript-vm` | Keep only verified execution responsibilities |
| Dynamic Provider ABI/linking | `rsscript-provider-api` | Keep; tighten wire values and resource handles later |
| Stable embedding path | `rsscript-sdk` | Shrink to explicit phase APIs before public compatibility promises |
| Package capture and persistence | `rsscript-compiler` plus workspace loader | Move OS/persistence concerns out of compiler core |
| AOT/JIT/native/REIR/selfhost | Experimental/Integration/Research tiers | Frozen except correctness and differential value |

## Review convergence TODO

This is the single execution checklist for the architecture review. A checked
item must be backed by code and a mechanical guard; moving a file, adding a
crate, or documenting an intended boundary is not completion by itself.
Unchecked items remain open even when preparatory work exists. The order below
is the required dependency order unless a correctness or security fix must land
first.

Parent items are architecture milestones. Their indented child items are the
planning units: each should fit in one focused change set with targeted tests.
A parent may be checked only after every child is checked and its stated
mechanical acceptance condition holds.

### 0. Freeze and migration guardrails

- [x] **G01 — Classify every workspace package by maturity.**
  `workspace-tiers.toml` is exhaustive and architecture tests reject missing or
  duplicate classifications.
- [x] **G02 — Keep the supported path as the default Cargo build.** Root
  `default-members` contain only Core, applications, and the reference runner.
- [x] **G03 — Split Core and experimental CI feedback.** Core is the blocking
  default gate; experimental, JIT, and self-host checks use separate workflows.
- [x] **G04 — Remove disabled-code cemetery blocks.** CI rejects new
  `#[cfg(any())]` Rust code.
- [x] **G05 — Freeze behavior baselines.** Diagnostics, canonical Artifact,
  execution-report, cancellation, Provider, and Core SLO fixtures protect the
  migration.
- [ ] **G06 — Physically isolate experiments from the Core workspace.** Move
  JIT, AOT, native ABI, REIR, self-host, C/research fixtures, and test generation
  to an experiments workspace or repository. The `experiments/` workspace now
  owns JIT, AOT, native ABI, REIR, and test generation; root Cargo metadata
  excludes them, and CI invokes their maintenance gate explicitly. Self-host and
  native package source remain migration/test fixtures, so the milestone stays
  open until those assets have an explicit external maintenance boundary.
  Self-host parity is now additionally feature-gated and only enabled by its
  dedicated workflow; it no longer participates in default Core test builds.
  The generated-program differential has also moved from the SDK test target to
  the `rss-testgen` experiment, so default Core tests no longer compile its
  experiment dependency.
- [ ] **G07 — Establish public API compatibility gates.** Check in a reviewed
  SDK API inventory, run semver/API-diff checks in CI, and reject experimental
  symbols from default SDK features. The reviewed inventory and default-feature
  architecture gate are now present; a checked v1 façade-export snapshot runs
  in the default Core test path. A generated semver baseline remains blocked on
  completing the explicit façade modules in A03.

### 1. Semantic ownership and query boundary

- [x] **S01 — Move semantic phase types to `rsscript-semantics`.** Immutable
  source snapshots, `SemanticDatabase`, completion state, `AnalysisResult`, and
  `ValidatedProgram` are owned there; architecture tests prevent compiler
  re-ownership.
- [x] **S02 — Move name/type/call checks into semantics.** Migrate analyzer,
  ownership, retention, resource, task-group, call-binding, exhaustiveness, and
  type checks out of `rsscript-compiler`. Completion means compiler only
  orchestrates semantic queries and contains no semantic rule implementation.
  - [x] **S02.1 — Move declaration/name-resolution facts.** Relocate symbol,
    namespace, import, and declaration validation with identical diagnostic
    fixtures and spans. The complete file-local editor symbol index (declarations,
    lexical scopes, references, pattern bindings, and document outlines) now
    belongs to `rsscript-semantics`; workspace namespace/import validation remains.
    - [x] **S02.1a — Move duplicate-declaration diagnostics.** The resolved HIR
      duplicate inventory and its diagnostic construction now belong to
      `rsscript-semantics`; compiler declaration checks only append the canonical
      semantic diagnostics. An architecture test rejects local re-interpretation
      of duplicate HIR facts, while backend-only lowered-name validation remains
      in the compiler.
    - [x] **S02.1b — Move token-local forbidden-surface diagnostics.** Rejection
      of legacy `own struct`, surface-reference, and cast-style syntax is now
      derived by `rsscript-semantics` from syntax tokens. Compiler retains only
      HIR/type-dependent operator validation.
    - [x] **S02.1h — Move unsupported-syntax diagnostic contract.** Compiler
      retains detection while adapting source forms, but
      `rsscript-semantics` owns the canonical unsupported-syntax diagnostic,
      including its code, message, cause, and fix.
    - [x] **S02.1i — Move declaration type and protocol-implementation
      diagnostics.** Semantics owns the canonical unresolved-type and protocol
      implementation mapping diagnostic contracts; compiler only supplies
      resolved declaration facts and spans.
    - [x] **S02.1j — Move protocol implementation signature comparison.**
      Semantics owns resolved parameter, `Self` substitution, return, freshness,
      and retention compatibility; compiler only resolves mapping targets.
    - [x] **S02.1k — Move protocol declaration rules.** Semantics owns the
      source-level protocol method index plus the bodyless-contract and default
      implementation-marker diagnostics; compiler only supplies visible items
      and protocol names.
    - [x] **S02.1l — Move module/use layout rules.** Semantics owns per-file
      module ordering, import ordering, and duplicate local import binding
      diagnostics; compiler only supplies the parsed item stream.
    - [x] **S02.1m — Move declaration surface rules.** Semantics owns removed
      markers, malformed top-level forms, generated-name reservation, generic
      protocol reservation, and the `.rss` function-body requirement.
    - [x] **S02.1n — Move item declaration surface rules.** Semantics owns
      malformed declaration fragments, opaque/drop restrictions, and literal
      const-initializer validation; compiler retains only alias-aware type
      placement and recursive syntax adaptation.
    - [x] **S02.2l — Move callback qualifier placement diagnostics.** Compiler
      supplies alias-canonical `TypeRef` facts; semantics owns recursive
      `owned`/`noescape Fn` placement and malformed type-argument diagnostics.
    - [x] **S02.5v — Move source body-surface traversal.** Semantics owns
      malformed/unsupported statement and expression diagnostics, select-arm
      shape, spawn rejection, and task-group context. Compiler only traverses
      bodies to extract alias-canonical type-reference facts.
    - [x] **S02.2g — Move resolved builtin operator diagnostics.** Semantics
      owns checked-HIR traversal, alias normalization, numeric classification,
      and fixed-operand compatibility; compiler only invokes the query.
    - [x] **S02.1c — Move lexical unknown-binding diagnostics.** Global source
      values and lexical visibility across HIR blocks, patterns, closures,
      resource scopes, and task/select bodies are now interpreted once by
      `rsscript-semantics`; compiler only appends the canonical diagnostics.
    - [x] **S02.1d — Move derive-surface diagnostics.** The language derive
      catalog and resource move-only derive restrictions are semantic queries;
      compiler source traversal only appends their diagnostics.
    - [x] **S02.1e — Move resource declaration-boundary diagnostics.** Raw `Fd`
      surface checks, non-resource resource fields, and weak-class field
      restrictions are semantic queries over source and HIR facts; compiler
      retains only generic-resource call traversal pending the larger migration.
    - [x] **S02.1f — Move resource generic-containment diagnostics.** Resource
      generic type positions and explicit generic call namespaces, including the
      direct `Result<Resource, E>` return exception, are now traversed by
      `rsscript-semantics`; compiler only appends the canonical result.
    - [x] **S02.1g — Move derive-field compatibility diagnostics.** Field-level
      `Eq`/`Ord`/`Hash`/JSON derive compatibility, including nested container,
      local generic, handle, and weak-field reasoning, is now a semantic query.
      The compiler only appends the canonical diagnostics and architecture tests
      reject reimplementation of the derive-support algorithm.
    - [x] **S02.2e — Move generic resource and fresh-return constraints.**
      Resource generic bounds/fields and `fresh` generic return requirements are
      derived by `rsscript-semantics`; compiler no longer owns bound maps or
      their diagnostics.
    - [x] **S02.3.2a — Move declaration signature and retention diagnostics.**
      Explicit return/parameter types, method/protocol `self` shape, and
      `retains` declaration validity are semantic queries; compiler only
      orchestrates their canonical diagnostics.
    - [x] **S02.2f — Move protocol-bound visibility diagnostics.** Generic
      protocol-bound resolution across interface and source snapshots is owned
      by `rsscript-semantics`; compiler retains protocol implementation mapping
      checks only.
  - [x] **S02.2 — Move type and generic-constraint checks.** Relocate type
    inference, substitutions, generic constraints, and call result facts behind
    the semantic crate API.
    - [x] **S02.2a — Move cyclic type-alias diagnostics.** Alias dependency
      graphs and their diagnostic construction are now derived by
      `rsscript-semantics` from immutable interface/source programs. Compiler
      type checks only append the canonical result; unknown-type, field, and
      generic-constraint checks remain follow-up work.
    - [x] **S02.2b — Move resolved unknown-field diagnostics.** The semantic HIR
      now owns field-existence diagnostics; compiler only appends them. Unknown
      type names, bindings, and generic constraints remain follow-up work.
    - [x] **S02.2c — Move `Dyn<Protocol>` validation.** External binding type
      shape and protocol-visibility diagnostics are semantic queries; compiler
      supplies only the already-composed visible protocol set.
    - [x] **S02.2d — Move unknown source type diagnostics.** Alias, builtin,
      generic, and declared-type identity now resolve through semantics HIR;
      compiler supplies source and visible protocol snapshots only.
    - [x] **S02.2h — Move generic protocol satisfaction rules.** Semantics now
      evaluates builtin, structural-container, generic-bound, implementation,
      and derive protocol facts. Compiler supplies the resolved call type and
      visible source inventory only.
    - [x] **S02.2i — Move structural call type compatibility.** Semantics owns
      alias-expanded rendered type compatibility for qualifiers, function
      parameter effects, containers, and open `Option`/`Result` variants;
      compiler only supplies resolved types and invokes the semantic rule.
    - [x] **S02.2k — Move unresolved generic detection.** Semantics owns the
      rendered-type recursion and generic identity rule; compiler call and
      assignment paths provide only active/declaration type facts.
    - [x] **S02.5p — Move assignment diagnostics.** Compiler retains lexical
      scope/place fact extraction, while semantics owns invalid-target,
      mutability, type-mismatch, and indexed-target diagnostic contracts.
    - [x] **S02.5q — Move exhaustiveness diagnostics.** Compiler determines
      match coverage from resolved HIR/declarations; semantics owns the
      canonical statement/expression non-exhaustiveness diagnostic contract.
    - [x] **S02.5r — Move async-lowering boundary diagnostics.** Compiler
      finds invalid nested-await and cancellation-token source facts, while
      semantics owns their canonical structured-async diagnostic contracts.
    - [x] **S02.5s — Move task-group async-let rules.** Semantics now owns
      source-AST traversal and validation of async-let lexical scope, direct
      await placement, declaration order, and exactly-once consumption;
      compiler only appends the resulting semantic diagnostics.
    - [x] **S02.5t — Move async cancellation-token ownership traversal.**
      Semantics owns the source-AST traversal that excludes nested task-group
      boundaries and derives cancellation-token diagnostics for async functions.
    - [x] **S02.5u — Move async lowering-shape traversal.** Semantics owns the
      source-AST rule that distinguishes direct suspension boundaries from
      nonlinear `await` expressions; compiler only appends the query result.
    - [x] **S02.2j — Move bounded generic substitution.** Semantics owns the
      recursive substitution algorithm; compiler contributes only a narrow
      adapter to its shared cancellation and substitution budget.
  - [x] **S02.3 — Move call binding and effect checks.** Relocate positional and
    named argument binding, `read`/`mut`/`take`, external signature matching,
    and retention facts.
    - [x] **S02.3.1 — Remove compiler call-binding compatibility ownership.**
      `CallBinding` and its source/evaluation-order contract are owned and
      consumed directly from `rsscript-semantics`; architecture tests reject the
      old compiler module and re-export path.
    - [x] **S02.3.2 — Move effect and signature diagnostics.** Relocate call-site
      `read`/`mut`/`take`, external signature, retention, and argument-shape
      validation from compiler checks into semantic queries.
      - [x] **S02.3.2b — Move call argument shape and data-effect diagnostics.**
        Naming, duplicate/unknown/missing argument, and `read`/`mut`/`take`
        diagnostics now consume resolved call facts in `rsscript-semantics`;
        compiler call checking supplies resolution facts only. External
        signature matching and retention remain follow-up work.
      - [x] **S02.3.2c — Move receiver-call effect diagnostics.** The implicit
        receiver's declared and call-site `read`/`mut`/`take` contract is now a
        semantic query over resolved receiver facts; compiler only resolves the
        receiver expression and signature slot.
      - [x] **S02.3.2d — Move return type diagnostics.** Compiler resolves
        return and variant-payload type facts; semantics owns both canonical
        return mismatch diagnostics and fixes.
      - [x] **S02.3.2e — Move callback contract diagnostics.** Compiler derives
        resolved callback parameter, return, freshness, operator, and retention
        facts; semantics owns their canonical mismatch diagnostics and fixes.
      - [x] **S02.3.2f — Move callback escape diagnostics.** Semantics owns the
        noescape/local closure escape-context model and diagnostics; compiler
        only finds escape uses and supplies resolved context/span facts.
      - [x] **S02.3.2g — Move resolved call type diagnostics.** Compiler keeps
        alias expansion and type matching, while semantics owns binding,
        argument/payload, map/list literal, resolution, and message-transfer
        diagnostic contracts.
      - [x] **S02.3.2h — Move resolved protocol and variant diagnostics.**
        Compiler resolves protocol satisfaction, `Dyn` construction, and
        variant-field facts; semantics owns the canonical generic-bound,
        protocol-receiver, and variant-construction diagnostic contracts.
  - [x] **S02.4 — Move ownership and resource checks.** Relocate moves, escapes,
    borrows, `fresh`/`owned`/`noescape`, resource declarations, and cleanup
    validation with property and hostile-corpus coverage.
    - [x] **S02.4a — Move weak-field upgrade validation.** Checked-HIR traversal
      for weak handle use and the required explicit `Weak.upgrade` boundary now
      belongs to `rsscript-semantics`; compiler only invokes the semantic rule.
    - [x] **S02.4b — Move `?` operand validation.** Known operand type validation
      for `Result`/`Option` short-circuiting is now a semantic query; function
      error-type compatibility remains a separate follow-up.
    - [x] **S02.4c — Move `?` function error compatibility.** Recursive checked
      HIR validation of `Result` error-type equality now belongs to semantics;
      compiler provides only the enclosing function's resolved error type.
    - [x] **S02.4d — Move literal validity diagnostics.** Checked-HIR validation
      for `i64` integer bounds and single-scalar `Char` literals is now owned by
      `rsscript-semantics`; the compiler only appends the semantic result.
    - [x] **S02.4e — Move `match` character-pattern validity.** The same
      single-scalar `Char` rule, including the pattern-specific fix, is now
      owned by `rsscript-semantics` rather than compiler pattern traversal.
    - [x] **S02.4f — Move pattern field effect diagnostics.** Semantics owns
      managed-class read-only and child-effect monotonicity diagnostics; compiler
      provides the resolved class and scrutinee-effect facts.
    - [x] **S02.4g — Move duplicate pattern field diagnostics.** Semantics owns
      diagnostics for duplicate fields and mutable/taking partial-access
      conflicts; compiler only tracks the preceding binding facts.
    - [x] **S02.4h — Move LocalAnalysis ownership diagnostics.** Compiler flow
      derives moved, managed-to-local, retained-local, retained-closure, and
      handle-field facts; semantics now owns each resulting diagnostic.
    - [x] **S02.4i — Move resource-boundary diagnostics.** Compiler continues
      to traverse checked HIR and derive flow facts, while semantics owns
      managed-closure captures, resource escapes/captures/producers, `with`
      result unwrapping, class-local bindings, and invalid `manage`/`take`
      diagnostics.
    - [x] **S02.4j — Move `fresh` return diagnostics.** Compiler flow supplies
      clean/unknown return facts and the resolved return type; semantics owns
      non-fresh, unknown-freshness, and invalid-fresh-return-type diagnostics.
    - [x] **S02.4k — Move `fresh` and constructor ownership diagnostics.**
      Compiler supplies constructor, field, effect, and capture facts; semantics
      owns fresh-temporary, weak-field, constructor-field, managed-inline, and
      spawn-local-capture diagnostics.
    - [x] **S02.4l — Move read-view and closure-contract diagnostics.**
      Compiler gathers local/capture/place facts; semantics owns read-view
      exclusivity, noescape capture consumption, and explicit capture contract
      diagnostics.
    - [x] **S02.4m — Move call-place conflict diagnostics.** Compiler resolves
      local place paths and disjointness facts; semantics owns managed split,
      whole-base, prefix, indexed, and move-versus-field diagnostics.
    - [x] **S02.4n — Move uninferable binding diagnostics.** Compiler gathers
      raw binding-use facts and recognizes open variant constructors; semantics
      owns the canonical ambiguous-binding diagnostic and fix.
    - [x] **S02.4o — Move local-flow fact model.** The moved-use, retention,
      managed-to-local, fresh-return, handle-take, and resource-escape fact
      types are owned by `rsscript-semantics`; compiler flow only produces them
      while its CFG implementation is migrated.
    - [x] **S02.4p — Move resource-producer classification and traversal.**
      Semantics owns HIR resource/`Result<Resource, E>` classification,
      recursive boundary traversal, and the `with`/`?` diagnostics; compiler
      only selects the enclosing expression and aggregates diagnostics.
    - [x] **S02.4q — Move generic HIR identifier-use traversal.** Semantics
      owns the source-order HIR identifier-use queries consumed by compiler CFG,
      closure-move, and resource checks; compiler only derives its local-flow
      facts from the neutral query output.
    - [x] **S02.4r — Move HIR effect-event and place-path queries.** Semantics
      owns canonical HIR effect-event extraction and identifier/field path
      resolution; compiler CFG consumes those neutral facts without traversing
      expressions to reconstruct `take` events.
    - [x] **S02.4s — Move HIR inline-capture queries.** Semantics owns the
      handle-aware closure capture-use traversal; compiler consumes its facts
      for managed closures, local flow, and retained-closure diagnostics.
    - [x] **S02.4t — Move managed-closure fact indexing.** Semantics owns
      statement-keyed managed-closure capture indexing, including nested
      closure discovery; compiler flow only reads the neutral index.
    - [x] **S02.4u — Move resource-escape fact indexing.** Semantics owns the
      recursive `with` escape/capture index, including retain, `manage`,
      wrapper, and intentional `TempDir.keep(take ...)` transfer semantics;
      compiler only reads the span-keyed neutral facts.
    - [x] **S02.4v — Move retained-closure argument resolution.** Semantics
      owns extraction of closure payloads through `read` and option/result
      wrappers; CFG and retained-capture checks consume the same HIR query.
    - [x] **S02.4w — Move value-property queries.** Semantics owns canonical
      Copy and cross-isolate transfer eligibility; compiler assignment, call,
      and CFG checks consume the shared query instead of reinterpreting type
      names.
    - [x] **S02.4x — Move taken handle-field traversal.** Semantics owns the
      nested HIR traversal that identifies `take` on handle fields; compiler
      local analysis only retains the resulting ownership facts.
    - [x] **S02.4y — Move fresh-return HIR projections.** Semantics owns fresh
      field-base, handle/weak-path, and operand-span projection rules; compiler
      local flow provides only state-dependent freshness evidence.
    - [x] **S02.4z — Move fresh match-binding facts.** Semantics owns the
      single-payload `Some`/`Ok` fresh binding contract and resolved-HIR
      projection; compiler CFG only turns that fact into a flow node.
    - [x] **S02.4aa — Move local-binding HIR facts.** Semantics owns the
      initializer's source, handle-field, and fresh-value projections; compiler
      CFG consumes one fact object when constructing a binding node.
    - [x] **S02.4ab — Move HIR type-name projection.** Semantics owns the
      temporary normalized rendered type view used by local CFG while consumers
      migrate to structural type identities.
    - [x] **S02.5w — Move structured control-flow exit state.** Semantics owns
      `Flow` and its conservative non-fallthrough merge rule; compiler CFG and
      block checks consume the shared state model.
    - [x] **S02.4ac — Move local ownership-state lattice.** Semantics owns the
      local/managed/resource/move state, parameter seeding, and move/retention
      transitions; compiler preserves only its compatibility alias while CFG
      transfer and merge migration continues.
    - [x] **S02.4ad — Move local-flow entry construction and path roots.**
      Semantics owns creation of the parameter-derived entry state and resolved
      place-path root projection, so compiler flow consumers no longer recreate
      either semantic convention.
    - [x] **S02.4ae — Move local-flow graph model.** Semantics owns the neutral
      graph step, binding, resource-binding, and cleanup-edge data model;
      compiler graph construction and fixed-point solving remain the next
      migration stages.
    - [x] **S02.4af — Move local-flow fixed-point solving.** Semantics owns
      ownership-state transfer, cleanup-edge application, conservative joins,
      and reachable graph-entry state computation; compiler only supplies the
      checked-HIR-derived graph.
    - [x] **S02.4ag — Move local-flow graph construction.** Semantics now
      lowers checked HIR into the local ownership graph, including structured
      branches, loops, resource-cleanup edges, and closure-capture facts;
      compiler has only compatibility adapters for legacy callers.
    - [x] **S02.4ah — Move fresh-return flow facts.** Semantics derives failed
      `fresh` return proofs from checked HIR plus local-flow entry states;
      compiler consumes the resulting facts to emit diagnostics.
    - [x] **S02.4ai — Move retained-closure flow facts.** Semantics derives
      local captures passed to retaining calls from checked HIR plus local-flow
      entry states; compiler consumes the resulting facts to emit diagnostics.
    - [x] **S02.4aj — Move moved-use flow facts.** Semantics derives
      use-after-move facts from checked HIR plus local-flow entry states,
      including field paths, match `take`, and nested closures; compiler has
      no local ownership traversal.
  - [x] **S02.5 — Move async and control-flow checks.** Relocate task groups,
    cancellation, await/select, assignment, exhaustiveness, and reachability
    checks.
    - [x] **S02.5a — Move function fallthrough diagnostics.** The resolved-HIR
      control-flow rule for non-`Unit` functions which can reach their end is
      now owned by `rsscript-semantics`; compiler call checking only appends its
      canonical diagnostic. Task, cancellation, select, assignment,
      exhaustiveness, and reachability remain follow-up work.
    - [x] **S02.5b — Move bare-return result diagnostics.** Explicit empty
      returns within nested checked HIR blocks are now derived by semantics for
      concrete non-`Unit` functions; compiler retains only expression-return
      compatibility while alias and aggregate facts migrate.
    - [x] **S02.5c — Move await-placement diagnostics.** Async-context traversal
      across checked HIR blocks, closures, assignment targets, and select
      boundaries is now owned by `rsscript-semantics`; operand and live-value
      checks remain separate follow-up work.
    - [x] **S02.5d — Move await-operand diagnostics.** The direct async-call or
      task-group async-let requirement is now checked by semantics, including
      consumption of a pending async-let binding; live-value checks remain a
      separate ownership follow-up.
    - [x] **S02.5e — Move async-call consumption diagnostics.** Resolved async
      calls evaluated without `await` or `spawn` are diagnosed by semantics;
      compiler supplies only resolved call facts.
    - [x] **S02.5f — Move await live-value diagnostics.** Compiler flow analysis
      supplies live resource/local facts, while semantics owns the canonical
      cross-suspension diagnostic, cause, and fix.
    - [x] **S02.5g — Move Boolean condition diagnostics.** Checked-HIR type
      validation for `if` and `while` conditions now belongs to semantics;
      compiler flow orchestration only appends the resulting diagnostic.
    - [x] **S02.5h — Move `for` iterable diagnostics.** Checked-HIR type
      validation for synchronous `List<T>` and asynchronous `Stream<T>` loop
      inputs now belongs to semantics; compiler retains loop flow handling.
    - [x] **S02.5i — Move `match` expression arm result diagnostics.** Checked-HIR
      validation that all value-producing match arms agree with the resolved
      expression result type now belongs to semantics.
    - [x] **S02.5j — Move `match` scrutinee diagnostics.** Semantics owns the
      supported scrutinee-type rule and canonical diagnostic; compiler supplies
      alias-expanded type and declared pattern-kind facts from its current HIR
      adapter.
    - [x] **S02.5k — Move literal `match` pattern type diagnostics.** Semantics
      owns the canonical compatibility rule and diagnostic for literal patterns;
      compiler retains only recursive pattern traversal and alias expansion.
    - [x] **S02.5l — Move variant pattern diagnostics.** Semantics now owns
      canonical diagnostics for non-matching patterns, invalid variant families,
      and positional binding arity; compiler only resolves fields and recurses.
    - [x] **S02.5m — Move structured `match` effect diagnostics.** Semantics
      owns the explicit scrutinee-effect requirement for struct and list
      patterns; compiler continues to supply only per-arm facts.
    - [x] **S02.5n — Move `match` guard mutation diagnostics.** Compiler still
      locates the first effect in checked HIR, while semantics owns the
      read-only-guard diagnostic, cause, and fix.
    - [x] **S02.5o — Move declared pattern field diagnostics.** Semantics owns
      diagnostics for unknown structured fields and omitted fields without `..`;
      compiler supplies the resolved declaration facts.
  - [x] **S02.6 — Guard compiler semantic-rule ownership.** Architecture tests
    now reject any direct language-diagnostic construction below compiler
    checks. The only explicit exceptions are frontend-budget termination and
    Rust `#lower_name` backend constraints, both documented at the source
    boundary; all language diagnostic contracts are delegated to
    `rsscript-semantics`.
- [ ] **S03 — Add one `CompilationSession` query boundary.** Introduce stable
  source/module/interface/definition/type identities, dependency tracking,
  cancellation, deadlines, and cached HIR/MIR queries shared by CLI, package
  compilation, tests, and editor tooling.
  - [x] **S03.1 — Define source and semantic identities.**
    `rsscript-source-model` now owns serializable `FileId`, `SourceRevision`,
    `ModuleId`, and `InterfaceId`; `rsscript-semantics` owns serializable
    `DefinitionId` and `TypeId`. Immutable source snapshots assign stable
    ordinal file IDs and initial revisions, expose lookup by ID, and have
    construction/serialization tests. Session mutation and dependency tracking
    remain follow-up work.
  - [x] **S03.2 — Capture revisions in a session-owned source store.**
    `CompilationSession` owns separate deterministic source and interface
    stores with explicit set/replace/remove operations. Snapshots are immutable,
    path-ordered, retain non-reused file IDs, and advance revisions only when
    bytes change; focused tests cover replacement, deletion, and interface
    capture. Query migration remains follow-up work.
  - [ ] **S03.3 — Cache parse, resolve, type, HIR, and diagnostic queries.**
    Record dependencies so unrelated file changes do not invalidate a workspace.
    `CompilationSession` now owns parse-tree and local HIR caching keyed by
    immutable role/file/revision, including replacement/deletion invalidation
    and deterministic source iteration. Module-header/import facts now share
    that cache. `WorkspaceModuleGraph` now owns parsed import closure and
    transitive interface-dependent-path facts, so editor clients no longer
    retain a second dependency graph. Namespace isolation, including
    source/interface graph partitioning, is semantic-owned and the session now
    caches an interface-aware workspace HIR after that canonical rewrite. The
    declaration/signature `SemanticTypeFacts` owned by that HIR now also have a
    revision-invalidated workspace query with cancellation/deadline handling.
    Complete workspace diagnostics also use a session-owned, immutable
    source/interface snapshot cache with revision invalidation. The language
    service now obtains document bytes from those session snapshots, retaining
    only LSP revision/kind metadata rather than a second source-text cache;
    resolve/type and dependency-precise semantic-diagnostic invalidation remain
    open.
  - [ ] **S03.4 — Thread cancellation and deadlines through every query.** Add
    cancellation, deadline, and diagnostic-budget tests for cold and cached
    paths. Session parse and local HIR queries now check the shared operation
    context before and after cache access for source and interface inputs. The
    session-owned workspace diagnostic query now applies the same checks before
    cache access, during transitional analysis, and before caching the result.
    Workspace-module graph, workspace-HIR, and language-service document
    diagnostics now apply the same rule across dependency closure, lint, and
    cached-result paths; resolve/type queries remain to be migrated.
  - [ ] **S03.5 — Migrate CLI/package/test callers.** All frontend consumers use
    the session API; direct analyzer construction becomes private.
- [ ] **S04 — Make language service consume semantic queries directly.** It
  must not depend on the compiler compatibility façade, package persistence,
  VM, SDK, or Providers; revision invalidation and request cancellation require
  focused tests.
  - [ ] **S04.1 — Replace the compiler façade dependency with syntax/semantic
    query dependencies.** Editor symbols and formatting now come directly from
    `rsscript-semantics` and `rsscript-syntax`; syntax lint now also bypasses
    compiler. Module/import dependency discovery and local editor symbols now
    consume shared CompilationSession parse/header/module-graph queries instead
    of reparsing or line-oriented text extraction, including interface
    visibility and invalidation traversal. The LSP now delegates both single-file and package
    overlay diagnostics to the language service workspace query instead of
    reconstructing a second analyzer call sequence. `LanguageService` no
    longer depends on the compiler: its composition root injects the temporary
    compiler diagnostic adapter while the service owns the immutable session
    snapshot and cache, including document text. Replacing it with a
    semantic-owned analyzer no longer
    requires changing the language-service crate. Cargo metadata tests reject
    a language-service edge to compiler, VM, SDK, package persistence, or
    concrete Providers.
  - [x] **S04.2 — Add document revision and invalidation tests.** The language
    service suite verifies revision replacement and deletion, direct and
    transitive interface invalidation, session-owned semantic-cache
    recomputation after interface edits, cancellation, deadlines, and
    response-budget cache semantics. Document diagnostics no longer retain a
    competing per-document semantic cache; precise dependency invalidation is
    an open `CompilationSession` query-engine responsibility.
- [ ] **S05 — Finish compiler purity.** Compiler input is an explicit immutable
  `SourceSet`/`WorkspaceSnapshot`; package traversal, filesystem locking,
  temporary files, compression, Artifact persistence, review/risk, and Rust AOT
  lowering live outside the compiler dependency closure. The reviewed SDK now
  exposes `FrontendInputSnapshot` and `Compiler::compile_snapshot`, keeping
  in-memory source/interface capture separate from package-path convenience
  APIs. The latter now live behind the explicit SDK `project` adapter, so the
  reviewed `Compiler` surface remains in-memory; compiler package traversal
  and persistence remain to be moved.
  - [ ] **S05.1 — Move workspace capture to `rsscript-workspace-loader`.** Move
    directory traversal, manifest/dependency discovery, path normalization, and
    snapshot capture from compiler. The loader now exposes immutable
    WorkspaceSnapshot capture with explicit-base and operation-aware APIs plus
    a stable, absolute-path-independent content digest. Ambient-current-directory
    compatibility capture APIs have been removed and the LSP captures from its
    explicit package root; captured files now separate physical paths from
    stable root/dependency-qualified logical paths so snapshot identity cannot
    collide across same-named dependency files. The SDK project adapter now
    exposes explicit-base loader capture as a separate immutable
    `FrontendInputSnapshot` path, and a focused test proves that snapshot feeds
    the pure in-memory compiler without absolute paths. Its frontend digest now
    binds exactly the source/interface input used by `build_captured`, separate
    from the broader package digest that can include tests. The normal project
    `compile_package` path now captures through this loader and delegates to
    the pure compiler; legacy snapshot/build APIs remain only for package
    analysis/native compatibility. A dependency-interface fixture proves the
    default route includes dependency-qualified `.rssi` inputs exactly once.
    The reviewed `project` feature no longer selects the compiler `package`
    dependency closure: the remaining package-analysis/native snapshot API is
    isolated under `project::legacy::PackageCompatibility` and requires the
    explicit SDK `compatibility` feature. Compiler package
    callers that also require native/review compatibility remain to be migrated.
  - [ ] **S05.2 — Move Artifact persistence to an adapter.** Relocate locks,
    atomic writes, temporary files, compression, and artifact-store policy out
    of compiler. The confined lock/read/write implementation now lives in the
    standalone `rsscript-artifact-store` adapter; compiler package compatibility
    code uses it only through the explicit `package` feature and no longer
    re-exports it. Remaining package lock, review, native snapshot, and generated-Rust persistence paths
    still need to leave the compiler compatibility layer before this item can
    close. The adapter is no longer re-exported through the compiler; only the
    explicit SDK compatibility façade exposes it directly.
  - [ ] **S05.3 — Move review, risk, and package presentation out of compiler.**
    Keep neutral analysis facts; make review formatting and policy adapters
    optional consumers.
  - [ ] **S05.4 — Move Rust/AOT lowering to its experimental boundary.** Compiler
    no longer exposes generated Rust or native lowering APIs. Native package
    dependency identity now belongs to the package snapshot, and the Rust
    lowerer only consumes and temporarily re-exports it for compatibility. Rust
    lowering is now selected only by the explicit `aot-rust` feature rather
    than ordinary compiler execution. The CLI mirrors this separation: its
    normal `execution` feature builds only the reference VM/isolated runner,
    while generated-Rust execution and its help text require an explicit
    `aot-rust` feature. Rust source lowering, source maps, runtime ABI helpers,
    and the public AOT façade remain in compiler, so this item remains open.
  - [ ] **S05.5 — Enforce a frontend-only compiler dependency closure.** Cargo
    metadata and `cargo tree` tests reject OS, persistence, Provider, VM, review,
    JIT, and AOT dependencies. Compiler lowering is now a dedicated feature;
    filesystem/package/persistence dependencies are optional behind the separate
    `package` feature, while reviewed SDK `execution` selects lowering but not
    package capture. The frontend still has a compatibility `execution` feature
    and build-script catalog inputs, so the complete dependency-closure
    acceptance condition remains open. Unused REIR/review/fuzz/schema dev
    dependencies have been removed; the legacy VM is now an optional dependency
    selected only by the explicit `selfhost-parity` research feature.

### 2. Typed CFG MIR

- [ ] **M01 — Define typed stable identities.** Add `FunctionId`, `TypeId`,
  `BlockId`, `ValueId`, `PlaceId`, `BuiltinId`, `ExternalSymbolId`, and
  `ResourceTypeId` without string identity at backend boundaries.
  - [x] **M01.1 — Define index/newtype IDs and ownership tables.** Initial
    frontend-free IDs are owned by `rsscript-mir`; they are local to one MIR
    module, non-string, deterministic, and cannot be mixed by type.
  - [ ] **M01.2 — Lower semantic names and `WireType` references into IDs.**
    Backend inputs contain resolved function, external symbol, builtin, and
    resource identities only.
    - [x] **M01.2a — Add the initial module type table.** Function parameter and
      result types are interned as `TypeId` values during the executable-IR
      bridge; builtin and resource identities remain follow-up work.
  - [ ] **M01.3 — Add stable display/debug/source-map side tables.** Human names
    remain available without becoming executable identity.
    - [x] **M01.3a — Add initial debug names.** Function/place debug names are
      present without becoming executable identity. Constants and source spans
      remain follow-up work.
- [ ] **M02 — Define an owned CFG MIR.** Functions contain basic blocks,
  instructions, and terminators; MIR does not depend on syntax and contains no
  unresolved or `Unknown` execution node.
  - [ ] **M02.1 — Introduce `MirModule`, `MirFunction`, `BasicBlock`,
    `Instruction`, and `Terminator`.** Only the lowerer and verifier may
    construct valid modules.
    - [x] **M02.1a — Add the initial owned model.** The model has private fields
      and construction runs structural verification. Serialization and a more
      restricted construction boundary remain follow-up work.
  - [ ] **M02.2 — Lower the pure scalar subset.** Cover constants, locals,
    arithmetic, calls, returns, branches, loops, and explicit block edges.
    - [x] **M02.2a — Bridge the initial scalar subset.** The executable-IR
      bridge lowers literals, local bindings, assignment, binary expressions,
      direct internal calls, returns, branches, loops, break, and continue;
      unsupported operations fail closed. `CompiledIr::mir` now uses only the
      direct checked-HIR path; the SDK enters the legacy executable-IR encoder
      only after an explicit `Unsupported` result, never to mask an invalid
      direct MIR lowering. Reviewed SDK `execution` no longer selects that
      encoder; only the explicit `legacy-exec-ir` compatibility feature may do
      so.
  - [x] **M02.3 — Lower aggregate and pattern operations.** Cover records,
    variants, collections, field/index operations, and match dispatch without
    source AST nodes in MIR.
    - [x] **M02.3a — Lower owned list construction.** Source array literals
      lower to `MakeList { destination, items }`; its owned `ValueId` inputs
      participate in dominance validation and codegen emits the existing
      verifier-checked `MakeList` bytecode instruction. Records, variants,
      field/index operations, and match dispatch remain fail-closed.
    - [x] **M02.3b — Lower owned map construction.** Source map literals lower
      to `MakeMap { destination, entries }`, preserving resolved key/value
      `ValueId` pairs for dominance validation. Codegen emits verifier-checked
      v1 map entries and the dual-path corpus compares map results; records,
      variants, field/index operations, and match dispatch remain fail-closed.
    - [x] **M02.3c — Lower JSON object construction.** JSON object literals
      lower to `MakeObject { destination, fields }`; field names are serialized
      JSON data while values remain resolved `ValueId`s. The dual-path corpus
      compares canonical JSON output; language records, variants, field access,
      non-list indexing, and match dispatch remain fail-closed.
    - [x] **M02.3d — Lower resolved list indexing.** The lowerer accepts an
      index only when checked projection facts identify its base as `List<…>`;
      it emits `ListGet` over resolved value IDs. Map/JSON/record indexing,
      field access, variants, and non-scalar match dispatch remain fail-closed.
    - [x] **M02.3e — Lower scalar match forms.** Checked literal and wildcard
      statement and expression arms lower to owned `Equal`/`Branch` CFG blocks
      with an explicit unmatched terminator; expression arms join through an
      owned result place. Guards and variant/record/list patterns remain
      fail-closed pending typed projection operations.
    - [x] **M02.3f — Lower checked list iteration.** Synchronous `for item in
      List<T>` lowers to explicit `ListLen`/`ListGet`, index-place, and CFG
      operations; a direct-HIR dual-path fixture compares the legacy VM, MIR
      reference interpreter, and verified MIR bytecode VM. Async and non-list
      iteration remain fail-closed pending stable protocol semantics.
    - [x] **M02.3g — Lower resolved aggregate field reads.** Checked HIR field
      access now lowers to `GetField { destination, base, field }`, using only
      owned value IDs and aggregate data. The MIR conformance interpreter and
      MIR-only codegen emit/execute the existing verifier-checked `GetField`
      bytecode operation; typed layouts, mutation, variants, and pattern
      projection remain fail-closed.
    - [x] **M02.3h — Lower resolved record construction.** Checked struct/class
      constructors now lower to `MakeStruct { destination, ty, fields }`, with
      a canonical named `TypeId`, source-order argument evaluation, and
      declaration-order layout fields. MIR rejects malformed record layouts and
      the direct migration corpus compares construction plus field projection
      across the legacy VM, MIR interpreter, and verified MIR bytecode VM.
      Resources, variants, mutation, and pattern projection remain fail-closed.
    - [x] **M02.3i — Lower resolved sum-variant construction.** Checked semantic
      variant layouts now lower to `MakeVariant { destination, ty, variant,
      fields }`, preserving source evaluation order and declaration-order
      fields. The direct build path emits the existing verified bytecode opcode;
      variant matching, destructuring, and guards remain fail-closed pending
      explicit CFG dispatch.
    - [x] **M02.3j — Lower resolved payload-free variant dispatch.** Checked HIR
      now lowers a resolved, tag-only sum-variant match arm to
      `MatchVariant { value, expected, match_target, else_target }`. The MIR
      verifier validates both CFG edges and non-empty tags; the direct codegen
      path reuses the verified `MatchVariant` bytecode instruction. The
      migration corpus covers both statement and expression arms. Payload
      destructuring and guards remain fail-closed pending explicit binding and
      cleanup semantics.
    - [x] **M02.3k — Lower flat variant payload bindings.** A resolved sum
      variant arm can now bind or ignore each positional field. The lowerer
      validates semantic layout arity, emits explicit `GetField` plus local
      `WritePlace` operations in the matching CFG block, and rejects nested
      patterns. The direct migration corpus differentially executes statement
      and expression payload bindings across the legacy VM, MIR interpreter,
      and verified bytecode VM.
    - [x] **M02.3l — Lower canonical Result match dispatch.** `Ok` and `Err`
      patterns now lower to `MatchResult` CFG edges, with named payload arms
      emitting `UnwrapResult` and `WritePlace` only after their selected edge.
      The verifier and cleanup/task dataflow traverse both explicit result
      targets, while unsupported nested bindings and guards remain fail-closed.
      Statement and expression fixtures compare the legacy VM, MIR interpreter,
      and verified bytecode VM.
    - [x] **M02.3m — Lower canonical Option match dispatch.** `Some` and `None`
      constructors and patterns now lower to explicit `MakeOption`,
      `MatchOption`, and (for named `Some` payloads) `UnwrapOption` operations.
      Both control-flow targets participate in verifier dataflow; nested
      patterns and guards remain fail-closed. Statement and expression fixtures
      compare legacy VM, MIR interpreter, and verified bytecode VM.
- [x] **M02.4 — Add MIR structural validation.** Reject dangling blocks,
    invalid IDs, unterminated blocks, undefined values, and malformed CFG edges.
    - [x] **M02.4a — Verify the initial structural subset.** The verifier
    rejects empty functions, invalid block/place/value references, duplicate
    definitions, undefined values, invalid CFG targets, and values that do not
    dominate a control-flow use. Cleanup-path validation remains follow-up work.
    - [x] **M02.4b — Verify resource cleanup over CFG exits.** Resource liveness
    propagates conservatively over reachable CFG edges and rejects a return
    branch that omits a release even when sibling branches clean up. Cancellation
    and Provider-error cleanup edges remain runtime-owned follow-up work.
    - [x] **M02.4c — Verify task-group closure over CFG exits.** Task liveness
    propagates over reachable CFG edges and rejects a return branch that omits
    its lexical group drain even when sibling branches join. Cancellation and
    select cleanup edges remain follow-up work.
- [ ] **M03 — Make semantic operations explicit.** MIR represents move,
  read/mut borrow, retain, drop, resource acquire/release, spawn, await, join,
  cancellation, selection, external calls, and every cleanup/unwind edge.
  - [x] **M03.1 — Add explicit ownership instructions.** MIR models standalone
    and call-boundary move, read/mutable borrow, retain, and drop; construction
    validation rejects use-after-move on linear paths and CFG joins.
    - [x] **M03.1a — Lower direct-call read borrows.** A checked `read` of a
      local argument becomes verifier-visible `BorrowRead`; mutable borrow,
      move, retain, and drop remain follow-up work.
    - [x] **M03.1b — Model mutable borrows and direct moves.** Call arguments
      retain `BorrowMut`/`Take` place identity; the verifier rejects reads after
      a direct move and the conformance interpreter writes mutable parameters
      back to their caller places. CFG join dataflow treats a place as moved when
      any predecessor moves it, while assignment reinitializes it. Retain/drop
      remain follow-up work.
    - [x] **M03.1c — Verify call ownership contracts.** Function signatures own
      `read`/`mut`/`take` parameter modes and verifier checks each direct or
      external call argument against that contract before execution.
    - [x] **M03.1d — Add explicit retain/drop ownership instructions.** MIR now
      preserves retention as a verifier-visible operation and models `drop` as a
      place-state transition; linear/CFG validation rejects reads after an
      explicit drop until a write reinitializes the place. Direct checked-HIR
      calls emit `Retain` for a resolved `retains(param)` managed-local argument;
      explicit source `drop` and a dedicated runtime opcode remain follow-up
      work.
    - [x] **M03.1e — Lower standalone local moves.** A checked `take local`
      expression becomes explicit `TakePlace`, which consumes the source place
      in MIR and remains visible to the scalar codegen/conformance paths.
    - [x] **M03.1f — Lower Result construction and propagation.** Checked
      `Ok`/`Err` enum variants lower to a typed `MakeResult` operation, and
      checked `?` lowers to `TryResult { source, cleanup }`. The latter records
      the lexical resource cleanup edge before the VM short-circuits an
      `Err`/`None`; codegen emits the verifier-checked v1 instructions and the
      execution-feature SDK tests cover both task results and an `Err` return.
  - [ ] **M03.2 — Add resource lifetime instructions and cleanup edges.** Model
    acquire/manage/release and verify cleanup for normal return, branch exit,
    error, and cancellation.
    - [x] **M03.2a — Establish acquire/release verifier primitives.** Typed MIR
      models canonical resource acquire/release, rejects invalid resource IDs
      and unbalanced normal-return lifetimes, and makes unsupported VM codegen
      fail closed. Source lowering, manage/transfer, and non-normal cleanup
      edges remain follow-up work.
    - [x] **M03.2b — Lower managed linear resource scopes.** The transitional
      lowerer turns an explicitly managed, normally-falling-through `with`
      scope into `AcquireResource`/`ReleaseResource` around its binding and
      interns a canonical resource type. Unmanaged scopes and non-normal exits
      remain fail-closed until cleanup-edge lowering is implemented.
    - [x] **M03.2c — Emit normal non-local cleanup edges.** The lowerer keeps a
      lexical resource cleanup stack and emits release operations before
      `return`, `break`, and `continue`; managed early-return and loop-break
      scopes are covered by MIR verification. Provider errors and cancellation
      still rely on runtime cleanup and remain follow-up work.
  - [ ] **M03.3 — Add structured-concurrency instructions.** Model spawn, await,
    join, cancel, and select with lexical task-group ownership.
    - [x] **M03.3a — Establish typed task lifecycle primitives.** MIR now owns
      task and task-group IDs, verifies internal async spawn signatures and
      lexical close on normal returns, and rejects unsupported backend execution
      until scheduling and source lowering are ready. Select and non-normal
      cleanup remain follow-up work.
    - [x] **M03.3b — Lower direct async bindings and awaits.** A checked direct
      internal async binding lowers to `Spawn`, and awaiting its local task
      lowers to `Await`; the lifecycle verifier rejects any unclosed child.
      Awaited external Provider calls lower through resolved external `Call`,
      whose VM dispatch owns future suspension/resumption; async bindings to
      external calls, join/cancel syntax, and select remain fail-closed.
    - [x] **M03.3c — Execute lexical task-group drain.** `Join` lowers to the
      v1 `JoinTasks` instruction over its resolved child handles. The scheduler
      resumes the parent only after every still-live child completes and reaps
      each child exactly once; awaited children are safe to omit from the drain.
      Cancellation delivery and select remain follow-up work.
  - [ ] **M03.4 — Add resolved builtin and external-call instructions.** Include
    signature/effect/retention identity and no unresolved callee text.
    - [x] **M03.4a — Execute resolved external calls through MIR bytecode.** A
      checked `.rssi` symbol is represented by `ExternalSymbolId`, emitted into
      the Artifact import table, verified against the bytecode call table, and
      dispatched through the same Provider binding as the legacy VM. Builtin
      identity and the remaining effect/retention facts stay open.
- [ ] **M04 — Lower checked HIR to MIR exactly once.** Backend code cannot
  inspect syntax AST or reconstruct semantic facts. MIR verification rejects
  unresolved calls, invalid ownership state, incomplete cleanup, and malformed
  structured-task scopes.
  - [ ] **M04.1 — Create the one-way HIR-to-MIR lowerer.** It consumes checked
    semantic facts, not syntax AST projections.
    - [x] **M04.1a — Introduce direct checked-HIR lowering.** Supported
      functions (checked local bindings, scalar/aggregate expressions, list
      indexing, assignments, structured `if`/`else`, conditional loops with
      `break`/`continue`, return, and resolved internal read/`mut`/`take`
      calls, standalone `take local`, plus lexical resource scopes) now lower from semantic HIR without
      constructing `ExecutableIr`; compiler output prefers that route and uses
      the explicit compatibility bridge only when a capability is not yet
      direct-lowerable. Internal task-group `async let`/`await` also lower to
      explicit MIR `Spawn`/`Await`; direct `await Host.call()` lowers to the
      resolved external `Call` and uses the VM's Provider-future suspension
      path. Async bindings to external calls and cancellation remain follow-up
      direct-lowering work.
  - [x] **M04.2 — Verify MIR ownership, resources, and task scopes.** The
    construction verifier runs ownership-mode, move/drop, resource-lifetime,
    resource-cleanup-over-CFG, and structured-task-close passes. Targeted
    invalid-MIR fixtures reject incompatible call effects, use-after-move,
    missing release on a reachable return, and an undrained task group.
  - [ ] **M04.3 — Enforce backend input boundaries.** Architecture tests reject
    syntax/HIR imports in VM, codegen, AOT, and JIT backend code.
    - [x] **M04.3a — Guard bytecode backend input boundaries.** Architecture
      tests now recursively inspect VM, MIR codegen, and JIT-lab sources plus
      Cargo metadata, rejecting frontend imports or dependencies. Rust AOT
      remains an explicit experimental migration item until it consumes MIR or
      verified bytecode rather than checked frontend representations.
- [ ] **M05 — Run old/new lowering differentially.** The same corpus must
  produce equivalent diagnostics, external imports, termination reasons,
  values, cleanup behavior, and deterministic usage reports.
  - [ ] **M05.1 — Add pure-control-flow differential fixtures.** Compare return
    values, errors, and usage reports.
    - [x] **M05.1a — Establish the dual-path harness.** Declarative capability
      stages and pure-control-flow fixtures lower directly from checked HIR,
      verify, execute through the test-only MIR reference interpreter, and
      compare return values with the legacy VM. A DualPath fixture cannot hide
      a fallback through `ExecutableIr`.
    - [x] **M05.1b — Close the initial MIR/VM bytecode loop.** The same scalar,
      CFG, direct-call, `read`, `mut`, and `take` fixtures now compile MIR
      directly to a bytecode artifact, pass the ordinary bytecode verifier, and
      execute in the existing VM before their values are compared with both
      older paths. Error, usage, cleanup, and async/provider report parity
      remain follow-up work.
  - [ ] **M05.2 — Add ownership/resource differential fixtures.** Compare move
    failures, retain behavior, cleanup counts, and resource limits.
    - [x] **M05.2a — Compare retaining external calls.** A checked
      `retains(param)` call now proves that both legacy and direct-MIR bytecode
      dispatch the same retained value, return the same result, and record the
      same Provider-call usage. Move-failure, cleanup-count, and resource-limit
      comparisons remain follow-up work.
    - [x] **M05.2b — Compare resource-scope cleanup.** A checked external
      resource scope now executes through both the legacy and direct-MIR
      bytecode paths against a counted Provider resource. Both paths must
      return the same value, report the same Provider/resource usage, and
      finalize exactly once per run. The same fixture also exhausts a zero
      resource budget and requires both paths to retain a report with the same
      `ResourceExhausted` Provider failure and usage. Move-failure comparison
      remains follow-up work.
  - [ ] **M05.3 — Add async/provider differential fixtures.** Compare task
    cancellation, external-call order, deadlines, and Provider traces.
    - [x] **M05.3a — Compare awaited async Provider calls.** A cooperative
      Provider future that deliberately yields once executes through both the
      legacy executable-IR path and direct checked-HIR MIR bytecode. Both paths
      return the same value and record the same Provider-call usage.
      The stable Provider-trace fields (identity, symbol, byte counts, and
      result code) also agree; elapsed time remains observational telemetry.
      Call ordering remains follow-up work.
    - [x] **M05.3b — Compare Provider cancellation during suspension.** Both
      paths run a cooperative async Provider that cancels the shared execution
      token after its first pending poll. Both retain an execution report with
      the same structured `ProviderErrorCode::Cancelled` failure, usage, and
      stable Provider trace fields.
    - [x] **M05.3c — Compare Provider deadlines during execution.** Both paths
      run an async Provider whose work crosses a shared monotonic deadline.
      They retain the same `ProviderErrorCode::DeadlineExceeded` failure, usage,
      and stable Provider trace fields.
    - [x] **M05.3d — Compare sequential external-call ordering.** Two awaited
      async Provider calls execute in source order through both paths. Their
      values, usage, and stable traces agree, with the trace symbol sequence
      fixed as `Host.first`, then `Host.second`. Task-group child cancellation
      remains follow-up work.
  - [ ] **M05.4 — Gate replacement on corpus parity.** New lowering cannot become
    default until all supported Core fixtures agree.
- [ ] **M06 — Delete the source-shaped executable IR.** Remove nested
  `If`/`For`/`Match`/`With` backend nodes, string type/callee identities, and
  `ExecutableStmt::Unknown`/`ExecutableExpr::Unknown` only after M05 passes.

### 3. Code generation, verifier, and VM boundary

- [x] **V01 — Remove compiler-to-VM dependency.** Cargo architecture tests
  reject compiler dependencies on the VM and the VM cannot depend on compiler,
  syntax, semantics, or lowering internals.
- [ ] **V02 — Extract `rsscript-codegen-vm`.** The sole bytecode-emission path is
  `VerifiedMir -> BytecodeModule`; source, HIR, package, and SDK entry points are
  forbidden in the codegen crate.
  - [x] **V02.1 — Create a MIR-only codegen crate.** Its manifest may depend on
    MIR, ABI, and bytecode model but not VM, compiler, syntax, package, or SDK.
    `rsscript-codegen-vm` now owns scalar-CFG Artifact emission; architecture
    tests enforce its dependency closure and verify SDK MIR builds use
    codegen → verifier → VM-token loading.
  - [ ] **V02.2 — Lower the scalar MIR subset to bytecode.** Preserve source maps
    and deterministic module ordering.
    - [x] **V02.2a — Prove the transitional VM-local adapter.** The current
      MIR-only adapter emits the scalar CFG and direct-call subset through the
      existing verified bytecode envelope. It is deliberately housed in the VM
      only until `rsscript-codegen-vm` can own the wire model without exposing
      VM-private register structures.
  - [ ] **V02.3 — Lower resources, async, builtins, and external calls.** Add
    codegen fixtures for every Core MIR instruction.
    - [x] **V02.3a — Emit the linear resource lifetime subset.** Resource
      acquisition carries its defined source value and emits `Move`; release
      emits `ResourceDrop`, with the ordinary bytecode verifier covering the
      resulting Artifact. Async scheduling, builtins, and non-normal cleanup
      remain follow-up work.
    - [x] **V02.3b — Emit direct spawn/await task bytecode.** MIR task IDs map
      to dedicated registers and direct async functions emit `SpawnTask` and
      `AwaitJoin`; lexical group join emits `JoinTasks`. The ordinary verifier
      validates task-handle definitions and call shapes, and the migration suite
      executes spawned, awaited, and drained children in the VM. Awaited async
      external calls use the existing verifier-checked `CallExternal`, whose VM
      dispatch parks and resumes the current task around the Provider future;
      async bindings, cancellation, and select remain follow-up work.
    - [x] **V02.3c — Emit explicit retain/drop ownership boundaries.** Retain
      remains a verifier-visible semantic fact with no implicit VM copy, while
      drop clears its proven-dead register before frame teardown. Codegen tests
      assert the emitted cleanup sequence and verify the ordinary Artifact;
      source-level retain/drop lowering remains follow-up work.
    - [x] **V02.3d — Emit owned list construction.** `MakeList` maps typed MIR
      value IDs to verifier-checked v1 item registers. Lowering and SDK
      migration fixtures prove source list literals reach a verified Artifact;
      non-list aggregate and pattern instructions remain fail-closed.
    - [x] **V02.3e — Emit owned map construction.** `MakeMap` maps ordered
      resolved key/value value-ID pairs to verifier-checked v1 map entries.
      Lowering and dual-path migration fixtures prove map literals reach and
      execute through verified bytecode; non-map aggregate and pattern
      instructions remain fail-closed.
    - [x] **V02.3f — Emit JSON object construction.** `MakeObject` maps JSON
      data field names and resolved value IDs to verifier-checked v1 object
      fields. The migration corpus compares source JSON-object output across
      old and MIR bytecode paths; language record layouts remain fail-closed.
    - [x] **V02.3g — Emit resolved list indexing.** `ListGet` maps its resolved
      list and index value IDs to a verifier-checked v1 list read. The
      dual-path corpus proves typed list-local indexing executes identically;
      other indexing shapes remain fail-closed.
  - [x] **V02.4 — Switch reviewed builds to the compiler bytecode boundary.**
    SDK source, interface, and package builds delegate checked HIR → MIR →
    `codegen-vm` Artifact emission to the compiler's VM-free `bytecode`
    feature, then package the resulting provider-neutral Artifact. The SDK no
    longer owns a normal VM compile adapter or selects MIR/codegen crates
    directly. The legacy executable-IR fallback remains confined to the
    opt-in compatibility adapter for capabilities that MIR intentionally
    rejects; architecture tests reject VM compile-helper calls from reviewed
    build methods.
- [ ] **V03 — Make the verifier construct the only executable program type.**
  Untrusted bytes decode and verify to a private-field `VerifiedModule`; public
  APIs cannot construct or mutate it and VM constructors accept nothing else.
  - [x] **V03.1 — Define private-field verifier-owned program phases.** The v1
    `VerifiedBytecode` envelope and v2 `VerifiedProgramV2` both have private
    fields and are constructed only by their bounded verifier paths; loaders
    accept verifier output rather than caller-built bytes or instruction data.
  - [x] **V03.1a — Require the verifier token at the SDK/VM boundary.** SDK
      Artifact verification now constructs `VerifiedBytecode` first and VM
    decoding accepts that opaque verifier output. VM-internal typed program
    ownership remains follow-up work.
  - [x] **V03.1b — Establish the v2 verifier-owned program phase.** The v2
    canonical decoder returns private-field `VerifiedProgramV2` only through
    `BytecodeV2Verifier`; the future VM integration can consume that phase
    object without exposing caller-built decoded instruction vectors.
  - [x] **V03.2 — Move instruction/data-flow verification into bytecode.** VM no
    longer independently validates decoded program structure. The register-VM
    decoder now accepts only `VerifiedBytecode`; duplicate payload, control-flow,
    register, and import-table validation was deleted and is mechanically
    rejected from returning.
  - [ ] **V03.3 — Restrict VM constructors.** Delete constructors accepting raw
    bytecode, executable IR, or decoded mutable instruction vectors.
    - [x] **V03.3a — Delete raw bytecode VM constructors.** The public VM loader
      accepts only `VerifiedBytecode`; SDK and CLI verification own byte input.
- [ ] **V04 — Make the VM execution-only.** Remove MIR/executable-IR lowering,
  bytecode encoding, Artifact packaging, compiler/source helpers, and duplicate
  payload verification from `rsscript-vm`.
  - [ ] **V04.1 — Delete VM source/HIR/executable-IR compile entry points.**
    Preserve only load/link/execute APIs over `VerifiedModule`.
    - [x] **V04.1a — Isolate legacy executable-IR lowering.** The default VM
      dependency closure no longer links `rsscript-exec-ir`; its source-shaped
      lowerer and compiler-facing entry point require the explicit
      `legacy-exec-ir` compatibility feature. The SDK migration adapter opts in
      deliberately while direct MIR builds continue through codegen, verifier,
      and the VM token boundary. Removing this compatibility feature entirely
      remains follow-up work.
  - [ ] **V04.2 — Delete VM bytecode encoder and Artifact assembly.** Move all
    production encode logic to codegen/Artifact crates.
    - [x] **V04.2a — Isolate legacy register-unit encoding.** The default VM
      closure no longer compiles the legacy register-unit Artifact encoder or
      assembly helper. Both require `legacy-exec-ir`, alongside the only caller
      that still lowers source-shaped executable IR. `rsscript-codegen-vm`
      remains the production MIR-to-Artifact writer; deleting the compatibility
      encoder entirely remains follow-up work.
  - [ ] **V04.3 — Delete duplicate VM payload verifier.** Keep runtime defensive
    assertions only; malformed-byte handling belongs to bytecode verifier.
    - [x] **V04.3a — Isolate compatibility raw-byte verification.** The default
      VM no longer compiles a byte-slice verification entry point; it only decodes
      the opaque `VerifiedBytecode` token. The retained raw verifier is coupled
      to the legacy encoder's self-check behind `legacy-exec-ir`. Deleting that
      compatibility self-check with the legacy encoder remains follow-up work.
- [ ] **V05 — Remove experimental state from Core VM program objects.** JIT,
  OSR, deopt, branch/call profiles, and native tier state live in experiment-owned
  side tables keyed by stable function IDs.
  - [ ] **V05.1 — Introduce experiment-owned `JitState` side tables.** Key state
    by stable function IDs and lifetime-bound execution instances.
  - [ ] **V05.2 — Move profiles, OSR, deopt, and native code handles.** Remove
    these fields from `RegFunction` and program types.
  - [ ] **V05.3 — Make Core VM build without JIT data structures.** Add a
    dependency and layout regression test.
- [ ] **V06 — Split VM primitives from deterministic core library.** VM Core
  keeps frames, registers, scheduler, cancellation, resource table, limits,
  dispatch, and external calls. JSON/YAML, regex, compression, encoding, hashes,
  date utilities, and collection algorithms move behind a versioned builtin
  registry or Core library runtime.
  - [ ] **V06.1 — Define the builtin registry contract.** Include `BuiltinId`,
    signature, determinism, cost, and version/digest information.
  - [ ] **V06.2 — Move pure library families incrementally.** Start with encoding
    and collection helpers, then JSON/YAML, regex, compression, hashes, and date
    utilities while preserving differential results.
  - [ ] **V06.3 — Reduce VM dependencies to execution primitives.** Verify VM
    Core no longer directly depends on library implementation crates.
- [ ] **V07 — Classify the intrinsic catalog.** Every entry is exactly one of a
  VM primitive, deterministic builtin, or Provider external symbol; adding a
  library API must not silently change the VM instruction set.

### 4. Bytecode and compatibility contracts

- [x] **B01 — Establish a bounded sectioned Artifact envelope.** Required and
  optional sections, canonical ordering, length/count limits, hashes, checksum,
  unknown-section handling, malformed corpora, and fuzz coverage are present.
- [x] **B02 — Define the typed bytecode v2 wire model.** Use bounded decoding,
  numeric opcodes, numeric IDs, fixed operand layouts, and separate type,
  constant, function, import, export, code, and optional debug tables.
  - [x] **B02.1 — Introduce a numeric v2 executable model.**
    `rsscript-bytecode::v2` now owns typed function/type/constant/import/register
    identities, numeric opcodes with exact operand arity, and independent
    structural checks for register, function, import, constant, and jump IDs.
    Exports/debug tables and a production Artifact writer remain follow-up;
    v1 stays the deployed artifact path.
  - [x] **B02.2 — Add a bounded canonical v2 codec.** The v2 codec emits
    array-shaped `[numeric_opcode, operands]` records, decodes only canonical
    CBOR, rejects unknown numeric opcodes, and invokes the typed structural
    verifier before returning a program. Artifact section integration and the
    remaining tables remain follow-up work.
  - [x] **B02.3 — Model import, export, and optional debug tables.** V2 now
    carries numeric Artifact-import links, numeric function exports, and
    function/instruction source locations as separate tables. The verifier
    rejects invalid export/debug references and inverted source ranges; section
    layout in a deployed v2 Artifact remains follow-up work.
- [ ] **B03 — Generate codec and verification rules from one instruction
  schema.** The schema generates Rust instruction types, encoder, bounded
  decoder, operand validation, documentation, and fuzz seeds; string field maps
  and `serde_json::Value` verification are removed.
  - [x] **B03.1 — Make v2 opcode schema the single source of truth.** One
    `INSTRUCTION_SCHEMA_V2` table now owns numeric tags, names, operand classes,
    and arity; it drives raw decode lookup, structural operand validation, and
    generated Markdown reference output. A bounded arbitrary-byte property
    corpus proves the v2 decoder cannot panic; explicit seed files and Artifact
    integration remain follow-up work.
  - [x] **B03.2 — Verify v2 instruction-CFG register data flow.** The typed
    verifier computes per-instruction predecessor intersections, rejects reads
    not defined on every reachable path, and rejects fallthrough past a function
    end. Type/resource/task-state data-flow remains follow-up work.
- [x] **B04 — Separate all compatibility versions.** Container format, language
  semantics, bytecode ISA, Core library ABI, Provider ABI, analysis schema, and
  compiler provenance have explicit independent values. Language compatibility
  must not be inferred from `CARGO_PKG_VERSION`.
  - [x] **B04.1 — Declare independent compatibility constants.** Define container,
    language, ISA, Core library, Provider, analysis, and provenance versions.
    - [x] **B04.1a — Separate language semantics from compiler provenance.** v1
      artifacts emit `LANGUAGE_SEMANTICS_VERSION`; the verifier accepts the
      explicit `SUPPORTED_LANGUAGE_SEMANTICS` range, while the compiler package
      version remains provenance only. Neutral package analysis consumes the
      same ABI-model language constant. Container and `BYTECODE_ISA_VERSION`
      are now explicit, and the ISA is serialized in each Artifact header and
      rejected by the verifier before payload validation;
      Core-library ABI is explicit in the Artifact header and verified
      independently before payload validation. Artifact bundles also accept
      only explicit source/package analysis schema IDs at their owning
      boundary; compiler provenance remains provenance-only.
  - [x] **B04.2 — Validate each version at its owning boundary.** Container
    magic/sections validate at decode; bundle loading validates the analysis
    schema allowlist; the bytecode verifier validates language, ISA, and Core
    library ABI before payload loading; Provider ABI continues to validate at
    link. Focused malformed/unknown-version tests exercise fail-closed paths.
  - [x] **B04.3 — Add supported-range fixtures.** The bytecode suite covers an
    accepted declared N-1 language input and rejects unknown container, language,
    runtime-ABI, ISA, and Core-library ABI versions before execution.
    - [x] **B04.3a — Exercise declared language ranges and container rejection.**
      The bytecode suite verifies an explicit N-1 compatibility range and
      rejects an unknown container major before decoding sections.
  - [ ] **B05 — Preserve a versioned compatibility corpus.** Keep read-only v1
  fixtures, malformed v1/v2 inputs, N-1 schema fixtures, deterministic
  cross-platform bytes, and explicit unknown-version/section fail-closed tests.
  - [ ] **B05.1 — Check in read-only v1 bundles and expected reports.** Retain
    loaders after v2 becomes the writer.
    - [x] **B05.1a — Execute a checked-in v1 reference bundle.** The SDK
      compatibility suite decodes a static v1 bundle rather than regenerating
      it, verifies it through the ordinary public reader, links it without
      Providers, and compares its completed execution result with a checked-in
      expected report. Additional v1 package and failure fixtures remain open.
  - [ ] **B05.2 — Add malformed and compatibility fixture suites.** Cover every
    section, table, opcode, version, and size boundary.
    - [x] **B05.2a — Preserve a static malformed v1 boundary case.** A
      checked-in trailing-byte mutation is applied to the read-only reference
      bundle and must be rejected at the bundle boundary before verification or
      execution. Per-section/table/opcode/version/size fixtures remain open.
  - [ ] **B05.3 — Test deterministic bytes across supported platforms.** Compare
    bundle and analysis bytes from identical snapshots.

### 5. Provider contract and authoring SDK

- [x] **P01 — Use structured external signatures.** Artifact and Provider
  contracts use canonical `WireType`, data effects, retention, async shape,
  signature hashes, and an explicit runtime ABI.
- [x] **P02 — Carry runtime context into Provider calls.** Cancellation,
  monotonic deadline, byte/output budgets, call identity, trace sink, authority,
  blocking/async lanes, and runtime-owned resource registration reach the
  callable; errors are structured.
- [x] **P03 — Use generation-safe resource handles.** Runtime resource tables
  reject stale handles and report created, cleaned, live, peak, and cleanup
  failures.
- [x] **P04 — Make semantic interface descriptors the bindgen input.** `.rssi`
  is parsed and canonicalized once by the semantic compiler into a versioned
  `InterfaceDescriptor`; bindgen must not duplicate syntax/type normalization.
  - [x] **P04.1 — Define `InterfaceDescriptorV1`.** Include canonical symbols,
    `WireType`, effects, retention, async shape, resources, and signature hashes.
    - [x] **P04.1a — Introduce the versioned semantic function descriptor.**
      `rsscript-semantics` owns `InterfaceDescriptorV1` with canonical external
      symbols, structured signatures (WireType/effects/retention/async),
      canonical signature hashes, public resource declarations, and a schema
      id. Serialized snapshots follow in P04.2. Descriptor bytes are now
      produced deterministically for binding and snapshot consumers.
  - [x] **P04.2 — Emit descriptors from semantic checking.** Immutable semantic
    snapshots derive deterministic descriptor bytes, and focused coverage proves
    aliases cannot alter canonical namespace-qualified ABI facts.
    - [x] **P04.2a — Derive descriptors from immutable semantic snapshots.**
      `SemanticDatabase::interface_descriptors()` derives contracts from the
      checked interface programs; tests prove it agrees with direct interface
      derivation, including canonical namespace-qualified resource identity.
  - [x] **P04.3 — Replace bindgen source parsing.** Bindgen accepts only the
    versioned descriptor, rejects unsupported versions, and no longer owns a
    syntax dependency or source-parsing entry point.
    - [x] **P04.3a — Remove bindgen syntax ownership.** All official Provider
      build scripts derive an `InterfaceDescriptorV1` in semantics and pass it
      to `ProviderInterface::from_descriptor`; bindgen no longer depends on
      `rsscript-syntax` or exposes a source-parsing entry point.
- [ ] **P05 — Generate typed Rust Provider APIs.** Generate sync/async traits,
  typed parameters/results, resource wrappers, descriptor/signature constants,
  registration glue, mocks, completeness checks, and conformance skeletons.
  `NativeValue` remains only in generated adapters.
  - [ ] **P05.1 — Generate scalar and aggregate Rust type mappings.** Cover unit,
    booleans, integers, floats, strings, bytes, lists, options, results, tuples,
    records, and variants.
    - [x] **P05.1a — Generate scalar and aggregate method signatures.** Generated
      Provider traits now map unit, bool, numeric, string, bytes, lists,
      options, results, tuples, and qualifiers to Rust types. Named records,
      variants, and resources remain adapter-layer values until P05.3.
  - [x] **P05.2 — Generate sync and async Provider traits.** Method signatures
    reflect descriptor parameters, results, effects, and async shape. Bindgen
    emits Rust `async fn` methods and matching `ProviderCallMode::Async` from
    the same semantic descriptor; regression coverage also proves take and
    retention contract facts remain present in generated registration metadata.
  - [x] **P05.3 — Generate resource wrappers and adapter glue.** Resource values
    use typed generation-safe handles; generated wrappers expose canonical
    `WireValue::Resource` conversion with a descriptor-supplied numeric resource
    type, while adapters isolate legacy `NativeValue` conversion.
    Descriptor-declared resource names now map function parameters/results to
    generated wrappers, including nested aggregate positions, while only each
    wrapper's explicit `from_native`/`into_native` adapter touches the legacy
    dynamic representation.
  - [x] **P05.4 — Generate registration, mock, and completeness tests.** Provider
    implementations fail to compile or conformance-test when symbols drift.
    Generated contracts now include registry registration glue, a call-recording
    sync/async mock that builds a descriptor-complete implementation map, and a
    `#[cfg(test)]` registration skeleton. The same fail-closed registry path
    rejects missing, undeclared, or signature-mismatched symbols for real and
    generated mock Providers.
- [ ] **P06 — Tighten the canonical wire value model.** Replace JSON, string
  type/field identity, and generic `Native { type_name, id }` escape hatches
  with typed records, variants, lists, resources, and generation-safe handles;
  JSON becomes an explicitly declared extension codec.
  - [x] **P06.1 — Define typed wire records, variants, and resources.**
    `rsscript-abi-model` owns positional `WireValue` records/variants plus
    numeric type/field/variant/resource identities and generation-safe resource
    handles; canonical values contain no free-form type or field-name identity.
  - [ ] **P06.2 — Implement the compatibility adapter.** Convert legacy
    `NativeValue` at generated boundaries while Core contracts use wire values.
    A parallel `WireInterpreterFn` now carries canonical `WireValue` arguments
    through the same cancellation/deadline context as the legacy callable;
    bindgen now chooses `WireValue` for unresolved named types rather than
    extending its NativeValue fallback; registry/VM dispatch migration remains
    open.
    The reference VM now admits descriptor-linked synchronous scalar
    `WireValue` Providers (`Unit`, booleans, integer, float, string, bytes)
    without routing their callable through `NativeInterpreterFn`; structured
    wire values remain fail-closed until the type-table adapter exists.
    - [x] **P06.2a — Bridge generation-safe resource handles.** Provider
      runtime handles now convert to/from the canonical numeric wire handle
      with a descriptor-supplied resource type; no legacy type-name string is
      used at that boundary. Aggregate `NativeValue` adapters remain open.
    - [x] **P06.2b — Generate canonical resource-wire adapters.** Generated
      Provider resource wrappers now encode/decode `WireValue::Resource` using
      explicit numeric resource identities; legacy `NativeValue` methods are
      visibly compatibility-only adapters.
  - [ ] **P06.3 — Migrate official Providers and mocks.** Each migration keeps
    signature, error, resource, and payload-budget conformance fixtures green.
    The scalar `time`, `entropy`, `log`, and rooted `fs` Providers now use
    `WireInterpreterFn`; the conformance kit validates wire callables directly,
    without a `NativeValue` detour. `fs` retains its host-chosen root authority
    and runtime byte-budget checks while its text arguments/results become wire
    values. The primary embedded-report-pipeline now uses the same wire callable
    form for its in-memory and rooted filesystem/log providers, proving the
    reviewed Artifact/link/run path without the legacy adapter.
    Structured and asynchronous Provider migrations remain follow-up.
  - [ ] **P06.4 — Remove legacy escape variants from canonical APIs.** JSON stays
    only behind a named extension codec with explicit interface declaration.
    The reviewed SDK Provider façade no longer re-exports `NativeValue` or
    `NativeInterpreterFn`; both remain compatibility-only while report and VM
    adapters still carry the legacy representation.
- [x] **P07 — Remove policy-shaped authority from Core ABI.** `HostCallContext`
  carries host-defined labels to Provider calls without Core interpreting an
  authorization policy. The runtime reports required symbols; provider profiles
  remain responsible for any authority narrowing.
- [x] **P08 — Complete async/resource conformance.** Async cancellation and
  deadlines, blocking lanes, exact-once cleanup across terminal paths,
  reentrancy, panic containment, default redaction, and request/response payload
  limits are enforced by the dispatcher and covered by focused fixtures.
  - [x] **P08.1 — Add async cancellation and deadline fixtures.** The VM async
    dispatcher is manually polled through a pending Provider future, then verifies
    that cooperative cancellation and a monotonic deadline are both observed after
    suspension before a successful Provider result can escape.
  - [x] **P08.2 — Add resource cleanup state-machine fixtures.** A single
    verified-bytecode state-machine fixture registers an exact-once Provider
    resource and exercises success, script error, Provider error, cancellation,
    deadline, and cleanup failure. Every terminal path retains usage evidence;
    cleanup failure is counted without leaving a live resource.
  - [x] **P08.3 — Add lane, reentrancy, and panic boundary fixtures.** Blocking
    lane admission, non-reentrant call exclusion (including suspended async
    futures), and unwind-style host failure containment are enforced by the
    dispatcher and covered by focused fixtures.
    - [x] **P08.3a — Contain unwind-style Provider failures.** Both synchronous
      callables and asynchronous future polls are caught by the VM dispatcher and
      converted to structured internal Provider errors. Abort panics and native
      faults remain isolated-runner concerns; reentrancy fixtures remain open.
  - [x] **P08.4 — Add redaction and payload-limit fixtures.** Reports and traces
    remain bounded and do not expose sensitive Provider payloads by default.
    - [x] **P08.4a — Redact Provider-controlled failure content by default.** The
      portable execution report keeps only the stable Provider error code plus
      aggregate telemetry; Provider message/details and per-call traces do not
      serialize unless a host keeps separately redacted diagnostics.
    - [x] **P08.4b — Enforce Provider request/response payload limits.** The
      synchronous and asynchronous VM dispatchers reject oversized requests
      before Provider code runs and oversized responses before they escape the
      boundary, even when an individual Provider omits its own checks.

### 6. Stable SDK and product workflows

- [x] **A01 — Establish phase-typed execution.** The supported path is immutable
  snapshot/build bundle -> verify -> link -> bounded execution report, with
  verification and link errors kept distinct.
- [x] **A02 — Remove SDK root glob re-exports.** Implementation crates cannot
  silently add SDK public symbols.
- [x] **A03 — Shrink the SDK to reviewed façade modules.** Default public API is
  limited to compiler/check, Artifact/verification, Provider registration,
  runtime/linking, execution request/limits/report, diagnostics, and operation
  control. Package review/risk, AOT, JIT/OSR, register VM, opcode, and legacy
  convenience APIs are not re-exported. Explicit façade modules, feature-gated
  compatibility exports, a reviewed inventory, and a default-path export
  snapshot now guard the complete supported SDK surface.
  - [x] **A03.1 — Inventory the existing SDK surface.** Classify each export as
    stable façade, compatibility-only, experimental, or internal.
  - [x] **A03.2 — Create explicit façade modules.** Expose only compile,
    artifact, provider, runtime, report, diagnostics, and operation APIs.
    - [x] **A03.2a — Publish reviewed module paths.** `compile`, `artifact`,
      `provider_api`, `runtime`, `report`, `analysis`, and `operation` now
      provide the reviewed embedding surface; root compatibility exports remain
      until A03.3 migrates legacy callers behind opt-in modules.
  - [x] **A03.3 — Move compatibility and experimental APIs behind opt-in modules.**
    JIT, AOT, review/risk, register VM, and opcode APIs disappear from defaults.
    The transitional root surface is now gated by the explicit `compatibility`
    feature; the default and `execution` builds expose the reviewed façade
    modules only, while the migration corpus opts in deliberately.
  - [x] **A03.4 — Add public API snapshots.**
    `sdk-api-snapshot.v1.toml` records the normalized export surface of every
    reviewed façade module. The default SDK test recomputes its SHA-256 digests,
    so CI rejects unreviewed stable-surface growth or removal; compatibility and
    execution feature builds retain their dedicated architecture gates.
- [ ] **A04 — Remove invalid phase states and report-losing paths.** Public types
  do not use optional fields to represent incompatible phases; script,
  Provider, cancellation, deadline, and budget termination always return a full
  execution report. Only host/protocol/internal-invariant failures use outer
  errors. Deployable `ArtifactBundle`s now reject bytecode without an immutable
  snapshot digest, and in-memory SDK builds derive that digest from the complete
  source/interface input; the remaining phase-type and report-path audit stays
  open.
  - [ ] **A04.1 — Audit all public phase types.** Replace optional phase fields
    and cross-phase enums with built/verified/linked/report-specific types.
  - [ ] **A04.2 — Audit every execution convenience API.** Script and Provider
    failures return `ExecutionReport`; only host/protocol failures return errors.
  - [ ] **A04.3 — Add compile-time and runtime phase tests.** Invalid transitions
    are unrepresentable through public constructors and report retention is
    tested for every termination reason.
- [x] **A05 — Make execution bounded by default.** Unbounded execution requires
  an explicitly named trusted-host constructor; per-run limits live on the
  execution request.
- [x] **A06 — Ship Artifact Bundle, `rss verify`, and neutral `rss diff`.** Both
  single-file and package builds produce analysis/provenance-bound bundles. The
  persisted Bundle schema and integrity checks are owned by
  `rsscript-artifact`; SDK only composes it into phase APIs. Direct
  `source_analysis.v1` evidence is constructed and decoded through the
  Artifact-owned `SourceAnalysisV1` model; package analysis remains an
  explicitly bounded compatibility adapter pending its own typed schema.
- [x] **A07 — Complete semantic diff evidence.** Add read/mut/take, retention and
  escape, resource acquire/transfer/cleanup, structured-task fan-out and
  cancellation, call graph/recursion, Provider requirements, and diagnostic
  additions/removals while remaining policy-neutral.
  - [x] **A07.1 — Diff ownership and call contracts.** Report effect, parameter,
    retention, escape, and external signature changes. Semantic diff now carries
    canonical Artifact import contracts (parameter names/effects/retention and
    structured types, result, async shape, ABI and signature hash), so a changed
    `read`/`mut`/`take` or retention contract is evidence rather than an opaque
    hash transition. Neutral package analysis likewise emits explicit local
    parameter effects/types/retention plus return contracts, including source
    escape qualifiers, for every public function.
  - [x] **A07.2 — Diff resources and concurrency.** Report acquire/transfer/close,
    task fan-out, await/select, cancellation, and cleanup-path changes. Neutral
    analysis records lexical `with` acquisition/scope-exit cleanup and
    cancellation cleanup, explicit `take` transfers of those managed bindings,
    and task-group fan-out/select/drain facts. Ordinary value moves are excluded
    from resource-transfer evidence.
  - [x] **A07.3 — Diff graph and diagnostic facts.** Report call graph/recursion,
    Provider requirements, and diagnostic additions/removals. Neutral analysis
    now records resolved call edges and recursion participants; semantic diff
    compares them alongside versioned Provider import requirements and
    coordinate-free diagnostic fact sets.
  - [x] **A07.4 — Version and fixture the neutral schema.** JSON and Markdown
    structural-evidence goldens round-trip through `rsscript.semantic_diff.v2`;
    the contract test rejects policy/risk/verdict vocabulary in the emitted
    neutral facts.
    - [x] **A07.4a — Version diagnostic evidence explicitly.**
      `rsscript.semantic_diff.v2` adds coordinate-free diagnostic fact sets;
      v1 remains available as the prior schema, while v2 is validated in the
      SDK and CLI workflow tests and contains no policy verdict fields.
  - [x] **A07.5 — Place semantic-diff ownership at the Artifact boundary.**
    `rsscript-artifact` owns the v2 schema, fact types, and bundle-to-bundle
    construction; SDK retains only explicit reviewed re-exports.
- [x] **A08 — Run scripts out of process by default.** `rss run` uses the
  versioned child protocol; trusted in-process execution is explicit.
- [ ] **A09 — Harden the reference Linux runner profile.** Add allowlisted
  Provider profiles, namespace/syscall/filesystem/network controls where
  available, parent-enforced kill-on-deadline, protocol/disconnect fuzzing, and
  tests separating runner termination from VM termination. Continue to state
  that this is defense in depth rather than a universal sandbox.
  - [ ] **A09.1 — Introduce explicit runner profiles.** Profiles preinstall
    allowlisted Providers and their host-owned roots/endpoints; requests cannot
    supply Provider code, library paths, credentials, or authorities.
    - [x] **A09.1a — Ship the fail-closed reference profile.** The versioned
      protocol carries only the `no_providers` profile; runner selection maps it
      to a host-owned empty registry, and the schema rejects provider code,
      library paths, credentials, roots, and authority injection fields. Every
      runner response now returns a non-secret profile ID/version/descriptor
      digest that the parent checks against its requested profile before it
      accepts a report.
  - [ ] **A09.2 — Add Linux isolation adapters.** Implement optional namespace,
    syscall, filesystem, network, and cgroup controls with capability detection
    and fail-closed profile requirements. The strict Linux/Android child path
    now installs kernel `no_new_privs` before runner code begins; namespace,
    syscall-filter, filesystem, network, and cgroup adapters remain open.
  - [ ] **A09.3 — Complete parent-side containment.** Cover process-tree kill,
    deadline, stdout/stderr/report limits, abnormal exits, and child disconnects.
    The child protocol now fail-closes contradictory response states: only a
    completed runner response may carry a report, and every rejection requires
    an error without a report. The parent now treats either bounded pipe overflow as an immediate reason to
    terminate the guarded process tree, reap the root, and join both readers;
    a successful child exit with an incomplete response frame is now reported as
    a reaped runner/protocol failure rather than a script report, with focused
    disconnect-path coverage. Process-tree fault injection remains open.
  - [ ] **A09.4 — Fuzz protocol and runner failure paths.** Exercise framing,
    malformed messages, oversized inputs, incomplete I/O, and termination
    separation without calling it a universal sandbox. The bounded protocol now
    has exhaustive truncated-frame and oversized-length regression coverage plus
    a coverage-guided request/response round-trip target; runner-process fault
    injection remains open.

### 7. Adoption, evidence, and maintenance

- [x] **E01 — Gate representative Core performance.** CI records check,
  compile, Artifact verify, VM, Provider boundary, cancellation, Artifact size,
  and deterministic usage metrics against the checked SLO fixture.
- [ ] **E02 — Add two complete product examples.** Keep the embedded Provider
  replacement pipeline and add a reviewable async/resource workflow; each must
  contain source, interfaces, generated Provider contract, memory and
  production-like Providers, Artifact identity, semantic-diff fixture, and
  success/failure reports for trusted and isolated execution.
  - [ ] **E02.1 — Upgrade the embedded report pipeline fixtures.** The example
    now derives deterministic semantic interface-descriptor bytes, reports their
    digest alongside the provider-neutral Artifact digest, and asserts an empty
    neutral self-diff. Trusted/isolated report snapshots remain follow-up work.
  - [ ] **E02.2 — Add an async/resource workflow example.** Demonstrate task
    groups, cancellation, cleanup, mock/production-like Providers, and failures.
    - [~] **E02.2.1 — Add the Provider-free structured-async baseline.**
      `examples/structured-async-pipeline` now proves the verified,
      provider-neutral `task_group` / `async let` / `await` execution path and
      bounded `ExecutionReport`. Cancellation, resource cleanup, Provider
      replacement, isolated-runner fixtures, and failure-report snapshots still
      need to be added before E02.2 can close.
- [ ] **E03 — Establish compatibility and conformance corpora.** Add source to
  diagnostic/HIR/MIR goldens, MIR to bytecode fixtures, old Artifact readers,
  cross-platform deterministic builds, Provider ABI compatibility, resource
  cleanup state machines, and interpreter/experimental-backend differential
  tests.
  - [ ] **E03.1 — Add source/semantic/MIR golden corpus.** Freeze diagnostics,
    normalized HIR/MIR, and lowering failures separately.
  - [ ] **E03.2 — Add Artifact/Provider compatibility corpus.** Cover old readers,
    ABI mismatch, replacement Providers, and deterministic bundles.
  - [ ] **E03.3 — Add execution state-machine corpus.** Cover budgets,
    cancellation, cleanup, Provider errors, and interpreter/experiment parity.
- [x] **E04 — Require ADR/RFC records for contract changes.** Core CI compares
  each change with its base revision and rejects syntax/semantics, ABI-model,
  MIR, bytecode, Provider ABI, or reviewed SDK changes unless the same change updates a
  numbered ADR. The checked-in template requires problem, non-goals,
  compatibility, migration, verifier/security impact, and Provider/backend
  impact; ADR 0001 records the initial typed Provider wire-value decision.
- [ ] **E05 — Add opt-in deterministic Provider record/replay.** This remains P2
  until Core boundaries are stable and must define replayability, normalization,
  redaction, external-state dependence, and persistence rules without claiming
  a security proof.

### Explicitly deferred

Release publication, crates.io distribution, registry/publish workflows, new
language syntax, new qualifiers, new public intrinsics, new official Providers,
new execution backends, new JIT tiers/speculation, full self-hosting, C backend
coverage, and built-in AI/Agent frameworks are not part of this TODO. They stay
frozen until the unchecked Core items above are complete and a separate product
decision reopens them.

## Exit criteria for this preparation phase

- Root default commands select only Core, applications, and the runner.
- Full `--workspace --all-features` maintenance tests remain available.
- CI has separate Core and experimental workflows.
- Workspace classification and dependency direction are machine checked.
- A canonical compilation/diagnostic baseline is checked in.
- New disabled `#[cfg(any())]` cemetery code is rejected.
- Scope freeze and migration ownership are visible from the roadmap.
