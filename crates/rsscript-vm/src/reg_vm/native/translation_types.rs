//! Named state transferred between native lowering, admission, and codegen.

use super::*;

#[cfg(feature = "native-jit")]
#[derive(Clone)]
pub(in crate::reg_vm) struct NativeCompiledCallee {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) ret_ty: NativeTy,
    pub(in crate::reg_vm) param_tys: Vec<NativeTy>,
}

/// Typed handoff from whole-function lowering to admission/codegen.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct NativeTranslation {
    pub(in crate::reg_vm) jit_fn: vm_jit::JitFunction,
    pub(in crate::reg_vm) return_ty: NativeTy,
    pub(in crate::reg_vm) param_tys: Vec<NativeTy>,
    pub(in crate::reg_vm) string_literals: Vec<Rc<String>>,
    pub(in crate::reg_vm) precise_resume_safe: bool,
}

/// Typed OSR/region result replacing the historical seven-element tuple.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct OsrTranslation {
    pub(in crate::reg_vm) jit_fn: vm_jit::JitFunction,
    pub(in crate::reg_vm) param_tys: Vec<NativeTy>,
    pub(in crate::reg_vm) derived_live_ins: Vec<OsrDerivedLiveIn>,
    pub(in crate::reg_vm) scalar_fields: Vec<OsrScalarField>,
    pub(in crate::reg_vm) reg_tys: Vec<NativeTy>,
    pub(in crate::reg_vm) written_regs: Vec<bool>,
    pub(in crate::reg_vm) string_literals: Vec<Rc<String>>,
}

/// Compact continuation lowering result and its normal-exit materialization map.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct ContinuationTranslation {
    pub(in crate::reg_vm) jit_fn: vm_jit::JitFunction,
    pub(in crate::reg_vm) slots: Box<[ContinuationSlot]>,
    pub(in crate::reg_vm) live_in_count: usize,
    pub(in crate::reg_vm) exits: std::collections::BTreeMap<usize, ContinuationExit>,
    pub(in crate::reg_vm) typed_summary: TypedRegionSummary,
    pub(in crate::reg_vm) virtual_summary: VirtualObjectSummary,
}
