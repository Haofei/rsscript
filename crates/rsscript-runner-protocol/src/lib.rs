#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

pub const RUNNER_REQUEST_SCHEMA: &str = "rsscript.runner_request.v1";
pub const RUNNER_RESPONSE_SCHEMA: &str = "rsscript.runner_response.v1";
const REQUEST_MAGIC: &[u8; 8] = b"RSSRUNQ1";
const RESPONSE_MAGIC: &[u8; 8] = b"RSSRUNS1";
pub const MAX_HEADER_BYTES: usize = 1024 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ARGUMENTS: usize = 256;
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
pub const MAX_WALL_TIME_MS: u64 = 60_000;
pub const MAX_DEPTH: usize = 1024;
pub const MAX_STEP_BUDGET: u64 = 100_000_000;
pub const MAX_ALLOCATION_BUDGET: usize = 512 * 1024 * 1024;
pub const MAX_LIVE_MEMORY_LIMIT: usize = 256 * 1024 * 1024;
pub const MAX_OUTPUT_BUDGET: usize = 4 * 1024 * 1024;
pub const MAX_INTRINSIC_CALL_BUDGET: u64 = 10_000_000;
pub const MAX_PROVIDER_CALL_BUDGET: u64 = 100_000;
pub const MAX_RESOURCE_LIMIT: usize = 16_384;

/// Host-selected, preinstalled Provider profile. The protocol deliberately has
/// no field for Provider code, library paths, credentials, roots, or authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerProfileV1 {
    /// Reference fail-closed profile: no external Provider is linkable.
    #[default]
    NoProviders,
    /// Reference fail-closed profile with no Providers plus an explicitly
    /// required Linux user/mount namespace boundary. Hosts that cannot install
    /// those controls reject the launch before the runner parses an Artifact.
    NoProvidersNamespaced,
    /// Reference fail-closed profile with no Providers plus user, mount, and
    /// network namespaces. The child starts without the host network namespace;
    /// a host that cannot enforce that boundary rejects the launch.
    NoProvidersNetworkIsolated,
    /// Reference fail-closed profile with no Providers and a host-owned empty
    /// filesystem root. The runner applies a Linux Landlock allowlist before
    /// Artifact parsing; the request cannot choose or widen that root.
    NoProvidersFilesystemIsolated,
    /// Reference allowlisted profile with only `host.log.emit` installed.
    ///
    /// The sink is selected by the runner host and has no filesystem, network,
    /// process, credential, or ambient-environment authority. This profile
    /// exists to exercise a non-empty, preinstalled Provider allowlist without
    /// allowing a request to inject Provider code or configuration.
    LogOnly,
}

/// Stable, non-secret identity of the host-selected runner profile.
///
/// This is response evidence, not a capability grant. Provider code,
/// credentials, filesystem roots, endpoints, and other authorities remain
/// deliberately absent from the protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerProfileIdentityV1 {
    pub id: String,
    pub version: u32,
    pub descriptor_digest: String,
}

impl RunnerProfileV1 {
    pub fn identity(self) -> RunnerProfileIdentityV1 {
        match self {
            Self::NoProviders => RunnerProfileIdentityV1 {
                id: "rsscript.runner.no_providers".to_string(),
                version: 1,
                descriptor_digest:
                    "sha256:59e7504d735fe8ba29a406c993312a784d338892b279c60d6bdb5670165745dd"
                        .to_string(),
            },
            // sha256 of the versioned no-Provider profile descriptor plus its
            // user/mount namespace requirement. It carries no authority data.
            Self::NoProvidersNamespaced => RunnerProfileIdentityV1 {
                id: "rsscript.runner.no_providers_namespaced".to_string(),
                version: 1,
                descriptor_digest:
                    "sha256:75a022141b604c70f400232a509942e5b232f9a2b11a4d8f42c47b3ae797bcbb"
                        .to_string(),
            },
            // sha256 of the versioned no-Provider profile descriptor plus its
            // user/mount/network namespace requirement.
            Self::NoProvidersNetworkIsolated => RunnerProfileIdentityV1 {
                id: "rsscript.runner.no_providers_network_isolated".to_string(),
                version: 1,
                descriptor_digest:
                    "sha256:a44a14fe53c94d34f46b99cb2fe3176a614c5574abab276aaf3a607c348b9e8b"
                        .to_string(),
            },
            // sha256 of the versioned no-Provider profile descriptor plus its
            // host-owned empty Landlock root requirement.
            Self::NoProvidersFilesystemIsolated => RunnerProfileIdentityV1 {
                id: "rsscript.runner.no_providers_filesystem_isolated".to_string(),
                version: 1,
                descriptor_digest:
                    "sha256:096864fb62a41bcf0aac402fd117e4b24bc6c9393e4ba1a2bde964cde0e5592f"
                        .to_string(),
            },
            // sha256 of the versioned profile descriptor containing the
            // `rsscript.log` / `host.log.emit` read-String-to-Unit contract.
            // It is a profile identity, not a request-supplied authority.
            Self::LogOnly => RunnerProfileIdentityV1 {
                id: "rsscript.runner.log_only".to_string(),
                version: 1,
                descriptor_digest:
                    "sha256:532783c81dcde5137cdae02a8f5a77cfd579f1f216b97feb80b78a2df30b4ac0"
                        .to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerLimitsV1 {
    pub wall_time_ms: u64,
    pub max_depth: usize,
    pub step_budget: u64,
    pub allocation_budget: usize,
    pub live_memory_limit: usize,
    pub output_budget: usize,
    pub intrinsic_call_budget: u64,
    pub provider_call_budget: u64,
    pub resource_limit: usize,
}

impl Default for RunnerLimitsV1 {
    fn default() -> Self {
        Self {
            wall_time_ms: 60_000,
            max_depth: 256,
            step_budget: 10_000_000,
            allocation_budget: 256 * 1024 * 1024,
            live_memory_limit: 128 * 1024 * 1024,
            output_budget: 1024 * 1024,
            intrinsic_call_budget: 1_000_000,
            provider_call_budget: 10_000,
            resource_limit: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerRequestV1 {
    pub schema: String,
    pub profile: RunnerProfileV1,
    pub args: Vec<String>,
    pub limits: RunnerLimitsV1,
    pub metadata_only_trace: bool,
}

impl RunnerRequestV1 {
    pub fn new(args: Vec<String>) -> Result<Self, ProtocolError> {
        Self::with_profile(args, RunnerProfileV1::default())
    }

    /// Construct a request for one compile-time known, host-selected profile.
    /// The profile enum intentionally cannot carry Provider libraries,
    /// credentials, roots, endpoints, or other authority-bearing inputs.
    pub fn with_profile(
        args: Vec<String>,
        profile: RunnerProfileV1,
    ) -> Result<Self, ProtocolError> {
        validate_args(&args)?;
        Ok(Self {
            schema: RUNNER_REQUEST_SCHEMA.to_string(),
            profile,
            args,
            limits: RunnerLimitsV1::default(),
            metadata_only_trace: true,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerTerminationV1 {
    Completed,
    /// The child refused to execute because a declared runner isolation
    /// control was not installed or could not be verified.  This is separate
    /// from both Artifact verification and VM/script termination.
    IsolationRejected,
    VerificationRejected,
    LinkRejected,
    ProtocolRejected,
    HostFailure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerResponseV1 {
    pub schema: String,
    pub profile: RunnerProfileIdentityV1,
    pub runner_termination: RunnerTerminationV1,
    pub report: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl RunnerResponseV1 {
    pub fn report(profile: RunnerProfileV1, report: serde_json::Value) -> Self {
        Self {
            schema: RUNNER_RESPONSE_SCHEMA.to_string(),
            profile: profile.identity(),
            runner_termination: RunnerTerminationV1::Completed,
            report: Some(report),
            error: None,
        }
    }

    pub fn rejected(
        profile: RunnerProfileV1,
        termination: RunnerTerminationV1,
        error: impl Into<String>,
    ) -> Self {
        Self {
            schema: RUNNER_RESPONSE_SCHEMA.to_string(),
            profile: profile.identity(),
            runner_termination: termination,
            report: None,
            error: Some(error.into()),
        }
    }
}

/// Reject a response from a child that claims an unexpected host profile.
///
/// The parent already knows the requested profile and must not infer it from a
/// child-controlled JSON frame. Keeping this check in the protocol crate makes
/// the response identity useful to every runner host, not only the CLI.
pub fn validate_response_profile(
    requested: RunnerProfileV1,
    response: &RunnerResponseV1,
) -> Result<(), ProtocolError> {
    if response.profile == requested.identity() {
        Ok(())
    } else {
        Err(ProtocolError::ProfileMismatch {
            expected: requested.identity(),
            actual: response.profile.clone(),
        })
    }
}

pub fn write_request(
    mut output: impl Write,
    request: &RunnerRequestV1,
    bundle: &[u8],
) -> Result<(), ProtocolError> {
    validate_request(request)?;
    if bundle.len() > MAX_BUNDLE_BYTES {
        return Err(ProtocolError::Limit("Artifact Bundle"));
    }
    let header = serde_json::to_vec(request).map_err(ProtocolError::Json)?;
    write_frame_header(&mut output, REQUEST_MAGIC, header.len(), bundle.len())?;
    output.write_all(&header)?;
    output.write_all(bundle)?;
    output.flush()?;
    Ok(())
}

pub fn read_request(mut input: impl Read) -> Result<(RunnerRequestV1, Vec<u8>), ProtocolError> {
    let (header_len, bundle_len) = read_frame_header(&mut input, REQUEST_MAGIC)?;
    if header_len > MAX_HEADER_BYTES || bundle_len > MAX_BUNDLE_BYTES {
        return Err(ProtocolError::Limit("runner request"));
    }
    let mut header = vec![0; header_len];
    input.read_exact(&mut header)?;
    let request: RunnerRequestV1 = serde_json::from_slice(&header).map_err(ProtocolError::Json)?;
    validate_request(&request)?;
    let mut bundle = vec![0; bundle_len];
    input.read_exact(&mut bundle)?;
    let mut trailing = [0_u8; 1];
    if input.read(&mut trailing)? != 0 {
        return Err(ProtocolError::TrailingBytes);
    }
    Ok((request, bundle))
}

pub fn write_response(
    mut output: impl Write,
    response: &RunnerResponseV1,
) -> Result<(), ProtocolError> {
    validate_response(response)?;
    let bytes = serde_json::to_vec(response).map_err(ProtocolError::Json)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::Limit("runner response"));
    }
    output.write_all(RESPONSE_MAGIC)?;
    output.write_all(&(bytes.len() as u64).to_be_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

pub fn read_response(mut input: impl Read) -> Result<RunnerResponseV1, ProtocolError> {
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != RESPONSE_MAGIC {
        return Err(ProtocolError::Magic);
    }
    let length = read_u64(&mut input)?;
    let length = usize::try_from(length).map_err(|_| ProtocolError::Limit("runner response"))?;
    if length > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::Limit("runner response"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let mut trailing = [0_u8; 1];
    if input.read(&mut trailing)? != 0 {
        return Err(ProtocolError::TrailingBytes);
    }
    let response: RunnerResponseV1 = serde_json::from_slice(&bytes).map_err(ProtocolError::Json)?;
    validate_response(&response)?;
    Ok(response)
}

fn validate_request(request: &RunnerRequestV1) -> Result<(), ProtocolError> {
    if request.schema != RUNNER_REQUEST_SCHEMA {
        return Err(ProtocolError::Schema(request.schema.clone()));
    }
    validate_args(&request.args)?;
    if request.limits.wall_time_ms == 0
        || request.limits.max_depth == 0
        || request.limits.step_budget == 0
        || request.limits.output_budget == 0
    {
        return Err(ProtocolError::Invalid("runner limits must be non-zero"));
    }
    if request.limits.wall_time_ms > MAX_WALL_TIME_MS
        || request.limits.max_depth > MAX_DEPTH
        || request.limits.step_budget > MAX_STEP_BUDGET
        || request.limits.allocation_budget > MAX_ALLOCATION_BUDGET
        || request.limits.live_memory_limit > MAX_LIVE_MEMORY_LIMIT
        || request.limits.output_budget > MAX_OUTPUT_BUDGET
        || request.limits.intrinsic_call_budget > MAX_INTRINSIC_CALL_BUDGET
        || request.limits.provider_call_budget > MAX_PROVIDER_CALL_BUDGET
        || request.limits.resource_limit > MAX_RESOURCE_LIMIT
    {
        return Err(ProtocolError::Limit("runner limits"));
    }
    Ok(())
}

/// Validate the response state machine independently of JSON Schema. The
/// child process is an untrusted protocol peer: callers must not treat a frame
/// as a completed execution merely because it decoded successfully.
fn validate_response(response: &RunnerResponseV1) -> Result<(), ProtocolError> {
    if response.schema != RUNNER_RESPONSE_SCHEMA {
        return Err(ProtocolError::Schema(response.schema.clone()));
    }
    match response.runner_termination {
        RunnerTerminationV1::Completed if response.report.is_some() && response.error.is_none() => {
            Ok(())
        }
        RunnerTerminationV1::Completed => Err(ProtocolError::Invalid(
            "completed runner response requires a report and no error",
        )),
        _ if response.report.is_none() && response.error.is_some() => Ok(()),
        _ => Err(ProtocolError::Invalid(
            "rejected runner response requires an error and no report",
        )),
    }
}

fn validate_args(args: &[String]) -> Result<(), ProtocolError> {
    if args.len() > MAX_ARGUMENTS {
        return Err(ProtocolError::Limit("arguments"));
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_ARGUMENT_BYTES)
    {
        return Err(ProtocolError::Limit("argument bytes"));
    }
    Ok(())
}

fn write_frame_header(
    output: &mut impl Write,
    magic: &[u8; 8],
    header_len: usize,
    bundle_len: usize,
) -> Result<(), ProtocolError> {
    if header_len > MAX_HEADER_BYTES {
        return Err(ProtocolError::Limit("runner header"));
    }
    output.write_all(magic)?;
    output.write_all(&(header_len as u64).to_be_bytes())?;
    output.write_all(&(bundle_len as u64).to_be_bytes())?;
    Ok(())
}

fn read_frame_header(
    input: &mut impl Read,
    expected: &[u8; 8],
) -> Result<(usize, usize), ProtocolError> {
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != expected {
        return Err(ProtocolError::Magic);
    }
    let header =
        usize::try_from(read_u64(input)?).map_err(|_| ProtocolError::Limit("runner header"))?;
    let bundle =
        usize::try_from(read_u64(input)?).map_err(|_| ProtocolError::Limit("Artifact Bundle"))?;
    Ok((header, bundle))
}

fn read_u64(input: &mut impl Read) -> Result<u64, ProtocolError> {
    let mut bytes = [0_u8; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    Json(serde_json::Error),
    Magic,
    Schema(String),
    Limit(&'static str),
    Invalid(&'static str),
    ProfileMismatch {
        expected: RunnerProfileIdentityV1,
        actual: RunnerProfileIdentityV1,
    },
    TrailingBytes,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "runner I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "runner JSON failed: {error}"),
            Self::Magic => formatter.write_str("invalid runner protocol magic"),
            Self::Schema(schema) => write!(formatter, "unsupported runner schema `{schema}`"),
            Self::Limit(name) => write!(formatter, "{name} exceeds the protocol limit"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::ProfileMismatch { expected, actual } => write!(
                formatter,
                "runner response profile mismatch: expected {}@{} ({}) but received {}@{} ({})",
                expected.id,
                expected.version,
                expected.descriptor_digest,
                actual.id,
                actual.version,
                actual.descriptor_digest,
            ),
            Self::TrailingBytes => {
                formatter.write_str("runner protocol frame contains trailing bytes")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn request_and_response_round_trip_through_bounded_frames() {
        let request = RunnerRequestV1::new(vec!["hello".to_string()]).expect("request");
        let mut bytes = Vec::new();
        write_request(&mut bytes, &request, b"bundle").expect("encode request");
        let (decoded, bundle) = read_request(bytes.as_slice()).expect("decode request");
        assert_eq!(decoded, request);
        assert_eq!(bundle, b"bundle");

        let response = RunnerResponseV1::report(
            RunnerProfileV1::NoProviders,
            serde_json::json!({"ok": true}),
        );
        let mut bytes = Vec::new();
        write_response(&mut bytes, &response).expect("encode response");
        assert_eq!(
            read_response(bytes.as_slice()).expect("decode response"),
            response
        );
    }

    #[test]
    fn request_uses_an_explicit_fail_closed_profile() {
        let request = RunnerRequestV1::new(Vec::new()).expect("request");
        assert_eq!(request.profile, RunnerProfileV1::NoProviders);
        let json = serde_json::to_value(request).expect("request JSON");
        assert_eq!(json["profile"], "no_providers");
        for forbidden in ["provider", "library", "credential", "authority", "root"] {
            assert!(
                json.get(forbidden).is_none(),
                "protocol must not inject {forbidden}"
            );
        }
    }

    #[test]
    fn log_only_profile_is_a_distinct_preinstalled_allowlist() {
        let request =
            RunnerRequestV1::with_profile(Vec::new(), RunnerProfileV1::LogOnly).expect("request");
        assert_eq!(request.profile, RunnerProfileV1::LogOnly);
        let identity = request.profile.identity();
        assert_eq!(identity.id, "rsscript.runner.log_only");
        assert_ne!(identity, RunnerProfileV1::NoProviders.identity());
        let json = serde_json::to_value(request).expect("request JSON");
        for forbidden in [
            "provider",
            "library",
            "credential",
            "authority",
            "root",
            "endpoint",
        ] {
            assert!(
                json.get(forbidden).is_none(),
                "profile selection must not add request-supplied {forbidden}"
            );
        }
    }

    #[test]
    fn namespaced_profile_carries_no_provider_or_authority_input() {
        let request =
            RunnerRequestV1::with_profile(Vec::new(), RunnerProfileV1::NoProvidersNamespaced)
                .expect("request");
        let identity = request.profile.identity();
        assert_eq!(identity.id, "rsscript.runner.no_providers_namespaced");
        let json = serde_json::to_value(request).expect("request JSON");
        assert_eq!(json["profile"], "no_providers_namespaced");
        for forbidden in ["provider", "library", "credential", "authority", "root"] {
            assert!(
                json.get(forbidden).is_none(),
                "profile must not inject {forbidden}"
            );
        }
    }

    #[test]
    fn network_isolated_profile_is_host_selected_without_network_configuration() {
        let request =
            RunnerRequestV1::with_profile(Vec::new(), RunnerProfileV1::NoProvidersNetworkIsolated)
                .expect("request");
        let json = serde_json::to_value(request).expect("request JSON");
        assert_eq!(json["profile"], "no_providers_network_isolated");
        for forbidden in ["provider", "endpoint", "network", "credential", "authority"] {
            assert!(
                json.get(forbidden).is_none(),
                "profile must not inject {forbidden}"
            );
        }
    }

    #[test]
    fn filesystem_isolated_profile_carries_no_root_or_provider_input() {
        let request = RunnerRequestV1::with_profile(
            Vec::new(),
            RunnerProfileV1::NoProvidersFilesystemIsolated,
        )
        .expect("request");
        let json = serde_json::to_value(request).expect("request JSON");
        assert_eq!(json["profile"], "no_providers_filesystem_isolated");
        for forbidden in ["provider", "root", "path", "filesystem", "authority"] {
            assert!(
                json.get(forbidden).is_none(),
                "profile must not inject {forbidden}"
            );
        }
    }

    #[test]
    fn response_state_machine_rejects_report_error_ambiguity() {
        let completed_without_report = RunnerResponseV1 {
            schema: RUNNER_RESPONSE_SCHEMA.to_string(),
            profile: RunnerProfileV1::NoProviders.identity(),
            runner_termination: RunnerTerminationV1::Completed,
            report: None,
            error: None,
        };
        assert!(matches!(
            write_response(Vec::new(), &completed_without_report),
            Err(ProtocolError::Invalid(_))
        ));

        let rejected_with_report = RunnerResponseV1 {
            schema: RUNNER_RESPONSE_SCHEMA.to_string(),
            profile: RunnerProfileV1::NoProviders.identity(),
            runner_termination: RunnerTerminationV1::LinkRejected,
            report: Some(serde_json::json!({"forged": true})),
            error: Some("link failed".to_string()),
        };
        assert!(matches!(
            write_response(Vec::new(), &rejected_with_report),
            Err(ProtocolError::Invalid(_))
        ));
    }

    #[test]
    fn isolation_rejection_is_a_runner_failure_not_a_vm_report() {
        let response = RunnerResponseV1::rejected(
            RunnerProfileV1::NoProviders,
            RunnerTerminationV1::IsolationRejected,
            "required strict isolation control is unavailable",
        );
        assert!(response.report.is_none());
        assert!(response.error.is_some());
        let json = serde_json::to_value(response).expect("response JSON");
        assert_eq!(json["runner_termination"], "isolation_rejected");
    }

    #[test]
    fn wire_headers_match_checked_in_fail_closed_schemas() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let request_schema: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("schemas/rsscript.runner_request.v1.schema.json"))
                .expect("request schema"),
        )
        .expect("parse request schema");
        let response_schema: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("schemas/rsscript.runner_response.v1.schema.json"))
                .expect("response schema"),
        )
        .expect("parse response schema");
        let request = RunnerRequestV1::new(vec!["hello".to_string()]).expect("request");
        let response = RunnerResponseV1::report(
            RunnerProfileV1::NoProviders,
            serde_json::json!({"schema": "report"}),
        );
        assert!(
            jsonschema::validator_for(&request_schema)
                .expect("request validator")
                .is_valid(&serde_json::to_value(request).unwrap())
        );
        assert!(
            jsonschema::validator_for(&response_schema)
                .expect("response validator")
                .is_valid(&serde_json::to_value(response).unwrap())
        );
    }

    #[test]
    fn oversized_or_trailing_frames_fail_closed() {
        let request = RunnerRequestV1::new(Vec::new()).expect("request");
        let mut bytes = Vec::new();
        write_request(&mut bytes, &request, b"bundle").expect("request frame");
        bytes.push(0);
        assert!(matches!(
            read_request(bytes.as_slice()),
            Err(ProtocolError::TrailingBytes)
        ));
    }

    #[test]
    fn every_incomplete_request_and_response_frame_is_rejected_without_a_report() {
        let request = RunnerRequestV1::new(vec!["hello".to_string()]).expect("request");
        let mut request_bytes = Vec::new();
        write_request(&mut request_bytes, &request, b"bundle").expect("request frame");
        for length in 0..request_bytes.len() {
            assert!(
                matches!(
                    read_request(&request_bytes[..length]),
                    Err(ProtocolError::Io(_))
                ),
                "request prefix {length} must fail as incomplete I/O"
            );
        }

        let response = RunnerResponseV1::report(
            RunnerProfileV1::NoProviders,
            serde_json::json!({"ok": true}),
        );
        let mut response_bytes = Vec::new();
        write_response(&mut response_bytes, &response).expect("response frame");
        for length in 0..response_bytes.len() {
            assert!(
                matches!(
                    read_response(&response_bytes[..length]),
                    Err(ProtocolError::Io(_))
                ),
                "response prefix {length} must fail as incomplete I/O"
            );
        }
    }

    #[test]
    fn response_profile_must_match_the_parent_selected_profile() {
        let mut response = RunnerResponseV1::report(
            RunnerProfileV1::NoProviders,
            serde_json::json!({"ok": true}),
        );
        validate_response_profile(RunnerProfileV1::NoProviders, &response)
            .expect("matching profile");
        response.profile.descriptor_digest = "sha256:forged".to_string();
        assert!(matches!(
            validate_response_profile(RunnerProfileV1::NoProviders, &response),
            Err(ProtocolError::ProfileMismatch { .. })
        ));
    }

    #[test]
    fn declared_oversized_lengths_fail_before_payload_allocation() {
        let mut request = Vec::new();
        request.extend_from_slice(REQUEST_MAGIC);
        request.extend_from_slice(&((MAX_HEADER_BYTES as u64) + 1).to_be_bytes());
        request.extend_from_slice(&0_u64.to_be_bytes());
        assert!(matches!(
            read_request(request.as_slice()),
            Err(ProtocolError::Limit("runner request"))
        ));

        let mut response = Vec::new();
        response.extend_from_slice(RESPONSE_MAGIC);
        response.extend_from_slice(&((MAX_RESPONSE_BYTES as u64) + 1).to_be_bytes());
        assert!(matches!(
            read_response(response.as_slice()),
            Err(ProtocolError::Limit("runner response"))
        ));
    }
}
