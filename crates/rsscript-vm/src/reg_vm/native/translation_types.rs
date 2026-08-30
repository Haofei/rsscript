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

/// The three native entry shapes share one lowering/validation/publication
/// pipeline. Their execution metadata remains explicit, but code generation no
/// longer accepts an unclassified tuple or a raw `JitFunction`.
#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum NativeRegionEntry {
    Function,
    Osr { header: u32 },
    Continuation { header: u32 },
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) enum NativeRegionMetadata {
    Whole {
        return_ty: NativeTy,
        param_tys: Vec<NativeTy>,
        precise_resume_safe: bool,
    },
    Osr {
        param_tys: Vec<NativeTy>,
        derived_live_ins: Vec<OsrDerivedLiveIn>,
        scalar_fields: Vec<OsrScalarField>,
        reg_tys: Vec<NativeTy>,
        written_regs: Vec<bool>,
    },
    Continuation {
        slots: Box<[ContinuationSlot]>,
        live_in_count: usize,
        exits: std::collections::BTreeMap<usize, ContinuationExit>,
        typed_summary: TypedRegionSummary,
        virtual_summary: VirtualObjectSummary,
    },
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct Lowered;

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct Analyzed {
    pub(in crate::reg_vm) source_work: u64,
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct NativeRegion<State> {
    entry: NativeRegionEntry,
    jit_fn: vm_jit::JitFunction,
    metadata: NativeRegionMetadata,
    string_literals: Vec<Rc<String>>,
    state: State,
}

#[cfg(feature = "native-jit")]
impl NativeRegion<Lowered> {
    pub(in crate::reg_vm) fn whole(translation: NativeTranslation) -> Self {
        Self {
            entry: NativeRegionEntry::Function,
            jit_fn: translation.jit_fn,
            metadata: NativeRegionMetadata::Whole {
                return_ty: translation.return_ty,
                param_tys: translation.param_tys,
                precise_resume_safe: translation.precise_resume_safe,
            },
            string_literals: translation.string_literals,
            state: Lowered,
        }
    }

    pub(in crate::reg_vm) fn osr(header: u32, translation: OsrTranslation) -> Self {
        Self {
            entry: NativeRegionEntry::Osr { header },
            jit_fn: translation.jit_fn,
            metadata: NativeRegionMetadata::Osr {
                param_tys: translation.param_tys,
                derived_live_ins: translation.derived_live_ins,
                scalar_fields: translation.scalar_fields,
                reg_tys: translation.reg_tys,
                written_regs: translation.written_regs,
            },
            string_literals: translation.string_literals,
            state: Lowered,
        }
    }

    pub(in crate::reg_vm) fn continuation(
        header: u32,
        translation: ContinuationTranslation,
    ) -> Self {
        Self {
            entry: NativeRegionEntry::Continuation { header },
            jit_fn: translation.jit_fn,
            metadata: NativeRegionMetadata::Continuation {
                slots: translation.slots,
                live_in_count: translation.live_in_count,
                exits: translation.exits,
                typed_summary: translation.typed_summary,
                virtual_summary: translation.virtual_summary,
            },
            string_literals: Vec::new(),
            state: Lowered,
        }
    }

    /// Freeze shape-independent analysis before the sealed JIT validator runs.
    pub(in crate::reg_vm) fn analyze(self) -> Option<NativeRegion<Analyzed>> {
        if self.jit_fn.code.len() != self.jit_fn.instruction_origins.len() {
            return None;
        }
        let source_work = self
            .jit_fn
            .instruction_origins
            .iter()
            .try_fold(0_u64, |total, origin| {
                total.checked_add(u64::from(origin.source_cost))
            })?;
        Some(NativeRegion {
            entry: self.entry,
            jit_fn: self.jit_fn,
            metadata: self.metadata,
            string_literals: self.string_literals,
            state: Analyzed { source_work },
        })
    }
}

#[cfg(feature = "native-jit")]
impl NativeRegion<Analyzed> {
    pub(in crate::reg_vm) fn jit_fn(&self) -> &vm_jit::JitFunction {
        &self.jit_fn
    }

    pub(in crate::reg_vm) fn metadata(&self) -> &NativeRegionMetadata {
        &self.metadata
    }

    pub(in crate::reg_vm) fn string_literals(&self) -> &[Rc<String>] {
        &self.string_literals
    }

    pub(in crate::reg_vm) fn source_work(&self) -> u64 {
        self.state.source_work
    }

    pub(in crate::reg_vm) fn into_parts(
        self,
    ) -> (vm_jit::JitFunction, NativeRegionMetadata, Vec<Rc<String>>) {
        (self.jit_fn, self.metadata, self.string_literals)
    }

    pub(in crate::reg_vm) fn validate<'a>(
        &'a self,
        module: &vm_jit::NativeModule,
    ) -> Result<ValidatedNativeRegion<'a>, vm_jit::JitError> {
        let proof = match self.entry {
            NativeRegionEntry::Function => module.validate_region(&self.jit_fn)?,
            NativeRegionEntry::Osr { .. } | NativeRegionEntry::Continuation { .. } => {
                module.validate_osr_region(&self.jit_fn)?
            }
        };
        Ok(ValidatedNativeRegion {
            region: self,
            proof,
        })
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct ValidatedNativeRegion<'a> {
    region: &'a NativeRegion<Analyzed>,
    proof: vm_jit::ValidatedJitFunction<'a>,
}

#[cfg(feature = "native-jit")]
impl ValidatedNativeRegion<'_> {
    pub(in crate::reg_vm) fn publish(
        self,
        module: &mut vm_jit::NativeModule,
        controls: vm_jit::RegionCompileControls,
    ) -> Result<PublishedNativeRegion, vm_jit::JitError> {
        let id = match self.region.entry {
            NativeRegionEntry::Function => {
                module.compile_validated_with_controls(&self.proof, controls)?
            }
            NativeRegionEntry::Osr { header } | NativeRegionEntry::Continuation { header } => {
                module.compile_validated_osr_with_controls(&self.proof, header, controls)?
            }
        };
        Ok(PublishedNativeRegion {
            id,
            entry: self.region.entry,
        })
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct PublishedNativeRegion {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) entry: NativeRegionEntry,
}
