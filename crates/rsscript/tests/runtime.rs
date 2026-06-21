//! VM, runtime, and JIT integration tests.
//!
//! This is the single target for fast executable behavior: VM, runtime
//! intrinsics, JIT, hostile-input hardening, and small AOT smoke checks.
#![allow(clippy::duplicate_mod)]

#[path = "hostile.rs"]
mod hostile;
#[path = "jit_acceptance.rs"]
mod jit_acceptance;
#[path = "jit_arithmetic_edges.rs"]
mod jit_arithmetic_edges;
#[path = "jit_plan.rs"]
mod jit_plan;
#[path = "properties.rs"]
mod properties;
#[path = "smoke_run.rs"]
mod smoke_run;
#[path = "vm.rs"]
mod vm;
#[path = "vm_eval.rs"]
mod vm_eval;
#[path = "vm_eval_parity.rs"]
mod vm_eval_parity;
