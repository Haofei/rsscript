# Experiment fixture boundary

This directory owns non-Cargo inputs for frozen Research and Integration work.
They are not Core product assets, workspace members, release inputs, or SDK
dependencies.

| Fixture | Owner | Permitted use |
| --- | --- | --- |
| `selfhost/` | Research self-host parity workflow | Regression/parity evidence only; no feature expansion. |
| `native-abi-fixture/` | Native-ABI experiment and security-sensitive checks | Contract fixture only; never a default Provider or Core dependency. |

Feature-gated test harnesses address these physical paths directly. The retired
repository-root `selfhost` and `packages/native-abi-fixture` compatibility
aliases must not return. New or changed fixture material is experiment
maintenance and must not enter the root Cargo workspace or its default members.
