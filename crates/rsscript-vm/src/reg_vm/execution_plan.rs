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
    pub(super) allow_recursive_calls: bool,
    pub(super) cost_model: NativeCostModel,
    pub(super) osr_work_threshold: u32,
    pub(super) admission: NativeAdmissionPolicy,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeAdmissionPolicy {
    pub(super) max_code_bytes: u64,
    pub(super) max_compile_millis: u64,
    pub(super) optimize_work_threshold: u64,
}

/// Host-owned configuration for the optional trusted in-process native tier.
///
/// Artifacts and source code cannot select or modify this policy. Defaults are
/// deterministic and bounded; test-only stress modes remain separate entry
/// points rather than fields in the production contract.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeJitOptions {
    pub tier_up_threshold: u32,
    pub enable_osr: bool,
    pub collect_telemetry: bool,
    pub max_code_bytes: u64,
    pub max_compile_millis: u64,
    pub optimize_work_threshold: u64,
    pub cost_model: NativeCostModel,
    pub osr_work_threshold: u32,
    /// Enables non-tail native recursion on the host C stack.
    ///
    /// This remains disabled by default because the current Cranelift backend
    /// cannot prove the live host stack bound. Tail recursion continues to lower
    /// to loops, and disabled recursive calls fall back to the interpreter.
    pub allow_recursive_calls: bool,
}

/// Host-selected profitability behavior for eligible native regions.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCostModel {
    Off,
    Report,
    Enforce,
}

#[cfg(feature = "native-jit")]
impl NativeCostModel {
    pub(super) fn active(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[cfg(feature = "native-jit")]
impl Default for NativeJitOptions {
    fn default() -> Self {
        Self {
            tier_up_threshold: 0,
            enable_osr: true,
            collect_telemetry: true,
            max_code_bytes: 16 * 1024 * 1024,
            max_compile_millis: 2_000,
            optimize_work_threshold: 50_000,
            cost_model: NativeCostModel::Enforce,
            osr_work_threshold: 1_000,
            allow_recursive_calls: false,
        }
    }
}

#[cfg(feature = "native-jit")]
impl NativeExecutionPlan {
    pub(super) fn from_options(options: NativeJitOptions) -> Self {
        Self {
            tier_up_threshold: options.tier_up_threshold,
            force_bail: false,
            collect_stats: options.collect_telemetry,
            baseline: false,
            precise_deopt: true,
            osr_enabled: options.enable_osr,
            report: false,
            forced_safepoint: None,
            force_all_safepoints: false,
            allow_recursive_calls: options.allow_recursive_calls,
            cost_model: options.cost_model,
            osr_work_threshold: options.osr_work_threshold,
            admission: NativeAdmissionPolicy {
                max_code_bytes: options.max_code_bytes,
                max_compile_millis: options.max_compile_millis,
                optimize_work_threshold: options
                    .optimize_work_threshold
                    .max(u64::from(options.tier_up_threshold) + 1),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_diagnostics(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
        report_override: bool,
        forced_safepoint: Option<u32>,
        force_all_safepoints_override: bool,
    ) -> Self {
        let baseline = false;
        let osr_enabled = osr_override;
        let precise_deopt = precise_deopt_override || osr_enabled;
        let report = report_override;
        let force_all_safepoints = forced_safepoint.is_none() && force_all_safepoints_override;
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
            // Legacy diagnostic entry points are used only by in-crate tests and
            // developer tooling. They no longer opt production execution into
            // host-stack recursion implicitly.
            allow_recursive_calls: false,
            cost_model: NativeCostModel::Off,
            osr_work_threshold: 1_000,
            admission: NativeAdmissionPolicy::bounded(tier_up_threshold),
        }
    }
}

#[cfg(feature = "native-jit")]
impl NativeAdmissionPolicy {
    pub(super) fn bounded(tier_up_threshold: u32) -> Self {
        Self {
            max_code_bytes: 16 * 1024 * 1024,
            max_compile_millis: 2_000,
            optimize_work_threshold: 50_000_u64.max(u64::from(tier_up_threshold) + 1),
        }
    }
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
            ExecutionPlan::streaming(VmLimits::default()).stdout,
            StdoutMode::Streaming
        );
        #[cfg(feature = "native-jit")]
        assert!(
            !NativeJitOptions::default().allow_recursive_calls,
            "host-stack recursion must require an explicit trusted-host opt-in"
        );
    }
}
