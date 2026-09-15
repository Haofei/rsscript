#![forbid(unsafe_code)]

pub mod diagnostic {
    pub use rsscript_diagnostics::*;
}

/// Deterministic pure-library algorithms. A one-way module boundary: nothing in
/// it may name a VM type. See the module header and the VM runtime dependency
/// inventory.
mod corelib;
mod eval_types;
mod fnv;
mod reg_vm;
pub(crate) use crate::corelib::structured_data::serde_json;
mod text_util {
    pub(crate) use rsscript_core_types::text::*;
}
mod vm_value;

pub use eval_types::*;
pub use reg_vm::*;
