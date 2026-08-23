use super::*;

pub(super) struct NativeCompileAdmission {
    started: std::time::Instant,
    regions: u64,
}

pub(super) fn begin_native_compile(
    native: &mut NativeState,
    regions: usize,
    tier: NativeCodeTier,
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
    let initialized = match tier {
        NativeCodeTier::Baseline => native.baseline_module.ensure_initialized(),
        NativeCodeTier::Optimized => native
            .optimized_module
            .as_mut()
            .is_some_and(LazyNativeModule::ensure_initialized),
    };
    if !initialized {
        if native.collect_stats {
            native.stats.compile_failed = native.stats.compile_failed.saturating_add(1);
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

/// Publish successfully emitted ids. Baseline and optimized modules reserve fixed
/// Cranelift arenas from one shared hard budget, so codegen cannot grow beyond the
/// configured executable-memory mapping. Once Cranelift has finalized a function
/// it is resident for the module lifetime and cannot be individually reclaimed;
/// therefore the current function is always published. Crossing the soft code or
/// compile-time admission budget only closes admission for subsequent attempts.
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
    let (total, exhausted) = admission_after_publish(
        native.admission.admitted_code_bytes,
        code_bytes,
        native.admission.max_code_bytes,
        native.admission.compile_nanos,
        native.admission.max_compile_nanos,
    );
    native.admission.admitted_code_bytes = total;
    if exhausted {
        native.admission.code_exhausted = true;
    }
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
}

fn admission_after_publish(
    published_bytes: u64,
    current_bytes: u64,
    max_code_bytes: u64,
    compile_nanos: u128,
    max_compile_nanos: u128,
) -> (u64, bool) {
    let published_bytes = published_bytes.saturating_add(current_bytes);
    let exhausted = published_bytes >= max_code_bytes || compile_nanos > max_compile_nanos;
    (published_bytes, exhausted)
}

#[cfg(test)]
mod tests {
    use super::admission_after_publish;

    #[test]
    fn finalized_code_is_published_before_future_admission_closes() {
        assert_eq!(admission_after_publish(80, 30, 100, 5, 10), (110, true));
        assert_eq!(admission_after_publish(10, 20, 100, 11, 10), (30, true));
        assert_eq!(admission_after_publish(10, 20, 100, 10, 10), (30, false));
    }
}
