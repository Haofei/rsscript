# Experiment fixture boundary

This directory owns non-Cargo inputs for frozen Research and Integration work.
They are not Core product assets, workspace members, release inputs, or SDK
dependencies.

| Fixture | Owner | Permitted use |
| --- | --- | --- |
| `selfhost/` | Research self-host parity workflow | Regression/parity evidence only; no feature expansion. |
| `native-abi-fixture/` | Native-ABI experiment and security-sensitive checks | Contract fixture only; never a default Provider or Core dependency. |

The repository-root `selfhost` and `packages/native-abi-fixture` paths are
compatibility symlinks so existing feature-gated test harnesses can continue to
locate immutable fixtures.  Do not add content through those aliases.  New or
changed fixture material must be reviewed as experiment maintenance and must not
be added to the root Cargo workspace or its default members.
