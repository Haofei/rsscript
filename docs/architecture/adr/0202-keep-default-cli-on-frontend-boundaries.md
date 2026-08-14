# ADR 0202: Keep the default CLI on frontend boundaries

- Status: Accepted
- Date: 2026-08-14

## Problem

The normal `rss check`, `rss fix`, and `rss fmt` commands used APIs re-exported
by `rsscript-compiler`. This made a frontend-only CLI build select a historical
compiler compatibility facade even though the commands only need semantic
checking, syntax formatting/linting, and diagnostics presentation.

## Decision

The default CLI depends directly on `rsscript-semantics`, `rsscript-syntax`,
and `rsscript-diagnostics`. The compiler dependency is optional and selected
only by the explicit execution/package or grammar-tool features that still
need compatibility APIs.

## Consequences

`cargo tree -p rsscript-cli` is now a useful executable guard for the intended
frontend-only default path. This does not yet move legacy package analysis or
the experimental AOT route out of compiler; those remain feature-gated
compatibility work rather than a dependency of ordinary authoring commands.
