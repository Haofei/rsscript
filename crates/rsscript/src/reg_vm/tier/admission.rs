use super::*;

pub(super) struct NativeCompileAdmission {
    started: std::time::Instant,
    regions: u64,
}

pub(super) fn begin_native_compile(
    native: &mut NativeState,
    regions: usize,
) -> Option<NativeCompileAdmission> {
    let exhausted = native.admission.code_exhausted
        || native.admission.admitted_code_bytes >= native.admission.max_code_bytes
        || native.admission.compile_nanos >= native.admission.max_compile_nanos;
    if exhausted {
        if native.collect_stats {
            native.stats.admission_rejected = native
                .stats
                .admission_rejected
                .saturating_add(regions as u64);
        }
        return None;
    }
    Some(NativeCompileAdmission {
        started: std::time::Instant::now(),
        regions: regions as u64,
    })
}

pub(super) fn finish_native_compile_failure(
    native: &mut NativeState,
    admission: NativeCompileAdmission,
) {
    let elapsed = admission.started.elapsed().as_nanos();
    native.admission.compile_nanos = native.admission.compile_nanos.saturating_add(elapsed);
    if native.collect_stats {
        native.stats.compile_nanos = native.stats.compile_nanos.saturating_add(elapsed);
    }
}

/// Admit successfully emitted ids. Baseline and optimized modules reserve fixed
/// Cranelift arenas from one shared hard budget; codegen cannot grow beyond those
/// mappings. Compile-time exhaustion stops subsequent attempts; it does not
/// discard the current successfully emitted function and leave unreachable code
/// resident.
pub(super) fn finish_native_compile(
    native: &mut NativeState,
    admission: NativeCompileAdmission,
    ids: &[vm_jit::CompiledId],
    tier: NativeCodeTier,
) -> bool {
    let elapsed = admission.started.elapsed().as_nanos();
    native.admission.compile_nanos = native.admission.compile_nanos.saturating_add(elapsed);
    if native.collect_stats {
        native.stats.compile_nanos = native.stats.compile_nanos.saturating_add(elapsed);
    }
    let module = match tier {
        NativeCodeTier::Baseline => &native.baseline_module,
        NativeCodeTier::Optimized => native
            .optimized_module
            .as_ref()
            .expect("optimized admission requires optimized module"),
    };
    let code_bytes = ids.iter().fold(0u64, |total, &id| {
        total.saturating_add(module.code_size_bytes(id).unwrap_or(0))
    });
    let admitted_bytes = native
        .admission
        .admitted_code_bytes
        .checked_add(code_bytes)
        .filter(|&total| total <= native.admission.max_code_bytes);
    let within_time = native.admission.compile_nanos <= native.admission.max_compile_nanos;
    if let Some(total) = admitted_bytes.filter(|_| within_time) {
        native.admission.admitted_code_bytes = total;
        if native.collect_stats {
            native.stats.admission_admitted = native
                .stats
                .admission_admitted
                .saturating_add(admission.regions);
            native.stats.admission_admitted_bytes = native
                .stats
                .admission_admitted_bytes
                .saturating_add(code_bytes);
        }
        true
    } else {
        if admitted_bytes.is_none() {
            native.admission.code_exhausted = true;
        }
        if native.collect_stats {
            native.stats.admission_rejected = native
                .stats
                .admission_rejected
                .saturating_add(admission.regions);
            native.stats.admission_rejected_bytes = native
                .stats
                .admission_rejected_bytes
                .saturating_add(code_bytes);
        }
        false
    }
}
