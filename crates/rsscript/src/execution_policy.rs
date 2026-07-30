use std::fmt;
use std::str::FromStr;

/// Product support level for an execution capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportLevel {
    Core,
    Experimental,
    UnsupportedForUntrusted,
}

/// Host-facing capabilities whose availability depends on deployment trust.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionCapability {
    StaticLowering,
    BoundedRustAot,
    BoundedReferenceVm,
    UnlimitedVm,
    InProcessNative,
    NativeJit,
    DynamicGpuShader,
    ArbitraryProcess,
    ArbitraryNetwork,
}

impl ExecutionCapability {
    pub const fn support_level(self) -> SupportLevel {
        match self {
            Self::StaticLowering | Self::BoundedRustAot | Self::BoundedReferenceVm => {
                SupportLevel::Core
            }
            Self::NativeJit | Self::DynamicGpuShader => SupportLevel::Experimental,
            Self::UnlimitedVm
            | Self::InProcessNative
            | Self::ArbitraryProcess
            | Self::ArbitraryNetwork => SupportLevel::UnsupportedForUntrusted,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::StaticLowering => "static lowering",
            Self::BoundedRustAot => "bounded Rust AOT execution",
            Self::BoundedReferenceVm => "bounded reference VM execution",
            Self::UnlimitedVm => "unlimited VM execution",
            Self::InProcessNative => "in-process native plugins",
            Self::NativeJit => "native JIT execution",
            Self::DynamicGpuShader => "dynamic GPU shaders",
            Self::ArbitraryProcess => "arbitrary child processes",
            Self::ArbitraryNetwork => "arbitrary network access",
        }
    }
}

/// Deployment trust profile enforced at execution entry points.
///
/// `UntrustedIsolated` deliberately denies execution until RSScript has a
/// killable worker sandbox. It remains a profile so callers can fail closed
/// instead of silently falling back to trusted in-process execution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeploymentProfile {
    #[default]
    LocalTrusted,
    TrustedCi,
    UntrustedIsolated,
}

impl DeploymentProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalTrusted => "local-trusted",
            Self::TrustedCi => "trusted-ci",
            Self::UntrustedIsolated => "untrusted-isolated",
        }
    }

    pub fn authorize(self, capability: ExecutionCapability) -> Result<(), ExecutionPolicyError> {
        let allowed = match self {
            Self::LocalTrusted => true,
            // Runtime host capabilities are not yet carried through every VM
            // intrinsic and generated AOT program. Until they are, allowing a
            // "bounded" backend here would create a policy bypass for process,
            // network, and GPU calls made by the program.
            Self::TrustedCi => matches!(capability, ExecutionCapability::StaticLowering),
            Self::UntrustedIsolated => matches!(capability, ExecutionCapability::StaticLowering),
        };
        if allowed {
            Ok(())
        } else {
            Err(ExecutionPolicyError {
                profile: self,
                capability,
            })
        }
    }
}

impl fmt::Display for DeploymentProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DeploymentProfile {
    type Err = ParseDeploymentProfileError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "local-trusted" => Ok(Self::LocalTrusted),
            "trusted-ci" => Ok(Self::TrustedCi),
            "untrusted-isolated" => Ok(Self::UntrustedIsolated),
            _ => Err(ParseDeploymentProfileError(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseDeploymentProfileError(String);

impl fmt::Display for ParseDeploymentProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown deployment profile `{}`; expected local-trusted, trusted-ci, or untrusted-isolated",
            self.0
        )
    }
}

impl std::error::Error for ParseDeploymentProfileError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionPolicyError {
    profile: DeploymentProfile,
    capability: ExecutionCapability,
}

impl ExecutionPolicyError {
    pub const fn profile(&self) -> DeploymentProfile {
        self.profile
    }

    pub const fn capability(&self) -> ExecutionCapability {
        self.capability
    }
}

impl fmt::Display for ExecutionPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.profile == DeploymentProfile::UntrustedIsolated {
            return write!(
                formatter,
                "deployment profile `{}` denies {} because an isolated worker sandbox is not implemented",
                self.profile,
                self.capability.name()
            );
        }
        if self.profile == DeploymentProfile::TrustedCi {
            return write!(
                formatter,
                "deployment profile `{}` denies {} because end-to-end runtime capability enforcement is not implemented",
                self.profile,
                self.capability.name()
            );
        }
        write!(
            formatter,
            "deployment profile `{}` denies {}",
            self.profile,
            self.capability.name()
        )
    }
}

impl std::error::Error for ExecutionPolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_ci_is_static_only_until_runtime_policy_is_end_to_end() {
        let profile = DeploymentProfile::TrustedCi;
        assert!(
            profile
                .authorize(ExecutionCapability::StaticLowering)
                .is_ok()
        );
        assert!(
            profile
                .authorize(ExecutionCapability::BoundedRustAot)
                .is_err()
        );
        assert!(
            profile
                .authorize(ExecutionCapability::BoundedReferenceVm)
                .is_err()
        );
        assert!(profile.authorize(ExecutionCapability::NativeJit).is_err());
        assert!(
            profile
                .authorize(ExecutionCapability::InProcessNative)
                .is_err()
        );
        assert!(profile.authorize(ExecutionCapability::UnlimitedVm).is_err());
    }

    #[test]
    fn untrusted_profile_fails_closed_until_worker_is_available() {
        let error = DeploymentProfile::UntrustedIsolated
            .authorize(ExecutionCapability::BoundedReferenceVm)
            .expect_err("in-process execution must remain unavailable");
        assert!(
            error
                .to_string()
                .contains("isolated worker sandbox is not implemented")
        );
    }

    #[test]
    fn profile_parser_rejects_ambiguous_names() {
        assert_eq!("trusted-ci".parse(), Ok(DeploymentProfile::TrustedCi));
        assert!("production".parse::<DeploymentProfile>().is_err());
    }
}
