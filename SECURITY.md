# Security policy

RSScript accepts security reports through GitHub private vulnerability
reporting for this repository. If that channel is unavailable, open a minimal
issue requesting a private contact without including exploit details.

Only the current `main` branch is supported before the first tagged release.
Reports should identify the affected Artifact, Provider, VM, runner protocol,
or process-isolation boundary and include a minimal reproducer when possible.

The in-process VM, native JIT, generated code, and Provider implementations are
not security sandboxes. Bounded execution limits reduce resource risk but do
not isolate untrusted code. External or machine-generated scripts require a
separate hardened process/container boundary as described in
[`docs/threat-model.md`](docs/threat-model.md).
