# Packages and external providers

A package separates source implementations (`.rss`), interface declarations
(`.rssi`), and provider bindings. Interface declarations are ordinary bodyless
functions. Their implementation technology is not part of RSScript syntax.

Host services are explicit package dependencies. Binding files use
`rsscript.bindings.v1`, map a symbol to a provider and entry point, and may include
optional review metadata. Missing or duplicate bindings and ABI mismatches are
link errors.

The compiler emits `rsscript.package_analysis.v1`, containing diagnostics,
exports, semantic summaries, and external symbols. It does not contain permission
grants. Package build-selection features remain package metadata and are not
language features.

`rss pkg analysis [package-directory]` prints the neutral analysis artifact as
JSON. `rss pkg review` derives optional review output separately.

Optional package review uses the distinct `rsscript.package_review.v1` schema.
Risk, provider selection, native implementation details, and deployment evidence
must not appear in package analysis. The authoritative schemas are checked in
under `schemas/`.

Package graph snapshots, hashes, bounded file reads, reduced build environments,
and atomic artifact writes remain integrity/correctness controls. They do not
grant authority.
