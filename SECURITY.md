# Security policy

RSScript accepts security reports through
[GitHub private vulnerability reporting](https://github.com/Haofei/rsscript/security/advisories/new).
Do not publish exploit details in an issue or discussion. If GitHub does not
offer that private form, wait for the private channel to be restored rather
than disclosing the report through a public repository surface.

Only the current `main` branch is supported before the first tagged release.
Reports should identify the affected Artifact, Provider, VM, runner protocol,
or process-isolation boundary and include a minimal reproducer when possible.

The in-process VM, native JIT, generated code, and Provider implementations are
not security sandboxes. Bounded execution limits reduce resource risk but do
not isolate untrusted code. External or machine-generated scripts require a
separate hardened process/container boundary as described in
[`docs/threat-model.md`](docs/threat-model.md).
