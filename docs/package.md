# Package Reference

This document describes the package surface implemented by the current
prototype. It replaces the earlier v0.6 package-manager design draft. The
language specification remains authoritative for RSScript semantics; this
document governs package artifacts and command behavior only where it matches
the implementation and tests.

## Purpose

RSScript package tooling is a semantic review layer, not a replacement for
Cargo and not a registry service. It connects:

```text
rsspkg.toml
  + .rss implementation sources
  + .rssi public contracts
  + rsspkg.lock semantic identities
  + optional trusted native wrappers
  -> package check, review, diff, metadata, and REIR evidence
```

Dependency resolution succeeding does not imply that a graph is reviewable.
Unknown, native, build-time, and changed public-contract facts remain visible
to policy.

## Layout

A package normally contains:

```text
rsspkg.toml
rsspkg.lock
src/
  main.rss or library sources
interface/
  *.rssi
native/
  bindings.rssbind.toml
  rust/
```

Exact source selection comes from `rsspkg.toml`. Package reads are bounded and
reject unsafe path traversal and link shapes according to the platform support
described in [support.md](support.md).

## Providers

An interface-only dependency declares that the platform supplies its
implementation:

```toml
[dependencies]
platform-env = { path = "../platform-env", platform_provided = true }
```

The consuming package selects the concrete provider separately:

```toml
[providers]
platform-env = { package = "posix-env", version = "0.1.0" }
```

Provider packages declare implementations with
`[implements."<interface-package>"]`. The semantic lock records the selected
provider and its `interface_effective_hash`.

## Commands

The current command family is:

```text
rss pkg check
rss pkg ci
rss pkg review
rss pkg diff
rss pkg lock
rss pkg tree
rss pkg metadata
rss pkg vendor
rss pkg publish --dry-run
rss pkg add
```

Use `rss --help` for exact flags. `--json` and `--reir` outputs are structured
views over the same package facts. `publish` is validation-only; the repository
does not claim a complete hosted registry.

## Semantic Identity

The lock model distinguishes packages by name, version, source identity,
selected features, effective interface hash, implementation checksum, and
review metadata. A change to any review-relevant identity is an update event,
even if semver and compilation still succeed.

The effective interface is the normalized public `.rssi` contract after feature
selection. It includes review-visible call shapes such as:

- data effects (`read`, `mut`, `take`);
- return freshness and retention;
- resource and async boundaries;
- native and unsafe declarations;
- public names, types, and capabilities.

Package tooling consumes compiler-produced semantic facts. It must not infer
language contracts from generated Rust, README prose, or heuristic native
source scanning.

## Core Facade Contracts

Package examples and generated wrappers use the current interface shapes:

```rss
pub native fn Http.get(
    url: read Url,
) -> Result<fresh HttpResponse, HttpError>

pub native fn HttpResponse.text(
    response: read HttpResponse,
) -> fresh String

pub native fn Env.get(name: read String) -> Option<fresh String>

pub native fn Env.get_or_default(
    name: read String,
    default: read String,
) -> fresh String
```

These declarations are examples of package-visible contracts. The checked-in
`.rssi` interfaces remain the executable source of truth.

## Review Outputs

Package review reports:

- public contract and selected-feature deltas;
- direct and transitive dependency identities;
- package and graph risk;
- native/build-time execution facts;
- implementation and interface hashes;
- unknown or incomplete evidence;
- policy failures and evidence locations.

Human output is summary-first. Machine output is the integration surface for
CI, REIR, and other tools.

## Native Dependencies

Native wrappers are explicitly trusted host code. Authorization, immutable
snapshots, offline/frozen Cargo builds, artifact digests, cache ownership
checks, and ABI validation reduce accidental and supply-chain failures; they do
not sandbox machine code.

CLI execution denies native packages unless `--trusted-native` is supplied
under the `local-trusted` deployment profile. Third-party package inspection
must remain static and must not build or load native code.

## Mutation And Publication

Package lock, vendor, metadata, and generated artifacts use staged or atomic
publication where implemented. Operations are bounded by file count, depth,
and byte limits. The authoritative current limitations are tracked in
[status.md](status.md), including snapshot-first review and Windows secure-store
work that remains open.

## Non-Goals

- replacing Cargo's Rust dependency/build model;
- treating heuristic source scanning as proof;
- executing untrusted package code;
- providing a complete remote registry;
- making semver success equivalent to review approval;
- hiding unknown evidence to produce a pass.
