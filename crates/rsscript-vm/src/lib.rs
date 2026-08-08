#![forbid(unsafe_code)]

pub mod diagnostic {
    pub use rsscript_diagnostics::*;
}

mod eval_types;
mod fnv;
mod reg_vm;
mod text_util {
    pub(crate) use rsscript_text::*;
}
mod vm_value;

pub use eval_types::*;
pub use reg_vm::*;
