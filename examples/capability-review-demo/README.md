# Capability-aware review demo

An end-to-end walkthrough of RSScript as a **reviewable, capability-converging
front end for (AI-generated) code**: declare what powers a package may use,
review them ranked by risk, diff what a change *introduces*, and pin it so the
review is reproducible.

Two copies of one package tell the story:

- [`before/`](./before) — a data package that only reads a database.
- [`after/`](./after) — the same package after a "PR" adds an outbound HTTP call.

Each native boundary declares the capability it grants in `rsspkg.toml`, using
the canonical taxonomy in
[`crates/rsscript/src/capability.rs`](../../crates/rsscript/src/capability.rs):

```toml
[[review.capability_bindings]]
symbol   = "Net.fetch"
category = "network.client"   # domain.action, carries a default risk
provider = "reqwest"
```

## Run it

```sh
examples/capability-review-demo/run.sh
```

(It builds `rss` if needed, then runs the four steps below.)

## What it shows

### Phase 1 — `rss pkg review`: powers ranked by risk

```
$ rss pkg review before
capabilities (by risk):
  [medium] database.read via Db.query (provider sqlite)

$ rss pkg review after
capabilities (by risk):
  [high] network.client via Net.fetch (provider reqwest)
  [medium] database.read via Db.query (provider sqlite)
```

### Phase 3 — `rss pkg diff`: what powers did this change introduce?

```
$ rss pkg diff before after
package diff demo-data risk high
reasons:
  - new high-risk capability `network.client` via Net.fetch
capability changes:
  + [high] network.client via Net.fetch
```

`--json` emits the same as structured data for a GitHub Action or an agent:

```json
"risk": "high",
"capability_changes": [
  { "change": "added", "category": "network.client",
    "binding_symbol": "Net.fetch", "risk": "high" }
]
```

A reviewer sees *"this change adds a high-risk outbound-network power"* without
reading the code.

### Phase 2 — `rss pkg lock`: provider pinning (reproducible review)

Swap only the provider (`reqwest` → `evil-corp`), with the code unchanged:

```
reqwest  : review_hash = sha256:24f227ce...
evil-corp: review_hash = sha256:3fd4cacc...
=> review_hash CHANGED — the lock catches the provider swap.
```

Capabilities (including their provider) are part of the review hash, so a
supply-chain provider swap can't slip through unnoticed.

### Phase 0 — the foundation

- `read || { ... }` (an effect-annotated inline closure) now parses.
- A capability whose `category` isn't in the canonical taxonomy is flagged and
  treated as high risk:

  ```
  [high] databse.raed via Db.query  -- capability binding uses unrecognized category `databse.raed`
  ```

### Gate — reconcile required vs granted (policy + SARIF)

The deployment grants the package's powers *except* the new outbound network the
PR introduced. `reir report-pr` reconciles them under a policy and fails closed:

```
$ reir report-pr --policy examples/rss-policy.toml --target prod --sarif
   error missing_capability - Required capability not granted by target: NetworkClient / reqwest
  gate exit code: 1  (non-zero = blocked)
```

The SARIF output uploads to GitHub code scanning for inline PR annotations.

### Render, audit, fail-closed, agent

```
$ rss pkg review --markdown after     # PR-facing review (capabilities table, native, diagnostics)
$ rss pkg review --json after         # native adapter risk is included in package review JSON
$ rss pkg metadata <broken package>   # exit 1, no REIR written — evidence withheld for invalid source
$ rss check --explain RS0015 --json   # machine-readable diagnostic + repair guidance for agents
```

## The pipeline

```
declare capabilities (rsspkg.toml)
  -> rss pkg review [--markdown]   risk-ranked powers (human / JSON / markdown)
  -> rss pkg diff                  added / removed / escalated powers
  -> rss pkg lock                  capability-aware review_hash (provider pinned)
  -> reir collect | report-pr      reconcile required vs granted under rss-policy.toml
       --policy --target               fail on missing / unknown / excess / unverified
       --sarif                         inline PR annotations via GitHub code scanning
  -> rss pkg review --json         native adapter risk facts (transitive = not_audited)
  fail closed: invalid source (rss pkg metadata / reir collect --strict) emits no evidence
```
