# ADR 0210: Require observable MIR migration parity

- Status: Accepted
- Date: 2026-08-14

Value equality alone is insufficient evidence for replacing a lowering path.
The direct checked-HIR-to-MIR migration corpus must also preserve observable
program output and runtime accounting.

Each pure `DualPath` case now compares legacy and MIR-produced bytecode values,
stdout, stderr, and `ExecutionUsage`. Provider traces, cancellation, deadlines,
and resource cleanup keep their dedicated differential fixtures because their
observable contracts require structured setup beyond the pure corpus. A
capability cannot be promoted solely because the returned value happens to
match.
