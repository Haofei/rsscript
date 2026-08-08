#![forbid(unsafe_code)]

pub mod diagnostic {
    pub use rsscript_diagnostics::*;
}

mod eval_types;
mod fnv;
mod reg_vm;
#[allow(dead_code)]
#[path = "../../rsscript/src/text_util.rs"]
mod text_util;
mod vm_value;

pub use eval_types::*;
pub use reg_vm::*;
