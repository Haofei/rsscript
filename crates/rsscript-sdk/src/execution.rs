//! Verified Artifact, linking, execution, and report implementation.

use super::*;

#[derive(Debug)]
pub struct BuiltArtifact {
    bundle: ArtifactBundle,
}

impl BuiltArtifact {
    pub(super) fn from_bytecode(
        artifact: BytecodeArtifact,
        analysis: AnalysisEnvelopeV1,
    ) -> Result<Self, CompileError> {
        let bytes = artifact
            .to_bytes()
            .map_err(|error| CompileError::from(EvalError::Runtime(error.to_string())))?;
        let bundle = ArtifactBundle::new(bytes, analysis).map_err(CompileError::from)?;
        Ok(Self { bundle })
    }

    pub fn bundle(&self) -> &ArtifactBundle {
        &self.bundle
    }

    pub fn into_bundle(self) -> ArtifactBundle {
        self.bundle
    }

    pub fn bundle_bytes(&self) -> Result<Vec<u8>, CompileError> {
        self.bundle.to_bytes().map_err(CompileError::from)
    }

    pub fn artifact_bytes(&self) -> &[u8] {
        self.bundle.artifact_bytes()
    }

    /// Versioned analysis evidence bound to this Artifact Bundle.
    ///
    /// Reviewed callers select a typed source/package projection from this
    /// envelope instead of treating analysis as arbitrary JSON.
    pub fn analysis_envelope(&self) -> &AnalysisEnvelopeV1 {
        self.bundle.analysis_envelope()
    }

    /// Typed source evidence for direct source/interface builds. Package
    /// compatibility builds may instead carry their distinct package-analysis
    /// schema and return `None` here.
    pub fn source_analysis(&self) -> Option<&SourceAnalysisV1> {
        self.bundle.source_analysis()
    }

    /// Typed package evidence for immutable package compatibility builds.
    /// Direct source/interface builds instead carry `source_analysis.v1`.
    pub fn package_analysis(&self) -> Option<&PackageAnalysisV1> {
        self.bundle.package_analysis()
    }

    pub fn snapshot_digest(&self) -> &str {
        &self.bundle.provenance().snapshot_digest
    }

    pub fn module_digest(&self) -> &str {
        &self.bundle.provenance().module_digest
    }

    pub fn external_imports(&self) -> &[InterfaceRequirementV1] {
        self.bundle.required_interfaces()
    }
}

pub(super) fn source_set_analysis(
    validated: &ValidatedProgram,
    sources: &[(&str, &str)],
    snapshot_digest: &str,
) -> AnalysisEnvelopeV1 {
    use rsscript_semantics::hir::{CallResolution, ParamEffect};

    let hir = validated.database().hir();
    let mut exports = Vec::new();
    for (name, _) in hir.function_bodies() {
        let Some(signature) = hir.resolve_function(None, name) else {
            continue;
        };
        if signature.is_builtin || signature.is_external {
            continue;
        }

        let mut retained_params = signature
            .retained_params
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        retained_params.sort();
        let mut semantic_facts = Vec::new();
        if signature.is_async {
            semantic_facts.push("async boundary".to_string());
        }
        if signature.returns_fresh {
            semantic_facts.push("returns fresh value".to_string());
        }
        for parameter in &signature.params {
            let effect = parameter.effect.unwrap_or(ParamEffect::Read).as_str();
            if effect != "read" {
                semantic_facts.push(format!("{effect} parameter `{}`", parameter.name));
            }
        }
        semantic_facts.extend(
            retained_params
                .iter()
                .map(|parameter| format!("retains({parameter})")),
        );
        semantic_facts.sort();
        semantic_facts.dedup();

        exports.push(ExportFactV1 {
            name: name.to_string(),
            kind: "function".to_string(),
            function_kind: Some(if signature.is_async { "async" } else { "sync" }.to_string()),
            parameters: signature
                .params
                .iter()
                .map(|parameter| FunctionParameterFactV1 {
                    name: parameter.name.clone(),
                    effect: parameter
                        .effect
                        .unwrap_or(ParamEffect::Read)
                        .as_str()
                        .to_string(),
                    ty: parameter.ty.to_string(),
                    retained: signature.retained_params.contains(&parameter.name),
                })
                .collect(),
            return_type: signature.return_ty.as_ref().map(ToString::to_string),
            retained_params,
            semantic_facts,
        });
    }

    let mut call_edges = Vec::new();
    let mut external_calls = Vec::new();
    for call in hir.call_sites() {
        let CallResolution::Resolved { signature, .. } = &call.resolution else {
            continue;
        };
        let callee = signature
            .namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", signature.name))
            .unwrap_or_else(|| signature.name.clone());
        call_edges.push(CallEdgeFactV1 {
            caller: call.function_name.clone(),
            callee: callee.clone(),
        });
        if signature.is_external {
            external_calls.push(ExternalCallFactV1 {
                function: call.function_name.clone(),
                symbol: callee.clone(),
                call_chain: vec![call.function_name.clone(), callee],
            });
        }
    }
    AnalysisEnvelopeV1::source(
        SourceAnalysisV1::new(
            rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION,
            snapshot_digest,
            sources.iter().map(|(path, _)| *path),
        )
        .with_function_contracts(exports)
        .with_call_facts(call_edges, external_calls),
    )
}

pub(super) fn in_memory_snapshot_digest(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> String {
    // Direct SDK compilation has no filesystem workspace, but it still needs
    // the same immutable identity guarantee as package compilation. Domain
    // separation and role/path/byte lengths prevent ambiguous concatenations.
    let mut entries = sources
        .iter()
        .map(|(path, text)| ("source", *path, *text))
        .chain(
            interfaces
                .iter()
                .map(|(path, text)| ("interface", *path, *text)),
        )
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.in_memory_snapshot.v1\0");
    for (role, path, text) in entries {
        for value in [role.as_bytes(), path.as_bytes(), text.as_bytes()] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Default)]
pub struct ArtifactVerifier;

impl ArtifactVerifier {
    pub fn verify(&self, built: BuiltArtifact) -> Result<VerifiedArtifact, VerifyError> {
        self.verify_bundle(built.into_bundle())
    }

    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<VerifiedArtifact, VerifyError> {
        let bundle = ArtifactBundle::from_bytes(bytes).map_err(VerifyError::Bundle)?;
        self.verify_bundle(bundle)
    }

    pub fn verify_bundle(&self, bundle: ArtifactBundle) -> Result<VerifiedArtifact, VerifyError> {
        let verified_bytecode = BytecodeVerifier::default()
            .verify(bundle.artifact_bytes())
            .map_err(|error| VerifyError::Bytecode(EvalError::Runtime(error.to_string())))?;
        verified_artifact(bundle, verified_bytecode)
    }

    pub fn verify_bytes_with_operation(
        &self,
        bytes: &[u8],
        operation: &OperationContext,
    ) -> Result<VerifiedArtifact, VerifyError> {
        operation.check().map_err(VerifyError::Operation)?;
        let bundle = ArtifactBundle::from_bytes(bytes).map_err(VerifyError::Bundle)?;
        let verified_bytecode = BytecodeVerifier::default()
            .verify_with_context(
                bundle.artifact_bytes(),
                VerificationContext {
                    cancellation: operation.cancellation.as_ref(),
                    deadline: operation.deadline,
                },
            )
            .map_err(|error| VerifyError::Bytecode(EvalError::Runtime(error.to_string())))?;
        let artifact = verified_artifact(bundle, verified_bytecode)?;
        operation.check().map_err(VerifyError::Operation)?;
        Ok(artifact)
    }
}

/// The operation-aware and ordinary verifier entry points must produce the
/// exact same phase object. Keeping this conversion in one place prevents a
/// cancellation/deadline convenience API from accidentally skipping a bundle
/// provenance invariant.
fn verified_artifact(
    bundle: ArtifactBundle,
    verified_bytecode: VerifiedBytecode,
) -> Result<VerifiedArtifact, VerifyError> {
    let executable = RegVmExecutable::from_verified_bytecode(verified_bytecode)
        .map_err(VerifyError::Bytecode)?;
    if executable.bytecode_artifact().header.executable_hash != bundle.provenance().module_digest {
        return Err(VerifyError::DigestMismatch);
    }
    Ok(VerifiedArtifact { bundle, executable })
}

#[derive(Debug)]
pub struct VerifiedArtifact {
    bundle: ArtifactBundle,
    executable: RegVmExecutable,
}

impl VerifiedArtifact {
    pub fn bundle(&self) -> &ArtifactBundle {
        &self.bundle
    }

    pub fn module_digest(&self) -> &str {
        &self.bundle.provenance().module_digest
    }

    pub fn external_imports(&self) -> &[ExternalImport] {
        &self.executable.bytecode_artifact().imports
    }

    /// Verified bytecode metadata for inspection tools. Execution remains
    /// available only through the linked runtime stage.
    pub fn bytecode_artifact(&self) -> &BytecodeArtifact {
        self.executable.bytecode_artifact()
    }

    /// Apply the host-owned origin/admission decision before provider linking.
    ///
    /// Verification proves artifact structure and integrity. Admission is a
    /// separate host decision, for example a detached-signature, provenance,
    /// or runner-profile check; it does not create a language policy system.
    pub fn admit<P: ArtifactAdmissionPolicy>(
        self,
        policy: &P,
    ) -> Result<AdmittedArtifact, AdmissionError> {
        let admission = policy.admit(&self)?;
        Ok(AdmittedArtifact {
            artifact: self,
            admission,
        })
    }

    /// Explicitly mark bytes as trusted by this embedding host.
    ///
    /// Hosts handling external inputs should implement
    /// [`ArtifactAdmissionPolicy`] instead; an isolated runner may model its
    /// fixed profile as that policy.
    pub fn admit_trusted_input(self) -> AdmittedArtifact {
        AdmittedArtifact {
            artifact: self,
            admission: ArtifactAdmission::trusted_input(),
        }
    }
}

/// Evidence returned by one host-owned artifact admission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactAdmission {
    policy_id: String,
    evidence_digest: Option<String>,
}

impl ArtifactAdmission {
    /// Construct non-secret admission evidence for an accepted artifact.
    pub fn new(
        policy_id: impl Into<String>,
        evidence_digest: Option<impl Into<String>>,
    ) -> Result<Self, AdmissionError> {
        let policy_id = policy_id.into();
        if policy_id.trim().is_empty() {
            return Err(AdmissionError::InvalidPolicyId);
        }
        Ok(Self {
            policy_id,
            evidence_digest: evidence_digest.map(Into::into),
        })
    }

    fn trusted_input() -> Self {
        Self {
            policy_id: "trusted_input.v1".to_string(),
            evidence_digest: None,
        }
    }

    pub fn policy_id(&self) -> &str {
        &self.policy_id
    }

    pub fn evidence_digest(&self) -> Option<&str> {
        self.evidence_digest.as_deref()
    }
}

/// Host extension point for artifact origin/provenance verification.
pub trait ArtifactAdmissionPolicy {
    fn admit(&self, artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError>;
}

/// Explicit policy for a host that already trusts its artifact input channel.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrustedInputAdmission;

impl ArtifactAdmissionPolicy for TrustedInputAdmission {
    fn admit(&self, _artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError> {
        Ok(ArtifactAdmission::trusted_input())
    }
}

/// Host plug-in for detached signatures, transparency logs, or enterprise
/// provenance attestations. The verifier receives only integrity-verified
/// bundle identity and provenance; it cannot change language validity or
/// Provider authority.
pub trait ArtifactOriginVerifier {
    fn verify_origin(
        &self,
        bundle_digest: &str,
        provenance: &BuildProvenanceV1,
    ) -> Result<String, String>;
}

/// Admission policy backed by an explicit origin verifier. The returned
/// evidence digest is recorded in the admitted phase object.
pub struct OriginVerifiedAdmission<V> {
    policy_id: String,
    verifier: V,
}

impl<V> OriginVerifiedAdmission<V> {
    pub fn new(policy_id: impl Into<String>, verifier: V) -> Result<Self, AdmissionError> {
        let policy_id = policy_id.into();
        if policy_id.trim().is_empty() {
            return Err(AdmissionError::InvalidPolicyId);
        }
        Ok(Self {
            policy_id,
            verifier,
        })
    }
}

impl<V: ArtifactOriginVerifier> ArtifactAdmissionPolicy for OriginVerifiedAdmission<V> {
    fn admit(&self, artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError> {
        let evidence = self
            .verifier
            .verify_origin(artifact.bundle().digest(), artifact.bundle().provenance())
            .map_err(AdmissionError::rejected)?;
        ArtifactAdmission::new(&self.policy_id, Some(evidence))
    }
}

/// Host-side failure while admitting a structurally verified Artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    InvalidPolicyId,
    Rejected { message: String },
}

impl AdmissionError {
    pub fn rejected(message: impl Into<String>) -> Self {
        Self::Rejected {
            message: message.into(),
        }
    }
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicyId => formatter.write_str("artifact admission policy ID is empty"),
            Self::Rejected { message } => {
                write!(formatter, "artifact admission rejected: {message}")
            }
        }
    }
}

impl Error for AdmissionError {}

/// Structurally verified Artifact accepted by one explicit host policy.
#[derive(Debug)]
pub struct AdmittedArtifact {
    artifact: VerifiedArtifact,
    admission: ArtifactAdmission,
}

impl AdmittedArtifact {
    pub fn bundle(&self) -> &ArtifactBundle {
        self.artifact.bundle()
    }

    pub fn module_digest(&self) -> &str {
        self.artifact.module_digest()
    }

    pub fn external_imports(&self) -> &[ExternalImport] {
        self.artifact.external_imports()
    }

    pub fn admission(&self) -> &ArtifactAdmission {
        &self.admission
    }
}

#[derive(Default)]
pub struct ProviderRegistry {
    inner: ExternalFunctionRegistry,
}

impl ProviderRegistry {
    /// Attach host-defined, instance-local context to every resolved call.
    /// Providers decide how to interpret its labels; the language does not.
    pub fn set_host_call_context(&mut self, context: provider::HostCallContext) {
        self.inner.set_host_call_context(context);
    }

    pub fn register<T: Into<provider::ProviderCallable>>(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<provider::ExternalSymbol, ProviderFunction<T>>,
    ) -> Result<(), ProviderLoadError> {
        self.inner.register_provider(descriptor, functions)
    }
}

#[derive(Debug, Clone)]
pub struct RunLimits {
    max_depth: usize,
    step_budget: Option<u64>,
    allocation_budget: Option<usize>,
    live_memory_limit: Option<usize>,
    cancellation: Option<CancellationToken>,
    deadline: Option<MonotonicDeadline>,
    output_budget: Option<usize>,
    intrinsic_call_budget: Option<u64>,
    provider_call_budget: Option<u64>,
    resource_limit: Option<usize>,
    allow_blocking_provider_calls: bool,
}

impl RunLimits {
    /// Return the bounded public execution defaults.
    pub fn bounded() -> Self {
        Self::default()
    }

    /// Disable budgets for a host-controlled, trusted workload.
    ///
    /// This does not create an isolation boundary. The embedding host remains
    /// responsible for process isolation and provider authority.
    pub fn unbounded_for_trusted_host() -> Self {
        VmLimits::unbounded_for_trusted_host().into()
    }

    pub fn with_max_depth(mut self, maximum: usize) -> Self {
        self.max_depth = maximum;
        self
    }

    pub fn with_step_budget(mut self, budget: u64) -> Self {
        self.step_budget = Some(budget);
        self
    }

    pub fn with_allocation_budget(mut self, budget: usize) -> Self {
        self.allocation_budget = Some(budget);
        self
    }

    pub fn with_live_memory_limit(mut self, limit: usize) -> Self {
        self.live_memory_limit = Some(limit);
        self
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_deadline(mut self, deadline: MonotonicDeadline) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn with_output_budget(mut self, budget: usize) -> Self {
        self.output_budget = Some(budget);
        self
    }

    pub fn with_intrinsic_call_budget(mut self, budget: u64) -> Self {
        self.intrinsic_call_budget = Some(budget);
        self
    }

    pub fn with_provider_call_budget(mut self, budget: u64) -> Self {
        self.provider_call_budget = Some(budget);
        self
    }

    pub fn with_resource_limit(mut self, limit: usize) -> Self {
        self.resource_limit = Some(limit);
        self
    }

    pub fn allow_blocking_provider_calls(mut self, allow: bool) -> Self {
        self.allow_blocking_provider_calls = allow;
        self
    }

    pub fn blocking_provider_calls_allowed(&self) -> bool {
        self.allow_blocking_provider_calls
    }
}

impl Default for RunLimits {
    fn default() -> Self {
        VmLimits::default().into()
    }
}

impl From<VmLimits> for RunLimits {
    fn from(limits: VmLimits) -> Self {
        Self {
            max_depth: limits.max_depth,
            step_budget: limits.step_budget,
            allocation_budget: limits.allocation_budget,
            live_memory_limit: limits.live_memory_limit,
            cancellation: limits.cancel,
            deadline: limits.deadline,
            output_budget: limits.stdout_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
            allow_blocking_provider_calls: limits.allow_blocking_provider_calls,
        }
    }
}

impl From<RunLimits> for VmLimits {
    fn from(limits: RunLimits) -> Self {
        Self {
            max_depth: limits.max_depth,
            step_budget: limits.step_budget,
            allocation_budget: limits.allocation_budget,
            live_memory_limit: limits.live_memory_limit,
            cancel: limits.cancellation,
            deadline: limits.deadline,
            stdout_budget: limits.output_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
            allow_blocking_provider_calls: limits.allow_blocking_provider_calls,
        }
    }
}

pub struct Runtime {
    providers: ProviderRegistry,
}

impl Runtime {
    pub fn new(providers: ProviderRegistry) -> Self {
        Self { providers }
    }

    /// Resolve every external import before any instruction can run.
    /// `LinkedArtifact` has no public constructor, so the
    /// stable SDK cannot bypass Provider preflight.
    pub fn link<'artifact>(
        &self,
        artifact: &'artifact AdmittedArtifact,
    ) -> Result<LinkedArtifact<'artifact>, LinkError> {
        for import in artifact.external_imports() {
            if let Err(error) = self.providers.inner.resolve(import) {
                return Err(LinkError::Provider(error));
            }
        }
        Ok(LinkedArtifact {
            artifact,
            bindings: self.providers.inner.bindings().collect(),
            policy: LinkedExecutionPolicy::RequestControlled,
        })
    }

    /// Link under one host-owned deployment profile. The profile is not part
    /// of source or Artifact data and therefore cannot widen its own Provider,
    /// admission, budget, isolation, or audit authority.
    pub fn link_with_profile<'artifact>(
        &self,
        artifact: &'artifact AdmittedArtifact,
        profile: ExecutionProfileV1,
    ) -> Result<LinkedArtifact<'artifact>, LinkError> {
        profile.validate_artifact(artifact)?;
        for import in artifact.external_imports() {
            self.providers
                .inner
                .resolve(import)
                .map_err(LinkError::Provider)?;
        }
        Ok(LinkedArtifact {
            artifact,
            bindings: self.providers.inner.bindings().collect(),
            policy: LinkedExecutionPolicy::Profile(Box::new(profile)),
        })
    }
}

pub struct LinkedArtifact<'artifact> {
    artifact: &'artifact AdmittedArtifact,
    bindings: Vec<(String, ExternalFunction)>,
    policy: LinkedExecutionPolicy,
}

enum LinkedExecutionPolicy {
    RequestControlled,
    Profile(Box<ExecutionProfileV1>),
}

impl LinkedArtifact<'_> {
    pub fn module_digest(&self) -> &str {
        self.artifact.module_digest()
    }

    /// Execute and always return an audit report, including partial evidence
    /// for cancellation, budget exhaustion, and Provider failures.
    pub fn execute(&self, request: ExecutionRequest) -> ExecutionReport {
        let started = Instant::now();
        let mut request = request;
        if let LinkedExecutionPolicy::Profile(profile) = &self.policy {
            request.limits = profile.run_limits.clone();
            request.trace_policy = profile.audit_policy.trace_policy();
        }
        let limits: VmLimits = request.limits.into();
        #[cfg(feature = "native-jit")]
        let execution = if let Some(options) = request.native_jit {
            self.artifact
                .artifact
                .executable
                .execute_main_with_args_and_external_bindings_native_options_and_limits(
                    request.args,
                    self.bindings.iter().cloned(),
                    options,
                    limits.clone(),
                )
        } else {
            self.artifact
                .artifact
                .executable
                .execute_main_with_args_and_external_bindings_and_limits(
                    request.args,
                    self.bindings.iter().cloned(),
                    limits.clone(),
                )
        };
        #[cfg(not(feature = "native-jit"))]
        let execution = self
            .artifact
            .artifact
            .executable
            .execute_main_with_args_and_external_bindings_and_limits(
                request.args,
                self.bindings.iter().cloned(),
                limits.clone(),
            );
        let output = match execution {
            Ok(output) => output,
            Err(error) => {
                let diagnostics = match &error {
                    EvalError::Diagnostics(diagnostics) => diagnostics.clone(),
                    _ => Vec::new(),
                };
                return ExecutionReport::failed(
                    self.artifact.module_digest(),
                    RuntimeError::from_execution(error),
                    diagnostics,
                    started.elapsed(),
                    limits.cancel.as_ref(),
                );
            }
        };
        let diagnostics = match &output.failure {
            Some(EvalError::Diagnostics(diagnostics)) => diagnostics.clone(),
            _ => Vec::new(),
        };
        let outcome = match output.failure.map(RuntimeError::from_execution) {
            Some(failure) => ExecutionOutcome::Failed(failure),
            None => ExecutionOutcome::Completed {
                wire_value: output.wire_value,
                display_value: output.display_value.unwrap_or_default(),
            },
        };
        let termination_reason = outcome.termination_reason();
        let telemetry = ExecutionTelemetry::from_traces(
            started.elapsed(),
            termination_reason,
            limits.cancel.as_ref(),
            &output.provider_call_traces,
            output.engine,
        );
        ExecutionReport {
            schema: EXECUTION_REPORT_SCHEMA,
            artifact_digest: self.artifact.module_digest().to_string(),
            usage: output.usage,
            telemetry,
            outcome,
            stdout: output.stdout,
            stderr: output.stderr,
            provider_call_traces: match request.trace_policy {
                TracePolicy::None => Vec::new(),
                TracePolicy::MetadataOnly | TracePolicy::RedactedDebug => {
                    output.provider_call_traces
                }
            },
            diagnostics,
        }
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(ProviderRegistry::default())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TracePolicy {
    #[default]
    None,
    MetadataOnly,
    RedactedDebug,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NondeterminismPolicy {
    #[default]
    Deny,
    ExplicitProvidersOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuditPolicy {
    #[default]
    MetadataOnly,
    None,
    RedactedDebug,
}

impl AuditPolicy {
    const fn trace_policy(self) -> TracePolicy {
        match self {
            Self::None => TracePolicy::None,
            Self::MetadataOnly => TracePolicy::MetadataOnly,
            Self::RedactedDebug => TracePolicy::RedactedDebug,
        }
    }
}

/// Versioned host deployment contract enforced at link and run time.
#[derive(Debug, Clone)]
pub struct ExecutionProfileV1 {
    profile_id: String,
    provider_interfaces: BTreeMap<String, (String, u32)>,
    run_limits: RunLimits,
    admission_policy_id: String,
    isolation_profile_id: String,
    nondeterminism_policy: NondeterminismPolicy,
    audit_policy: AuditPolicy,
}

impl ExecutionProfileV1 {
    pub fn new(
        profile_id: impl Into<String>,
        run_limits: RunLimits,
        admission_policy_id: impl Into<String>,
        isolation_profile_id: impl Into<String>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            provider_interfaces: BTreeMap::new(),
            run_limits,
            admission_policy_id: admission_policy_id.into(),
            isolation_profile_id: isolation_profile_id.into(),
            nondeterminism_policy: NondeterminismPolicy::Deny,
            audit_policy: AuditPolicy::MetadataOnly,
        }
    }

    pub fn allow_provider_interface(
        mut self,
        symbol: impl Into<String>,
        signature_hash: impl Into<String>,
        abi_version: u32,
    ) -> Self {
        self.provider_interfaces
            .insert(symbol.into(), (signature_hash.into(), abi_version));
        self
    }

    pub fn nondeterminism(mut self, policy: NondeterminismPolicy) -> Self {
        self.nondeterminism_policy = policy;
        self
    }

    pub fn audit(mut self, policy: AuditPolicy) -> Self {
        self.audit_policy = policy;
        self
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn isolation_profile_id(&self) -> &str {
        &self.isolation_profile_id
    }

    pub const fn nondeterminism_policy(&self) -> NondeterminismPolicy {
        self.nondeterminism_policy
    }

    pub const fn audit_policy(&self) -> AuditPolicy {
        self.audit_policy
    }

    fn validate_artifact(&self, artifact: &AdmittedArtifact) -> Result<(), LinkError> {
        if self.profile_id.trim().is_empty() || self.isolation_profile_id.trim().is_empty() {
            return Err(LinkError::Profile(
                "execution profile IDs must be non-empty".to_string(),
            ));
        }
        if artifact.admission().policy_id() != self.admission_policy_id {
            return Err(LinkError::Profile(format!(
                "artifact admission `{}` does not match profile requirement `{}`",
                artifact.admission().policy_id(),
                self.admission_policy_id
            )));
        }
        for import in artifact.external_imports() {
            let Some((signature_hash, abi_version)) =
                self.provider_interfaces.get(import.symbol.as_str())
            else {
                return Err(LinkError::Profile(format!(
                    "Provider import `{}` is not allowed by execution profile `{}`",
                    import.symbol.as_str(),
                    self.profile_id
                )));
            };
            if signature_hash != import.signature_hash.as_str()
                || *abi_version != import.abi_version
            {
                return Err(LinkError::Profile(format!(
                    "Provider import `{}` does not match the execution profile contract",
                    import.symbol.as_str()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    args: Vec<String>,
    limits: RunLimits,
    trace_policy: TracePolicy,
    #[cfg(feature = "native-jit")]
    native_jit: Option<NativeJitOptions>,
}

impl ExecutionRequest {
    pub fn new(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            limits: RunLimits::bounded(),
            trace_policy: TracePolicy::None,
            #[cfg(feature = "native-jit")]
            native_jit: None,
        }
    }

    pub fn limits(mut self, limits: RunLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn trace(mut self, policy: TracePolicy) -> Self {
        self.trace_policy = policy;
        self
    }

    /// Select adaptive Cranelift execution using an explicit host-owned policy.
    /// This does not alter the requested limits. Armed limits remain
    /// authoritative and may keep unsupported native regions on the reference
    /// interpreter. A host that deliberately wants unrestricted execution must
    /// separately select [`RunLimits::unbounded_for_trusted_host`].
    #[cfg(feature = "native-jit")]
    pub fn native_jit(mut self, options: NativeJitOptions) -> Self {
        self.native_jit = Some(options);
        self
    }
}

impl Default for ExecutionRequest {
    fn default() -> Self {
        Self::new(std::iter::empty::<String>())
    }
}

/// The canonical report schema emitted by the reviewed SDK.
///
/// Version 2 makes the mutually-exclusive [`ExecutionOutcome`] explicit and
/// carries only [`provider::WireValue`] for a completed program result.  The
/// previous v1 JSON document included a `NativeValue` compatibility projection
/// and remains a historical reader fixture, but is no longer emitted by the
/// reviewed execution path.
pub const EXECUTION_REPORT_SCHEMA: &str = "rsscript.execution_report.v2";

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionTelemetry {
    pub execution_duration_ns: u64,
    pub cancellation_latency_ns: Option<u64>,
    pub provider_functions: Vec<ProviderFunctionTelemetry>,
    pub engine: ExecutionEngineTelemetry,
}

impl ExecutionTelemetry {
    fn from_traces(
        elapsed: Duration,
        termination_reason: TerminationReason,
        cancellation: Option<&CancellationToken>,
        traces: &[provider::ProviderCallTrace],
        engine: ExecutionEngineTelemetry,
    ) -> Self {
        let mut summaries = BTreeMap::<(String, String, String), ProviderFunctionTelemetry>::new();
        for trace in traces {
            let key = (
                trace.provider_id.clone(),
                trace.provider_version.clone(),
                trace.symbol.clone(),
            );
            let summary = summaries
                .entry(key)
                .or_insert_with(|| ProviderFunctionTelemetry {
                    provider_id: trace.provider_id.clone(),
                    provider_version: trace.provider_version.clone(),
                    symbol: trace.symbol.clone(),
                    ..ProviderFunctionTelemetry::default()
                });
            summary.calls = summary.calls.saturating_add(1);
            summary.failures = summary
                .failures
                .saturating_add(u64::from(trace.result.is_err()));
            summary.request_bytes = summary.request_bytes.saturating_add(trace.request_bytes);
            summary.response_bytes = summary.response_bytes.saturating_add(trace.response_bytes);
            let elapsed_ns = duration_ns(trace.elapsed);
            summary.total_duration_ns = summary.total_duration_ns.saturating_add(elapsed_ns);
            summary.max_duration_ns = summary.max_duration_ns.max(elapsed_ns);
        }
        let cancellation_latency_ns = (termination_reason == TerminationReason::Cancelled)
            .then(|| cancellation.and_then(CancellationToken::cancelled_at))
            .flatten()
            .map(|cancelled_at| duration_ns(cancelled_at.elapsed()));
        Self {
            execution_duration_ns: duration_ns(elapsed),
            cancellation_latency_ns,
            provider_functions: summaries.into_values().collect(),
            engine,
        }
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProviderFunctionTelemetry {
    pub provider_id: String,
    pub provider_version: String,
    pub symbol: String,
    pub calls: u64,
    pub failures: u64,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub total_duration_ns: u64,
    pub max_duration_ns: u64,
}

/// Mutually exclusive script-level completion states.
///
/// An execution report always has exactly one of these outcomes. Host protocol,
/// linking, and verification failures stay outside execution and therefore do
/// not manufacture a partial report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Completed {
        /// Canonical result value. A legacy v1 Artifact without enough named
        /// type-layout information yields `None` rather than exposing a
        /// stringly/dynamic compatibility value through the reviewed SDK.
        wire_value: Option<provider::WireValue>,
        display_value: String,
    },
    Failed(RuntimeError),
}

impl ExecutionOutcome {
    pub const fn termination_reason(&self) -> TerminationReason {
        match self {
            Self::Completed { .. } => TerminationReason::Completed,
            Self::Failed(error) => error.reason,
        }
    }

    /// Canonical completed result, when the Artifact can prove its structural
    /// wire layout. New embedders should use this instead of parsing display
    /// text.
    pub const fn wire_value(&self) -> Option<&provider::WireValue> {
        match self {
            Self::Completed { wire_value, .. } => wire_value.as_ref(),
            Self::Failed(_) => None,
        }
    }

    /// Human-readable v1 compatibility projection of a completed result.
    /// This is not a typed ABI value; use [`Self::wire_value`] for new host
    /// integrations.
    pub fn value(&self) -> Option<&str> {
        self.display_value()
    }

    pub fn display_value(&self) -> Option<&str> {
        match self {
            Self::Completed { display_value, .. } => Some(display_value),
            Self::Failed(_) => None,
        }
    }

    pub const fn failure(&self) -> Option<&RuntimeError> {
        match self {
            Self::Completed { .. } => None,
            Self::Failed(error) => Some(error),
        }
    }
}

/// Structured execution evidence for one linked Artifact run.
///
/// [`Self::outcome`] is the only terminal program state. The v2 JSON schema
/// serializes this same typed state directly, so machine consumers do not need
/// a dynamic `NativeValue` compatibility projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionReport {
    pub schema: &'static str,
    pub artifact_digest: String,
    outcome: ExecutionOutcome,
    pub usage: ExecutionUsage,
    pub telemetry: ExecutionTelemetry,
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<provider::ProviderCallTrace>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ExecutionReport {
    pub const fn outcome(&self) -> &ExecutionOutcome {
        &self.outcome
    }

    pub const fn termination_reason(&self) -> TerminationReason {
        self.outcome.termination_reason()
    }

    pub fn value(&self) -> Option<&str> {
        self.outcome.value()
    }

    /// Canonical completed result value. See [`ExecutionOutcome::wire_value`]
    /// for the intentional `None` behaviour of v1 named variants, whose
    /// layouts are not present in that Artifact format.
    pub const fn wire_value(&self) -> Option<&provider::WireValue> {
        self.outcome.wire_value()
    }

    pub fn display_value(&self) -> Option<&str> {
        self.outcome.display_value()
    }

    pub const fn failure(&self) -> Option<&RuntimeError> {
        self.outcome.failure()
    }

    pub(super) fn failed(
        artifact_digest: impl Into<String>,
        failure: RuntimeError,
        diagnostics: Vec<Diagnostic>,
        elapsed: Duration,
        cancellation: Option<&CancellationToken>,
    ) -> Self {
        let outcome = ExecutionOutcome::Failed(failure);
        let termination_reason = outcome.termination_reason();
        Self {
            schema: EXECUTION_REPORT_SCHEMA,
            artifact_digest: artifact_digest.into(),
            outcome,
            usage: ExecutionUsage::default(),
            telemetry: ExecutionTelemetry::from_traces(
                elapsed,
                termination_reason,
                cancellation,
                &[],
                ExecutionEngineTelemetry::Interpreter,
            ),
            stdout: String::new(),
            stderr: String::new(),
            provider_call_traces: Vec::new(),
            diagnostics,
        }
    }
}

/// Serialize the canonical v2 report contract.  Unlike v1, this document has
/// one explicit terminal outcome and never serializes `NativeValue`.
impl serde::Serialize for ExecutionReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ExecutionReport", 9)?;
        state.serialize_field("schema", self.schema)?;
        state.serialize_field("artifact_digest", &self.artifact_digest)?;
        state.serialize_field("outcome", &self.outcome)?;
        state.serialize_field("usage", &self.usage)?;
        state.serialize_field("telemetry", &self.telemetry)?;
        state.serialize_field("stdout", &self.stdout)?;
        state.serialize_field("stderr", &self.stderr)?;
        state.serialize_field("provider_call_traces", &self.provider_call_traces)?;
        state.serialize_field("diagnostics", &self.diagnostics)?;
        state.end()
    }
}
