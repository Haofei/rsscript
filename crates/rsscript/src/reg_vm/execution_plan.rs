use super::VmLimits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StdoutMode {
    Captured,
    Streaming,
}

#[derive(Debug, Clone)]
pub(super) struct ExecutionPlan {
    pub(super) limits: VmLimits,
    pub(super) stdout: StdoutMode,
    pub(super) tier: TierPlan,
}

#[derive(Debug, Clone)]
pub(super) enum TierPlan {
    Interpreter,
    Tier0 {
        force_all: bool,
    },
    #[cfg(feature = "native-jit")]
    Native(NativeExecutionPlan),
}

impl ExecutionPlan {
    pub(super) fn interpreter(limits: VmLimits) -> Self {
        Self {
            limits,
            stdout: StdoutMode::Captured,
            tier: TierPlan::Interpreter,
        }
    }

    pub(super) fn streaming(limits: VmLimits) -> Self {
        Self {
            limits,
            stdout: StdoutMode::Streaming,
            tier: TierPlan::Interpreter,
        }
    }

    pub(super) fn tier0(limits: VmLimits, force_all: bool) -> Self {
        Self {
            limits,
            stdout: StdoutMode::Captured,
            tier: TierPlan::Tier0 { force_all },
        }
    }

    #[cfg(feature = "native-jit")]
    pub(super) fn native(limits: VmLimits, native: NativeExecutionPlan) -> Self {
        Self {
            limits,
            stdout: StdoutMode::Captured,
            tier: TierPlan::Native(native),
        }
    }
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(super) struct NativeExecutionPlan {
    pub(super) tier_up_threshold: u32,
    pub(super) force_bail: bool,
    pub(super) collect_stats: bool,
    pub(super) baseline: bool,
    pub(super) precise_deopt: bool,
    pub(super) osr_enabled: bool,
    pub(super) report: bool,
    pub(super) forced_safepoint: Option<u32>,
    pub(super) force_all_safepoints: bool,
    pub(super) admission: NativeAdmissionPolicy,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeAdmissionPolicy {
    pub(super) max_code_bytes: u64,
    pub(super) max_compile_millis: u64,
    pub(super) optimize_work_threshold: u64,
}

#[cfg(feature = "native-jit")]
impl NativeExecutionPlan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_environment(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
        report_override: bool,
        forced_safepoint: Option<u32>,
        force_all_safepoints_override: bool,
    ) -> Self {
        let baseline = std::env::var_os("RSS_JIT_BASELINE").is_some();
        let osr_enabled = osr_override || std::env::var_os("RSS_JIT_OSR").is_some();
        let precise_deopt = precise_deopt_override
            || osr_enabled
            || std::env::var_os("RSS_JIT_PRECISE_DEOPT").is_some();
        let report = report_override || std::env::var_os("RSS_JIT_REPORT").is_some();
        let force_all_safepoints = forced_safepoint.is_none()
            && (force_all_safepoints_override || super::jit_native_deopt_every_from_env());
        Self {
            tier_up_threshold,
            force_bail,
            collect_stats,
            baseline,
            precise_deopt,
            osr_enabled,
            report,
            forced_safepoint,
            force_all_safepoints,
            admission: NativeAdmissionPolicy::from_environment(tier_up_threshold),
        }
    }
}

#[cfg(feature = "native-jit")]
impl NativeAdmissionPolicy {
    pub(super) fn from_environment(tier_up_threshold: u32) -> Self {
        Self {
            max_code_bytes: env_u64("RSS_JIT_MAX_CODE_BYTES", 16 * 1024 * 1024),
            max_compile_millis: env_u64("RSS_JIT_MAX_COMPILE_MS", 2_000),
            optimize_work_threshold: env_u64("RSS_JIT_OPT_THRESHOLD", 50_000)
                .max(u64::from(tier_up_threshold) + 1),
        }
    }
}

#[cfg(feature = "native-jit")]
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_plans_keep_tier_and_output_policy_explicit() {
        assert!(matches!(
            ExecutionPlan::interpreter(VmLimits::default()).tier,
            TierPlan::Interpreter
        ));
        assert!(matches!(
            ExecutionPlan::tier0(VmLimits::default(), true).tier,
            TierPlan::Tier0 { force_all: true }
        ));
        assert_eq!(
            ExecutionPlan::streaming(VmLimits::safe_default()).stdout,
            StdoutMode::Streaming
        );
    }
}
