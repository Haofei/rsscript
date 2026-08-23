# Canonical native-JIT baselines

Ad-hoc benchmark snapshots belong in CI artifacts, not source control. A file is
accepted here only when produced on controlled hardware and named
`canonical-<os>-<arch>.json`.

No canonical timing baseline is checked in yet. Until controlled runners are
provisioned, the release smoke enforces the scalar speedup and the weekly
scorecard publishes diagnostic evidence without treating workstation timings as
a product contract.
