use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Product support level for an execution capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportLevel {
    Core,
    Experimental,
    TrustedOnly,
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
            | Self::ArbitraryNetwork => SupportLevel::TrustedOnly,
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
/// RSScript does not provide an untrusted-package execution profile. Third-party
/// packages may be inspected statically, but execution is limited to explicitly
/// trusted local code or restricted trusted CI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeploymentProfile {
    #[default]
    LocalTrusted,
    TrustedCi,
}

impl DeploymentProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalTrusted => "local-trusted",
            Self::TrustedCi => "trusted-ci",
        }
    }

    pub fn authorize(self, capability: ExecutionCapability) -> Result<(), ExecutionPolicyError> {
        let allowed = match self {
            Self::LocalTrusted => true,
            Self::TrustedCi => matches!(
                capability,
                ExecutionCapability::StaticLowering | ExecutionCapability::BoundedReferenceVm
            ),
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
            "unknown deployment profile `{}`; expected local-trusted or trusted-ci",
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

/// Identity of one execution and its authority/cache scope.
///
/// Scope identifiers are process-local and deliberately cannot be selected by
/// callers. Host registries must include this value in cache keys whenever
/// resources must not cross execution boundaries.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionScopeId(u64);

impl ExecutionScopeId {
    fn fresh() -> Self {
        static NEXT_SCOPE_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_SCOPE_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id != u64::MAX, "execution scope identifier space exhausted");
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An exact network endpoint grant.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NetworkEndpointGrant {
    scheme: String,
    host: String,
    port: u16,
}

/// An authorized filesystem path bound to one execution scope.
///
/// The fields are private so restricted callers cannot manufacture authority
/// by wrapping an arbitrary host path. Host adapters should accept this type
/// instead of accepting a raw path plus a separate authorization boolean.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedPath {
    scope: ExecutionScopeId,
    path: PathBuf,
}

impl AuthorizedPath {
    pub const fn scope_id(&self) -> ExecutionScopeId {
        self.scope
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// An exact authorized network endpoint bound to one execution scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedEndpoint {
    scope: ExecutionScopeId,
    endpoint: NetworkEndpointGrant,
}

impl AuthorizedEndpoint {
    pub const fn scope_id(&self) -> ExecutionScopeId {
        self.scope
    }

    pub fn endpoint(&self) -> &NetworkEndpointGrant {
        &self.endpoint
    }
}

/// An authorized executable identity bound to one execution scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedExecutable {
    scope: ExecutionScopeId,
    path: PathBuf,
}

impl AuthorizedExecutable {
    pub const fn scope_id(&self) -> ExecutionScopeId {
        self.scope
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// An authorized logical database identity bound to one execution scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedDatabase {
    scope: ExecutionScopeId,
    logical_id: String,
}

impl AuthorizedDatabase {
    pub const fn scope_id(&self) -> ExecutionScopeId {
        self.scope
    }

    pub fn logical_id(&self) -> &str {
        &self.logical_id
    }
}

/// Canonical host-adapter boundary for one execution scope.
///
/// Restricted callers first ask [`ExecutionContext`] to mint an exact
/// capability and then pass that capability through this adapter before
/// touching the host. Keeping scope validation here prevents individual
/// filesystem, network, process, and database adapters from reimplementing
/// capability checks or accepting a detached authorization boolean.
#[derive(Clone, Copy, Debug)]
pub struct ScopedHostAdapters<'a> {
    context: &'a ExecutionContext,
}

impl<'a> ScopedHostAdapters<'a> {
    pub fn filesystem_path<'resource>(
        &self,
        authorized: &'resource AuthorizedPath,
    ) -> Result<&'resource Path, AuthorityError> {
        self.context.authorize_scope(authorized.scope)?;
        Ok(&authorized.path)
    }

    pub fn network_endpoint<'resource>(
        &self,
        authorized: &'resource AuthorizedEndpoint,
    ) -> Result<&'resource NetworkEndpointGrant, AuthorityError> {
        self.context.authorize_scope(authorized.scope)?;
        Ok(&authorized.endpoint)
    }

    pub fn process_executable<'resource>(
        &self,
        authorized: &'resource AuthorizedExecutable,
    ) -> Result<&'resource Path, AuthorityError> {
        self.context.authorize_scope(authorized.scope)?;
        Ok(&authorized.path)
    }

    pub fn database<'resource>(
        &self,
        authorized: &'resource AuthorizedDatabase,
    ) -> Result<&'resource str, AuthorityError> {
        self.context.authorize_scope(authorized.scope)?;
        Ok(&authorized.logical_id)
    }
}

impl NetworkEndpointGrant {
    pub fn new(
        scheme: impl Into<String>,
        host: impl Into<String>,
        port: u16,
    ) -> Result<Self, AuthorityError> {
        let scheme = normalize_endpoint_component("scheme", scheme.into())?;
        let host = normalize_endpoint_component("host", host.into())?;
        if port == 0 {
            return Err(AuthorityError::InvalidGrant(
                "network endpoint port must be nonzero".to_owned(),
            ));
        }
        Ok(Self { scheme, host, port })
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }
}

/// Explicit host grants for a restricted execution.
///
/// Empty grant sets deny their authority. This type has no ambient fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostCapabilities {
    filesystem_roots: Vec<PathBuf>,
    network_endpoints: BTreeSet<NetworkEndpointGrant>,
    process_executables: BTreeSet<PathBuf>,
    database_ids: BTreeSet<String>,
    environment_variables: BTreeSet<String>,
    allow_temp_directory: bool,
}

impl HostCapabilities {
    pub const fn deny_all() -> Self {
        Self {
            filesystem_roots: Vec::new(),
            network_endpoints: BTreeSet::new(),
            process_executables: BTreeSet::new(),
            database_ids: BTreeSet::new(),
            environment_variables: BTreeSet::new(),
            allow_temp_directory: false,
        }
    }

    pub fn grant_filesystem_root(
        mut self,
        root: impl Into<PathBuf>,
    ) -> Result<Self, AuthorityError> {
        let root = normalize_absolute_path(root.into())?;
        if !self.filesystem_roots.contains(&root) {
            self.filesystem_roots.push(root);
            self.filesystem_roots.sort();
        }
        Ok(self)
    }

    pub fn grant_network_endpoint(mut self, grant: NetworkEndpointGrant) -> Self {
        self.network_endpoints.insert(grant);
        self
    }

    pub fn grant_process_executable(
        mut self,
        executable: impl Into<PathBuf>,
    ) -> Result<Self, AuthorityError> {
        self.process_executables
            .insert(normalize_absolute_path(executable.into())?);
        Ok(self)
    }

    pub fn grant_database(mut self, logical_id: impl Into<String>) -> Result<Self, AuthorityError> {
        self.database_ids
            .insert(normalize_logical_name("database", logical_id.into())?);
        Ok(self)
    }

    pub fn grant_environment_variable(
        mut self,
        name: impl Into<String>,
    ) -> Result<Self, AuthorityError> {
        self.environment_variables
            .insert(normalize_logical_name("environment variable", name.into())?);
        Ok(self)
    }

    pub const fn grant_temp_directory(mut self) -> Self {
        self.allow_temp_directory = true;
        self
    }

    pub fn filesystem_roots(&self) -> &[PathBuf] {
        &self.filesystem_roots
    }

    pub fn network_endpoints(&self) -> impl Iterator<Item = &NetworkEndpointGrant> {
        self.network_endpoints.iter()
    }

    pub fn process_executables(&self) -> impl Iterator<Item = &Path> {
        self.process_executables.iter().map(PathBuf::as_path)
    }

    pub fn database_ids(&self) -> impl Iterator<Item = &str> {
        self.database_ids.iter().map(String::as_str)
    }

    pub fn environment_variables(&self) -> impl Iterator<Item = &str> {
        self.environment_variables.iter().map(String::as_str)
    }

    pub const fn allows_temp_directory(&self) -> bool {
        self.allow_temp_directory
    }
}

impl Default for HostCapabilities {
    fn default() -> Self {
        Self::deny_all()
    }
}

/// Coarse host authority required by an intrinsic.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HostAuthority {
    Filesystem,
    Network,
    Process,
    Database,
    Environment,
    TempDirectory,
    Native,
    Jit,
    Gpu,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostAccess {
    Ambient,
    Restricted(HostCapabilities),
}

/// Mandatory authority domain for one execution.
///
/// The type intentionally has no `Default` implementation. Embedders must
/// explicitly select trusted ambient authority or provide restricted grants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    profile: DeploymentProfile,
    scope: ExecutionScopeId,
    host: HostAccess,
}

impl ExecutionContext {
    /// Constructs the explicit compatibility context for trusted local use.
    pub fn trusted_local() -> Self {
        Self {
            profile: DeploymentProfile::LocalTrusted,
            scope: ExecutionScopeId::fresh(),
            host: HostAccess::Ambient,
        }
    }

    /// Constructs a restricted trusted-CI context.
    ///
    /// Every authority not present in `capabilities` is denied. No operation
    /// may fall back to the process environment, current directory, network,
    /// or another ambient host resource.
    pub fn trusted_ci(capabilities: HostCapabilities) -> Self {
        Self {
            profile: DeploymentProfile::TrustedCi,
            scope: ExecutionScopeId::fresh(),
            host: HostAccess::Restricted(capabilities),
        }
    }

    /// Constructs a context for a deployment profile.
    ///
    /// Only the trusted-CI profile may construct a restricted in-process
    /// context. Third-party package execution is outside the product scope.
    pub fn restricted(
        profile: DeploymentProfile,
        capabilities: HostCapabilities,
    ) -> Result<Self, ExecutionContextError> {
        match profile {
            DeploymentProfile::TrustedCi => Ok(Self::trusted_ci(capabilities)),
            DeploymentProfile::LocalTrusted => {
                Err(ExecutionContextError::ProfileRequiresAmbient { profile })
            }
        }
    }

    pub const fn profile(&self) -> DeploymentProfile {
        self.profile
    }

    pub const fn scope_id(&self) -> ExecutionScopeId {
        self.scope
    }

    pub const fn capabilities(&self) -> Option<&HostCapabilities> {
        match &self.host {
            HostAccess::Ambient => None,
            HostAccess::Restricted(capabilities) => Some(capabilities),
        }
    }

    pub const fn is_ambient(&self) -> bool {
        matches!(self.host, HostAccess::Ambient)
    }

    /// Returns the canonical adapter boundary for capabilities minted by this
    /// execution context.
    pub const fn host_adapters(&self) -> ScopedHostAdapters<'_> {
        ScopedHostAdapters { context: self }
    }

    /// Performs the coarse authorization used before dispatching an intrinsic.
    ///
    /// Resource-specific entry points below must still validate the concrete
    /// path, endpoint, executable, database, or variable.
    pub fn authorize_host_authority(&self, authority: HostAuthority) -> Result<(), AuthorityError> {
        match (&self.host, authority) {
            (HostAccess::Ambient, _) => Ok(()),
            (HostAccess::Restricted(capabilities), HostAuthority::Filesystem)
                if !capabilities.filesystem_roots.is_empty() =>
            {
                Ok(())
            }
            (HostAccess::Restricted(capabilities), HostAuthority::Network)
                if !capabilities.network_endpoints.is_empty() =>
            {
                Ok(())
            }
            (HostAccess::Restricted(capabilities), HostAuthority::Process)
                if !capabilities.process_executables.is_empty() =>
            {
                Ok(())
            }
            (HostAccess::Restricted(capabilities), HostAuthority::Database)
                if !capabilities.database_ids.is_empty() =>
            {
                Ok(())
            }
            (HostAccess::Restricted(capabilities), HostAuthority::Environment)
                if !capabilities.environment_variables.is_empty() =>
            {
                Ok(())
            }
            (HostAccess::Restricted(capabilities), HostAuthority::TempDirectory)
                if capabilities.allow_temp_directory =>
            {
                Ok(())
            }
            (HostAccess::Restricted(_), denied) => Err(AuthorityError::HostAuthorityDenied(denied)),
        }
    }

    /// Authorizes a host path.
    ///
    /// Restricted callers must provide an absolute path under an explicitly
    /// granted root. The returned path is lexically normalized, but consumers
    /// must still use handle-relative/no-follow I/O to prevent symlink races.
    pub fn authorize_filesystem_path(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<AuthorizedPath, AuthorityError> {
        let path = path.as_ref();
        let path = match &self.host {
            HostAccess::Ambient => path.to_path_buf(),
            HostAccess::Restricted(capabilities) => {
                let path = normalize_absolute_path(path.to_path_buf())?;
                if capabilities
                    .filesystem_roots
                    .iter()
                    .any(|root| path.starts_with(root))
                {
                    path
                } else {
                    return Err(AuthorityError::FilesystemDenied(path));
                }
            }
        };
        Ok(AuthorizedPath {
            scope: self.scope,
            path,
        })
    }

    /// Resolves and authorizes a relative path under a selected granted root.
    pub fn authorize_filesystem_relative(
        &self,
        root_index: usize,
        relative: impl AsRef<Path>,
    ) -> Result<AuthorizedPath, AuthorityError> {
        let relative = normalize_relative_path(relative.as_ref())?;
        let path = match &self.host {
            HostAccess::Ambient => relative,
            HostAccess::Restricted(capabilities) => {
                let root = capabilities
                    .filesystem_roots
                    .get(root_index)
                    .ok_or(AuthorityError::FilesystemRootDenied(root_index))?;
                root.join(relative)
            }
        };
        Ok(AuthorizedPath {
            scope: self.scope,
            path,
        })
    }

    pub fn authorize_network_endpoint(
        &self,
        scheme: &str,
        host: &str,
        port: u16,
    ) -> Result<AuthorizedEndpoint, AuthorityError> {
        let endpoint = NetworkEndpointGrant::new(scheme, host, port)?;
        if matches!(self.host, HostAccess::Ambient) {
            return Ok(AuthorizedEndpoint {
                scope: self.scope,
                endpoint,
            });
        }
        match &self.host {
            HostAccess::Ambient => unreachable!(),
            HostAccess::Restricted(capabilities)
                if capabilities.network_endpoints.contains(&endpoint) =>
            {
                Ok(AuthorizedEndpoint {
                    scope: self.scope,
                    endpoint,
                })
            }
            HostAccess::Restricted(_) => Err(AuthorityError::NetworkDenied(endpoint)),
        }
    }

    pub fn authorize_process_executable(
        &self,
        executable: impl AsRef<Path>,
    ) -> Result<AuthorizedExecutable, AuthorityError> {
        let executable = executable.as_ref();
        let path = match &self.host {
            HostAccess::Ambient => executable.to_path_buf(),
            HostAccess::Restricted(capabilities) => {
                let executable = normalize_absolute_path(executable.to_path_buf())?;
                if capabilities.process_executables.contains(&executable) {
                    executable
                } else {
                    return Err(AuthorityError::ProcessDenied(executable));
                }
            }
        };
        Ok(AuthorizedExecutable {
            scope: self.scope,
            path,
        })
    }

    pub fn authorize_database(
        &self,
        logical_id: &str,
    ) -> Result<AuthorizedDatabase, AuthorityError> {
        let logical_id = normalize_logical_name("database", logical_id.to_owned())?;
        if matches!(self.host, HostAccess::Ambient) {
            return Ok(AuthorizedDatabase {
                scope: self.scope,
                logical_id,
            });
        }
        match &self.host {
            HostAccess::Ambient => unreachable!(),
            HostAccess::Restricted(capabilities)
                if capabilities.database_ids.contains(&logical_id) =>
            {
                Ok(AuthorizedDatabase {
                    scope: self.scope,
                    logical_id,
                })
            }
            HostAccess::Restricted(_) => Err(AuthorityError::DatabaseDenied(logical_id)),
        }
    }

    /// Rejects a capability object that belongs to another execution.
    pub fn authorize_scope(&self, scope: ExecutionScopeId) -> Result<(), AuthorityError> {
        if self.scope == scope {
            Ok(())
        } else {
            Err(AuthorityError::ScopeMismatch {
                expected: self.scope,
                actual: scope,
            })
        }
    }

    pub fn authorize_environment_variable(&self, name: &str) -> Result<(), AuthorityError> {
        if matches!(self.host, HostAccess::Ambient) {
            return Ok(());
        }
        let name = normalize_logical_name("environment variable", name.to_owned())?;
        match &self.host {
            HostAccess::Ambient => unreachable!(),
            HostAccess::Restricted(capabilities)
                if capabilities.environment_variables.contains(&name) =>
            {
                Ok(())
            }
            HostAccess::Restricted(_) => Err(AuthorityError::EnvironmentDenied(name)),
        }
    }

    pub fn authorize_temp_directory(&self) -> Result<(), AuthorityError> {
        match &self.host {
            HostAccess::Ambient => Ok(()),
            HostAccess::Restricted(capabilities) if capabilities.allow_temp_directory => Ok(()),
            HostAccess::Restricted(_) => Err(AuthorityError::TempDirectoryDenied),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionContextError {
    ProfileRequiresAmbient { profile: DeploymentProfile },
}

impl fmt::Display for ExecutionContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileRequiresAmbient { profile } => write!(
                formatter,
                "deployment profile `{profile}` is not a restricted execution profile; use ExecutionContext::trusted_local explicitly"
            ),
        }
    }
}

impl std::error::Error for ExecutionContextError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorityError {
    InvalidGrant(String),
    HostAuthorityDenied(HostAuthority),
    FilesystemDenied(PathBuf),
    FilesystemRootDenied(usize),
    NetworkDenied(NetworkEndpointGrant),
    ProcessDenied(PathBuf),
    DatabaseDenied(String),
    EnvironmentDenied(String),
    TempDirectoryDenied,
    ScopeMismatch {
        expected: ExecutionScopeId,
        actual: ExecutionScopeId,
    },
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGrant(message) => {
                write!(formatter, "invalid host authority grant: {message}")
            }
            Self::HostAuthorityDenied(authority) => {
                write!(
                    formatter,
                    "host authority `{authority:?}` is not authorized"
                )
            }
            Self::FilesystemDenied(path) => {
                write!(
                    formatter,
                    "filesystem path `{}` is not authorized",
                    path.display()
                )
            }
            Self::FilesystemRootDenied(index) => {
                write!(formatter, "filesystem root grant {index} does not exist")
            }
            Self::NetworkDenied(endpoint) => write!(
                formatter,
                "network endpoint `{}://{}:{}` is not authorized",
                endpoint.scheme, endpoint.host, endpoint.port
            ),
            Self::ProcessDenied(path) => {
                write!(
                    formatter,
                    "process executable `{}` is not authorized",
                    path.display()
                )
            }
            Self::DatabaseDenied(logical_id) => {
                write!(formatter, "database `{logical_id}` is not authorized")
            }
            Self::EnvironmentDenied(name) => {
                write!(formatter, "environment variable `{name}` is not authorized")
            }
            Self::TempDirectoryDenied => {
                formatter.write_str("ambient temporary-directory access is not authorized")
            }
            Self::ScopeMismatch { expected, actual } => write!(
                formatter,
                "host capability belongs to execution scope {}, not {}",
                actual.get(),
                expected.get()
            ),
        }
    }
}

impl std::error::Error for AuthorityError {}

fn normalize_endpoint_component(
    component: &'static str,
    value: String,
) -> Result<String, AuthorityError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(AuthorityError::InvalidGrant(format!(
            "network endpoint {component} must be nonempty and contain no whitespace"
        )));
    }
    Ok(value)
}

fn normalize_logical_name(kind: &'static str, value: String) -> Result<String, AuthorityError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(AuthorityError::InvalidGrant(format!(
            "{kind} identifier must be nonempty and contain no control characters"
        )));
    }
    Ok(value)
}

fn normalize_absolute_path(path: PathBuf) -> Result<PathBuf, AuthorityError> {
    if !path.is_absolute() {
        return Err(AuthorityError::InvalidGrant(format!(
            "restricted host path `{}` must be absolute",
            path.display()
        )));
    }
    normalize_path_components(&path, true)
}

fn normalize_relative_path(path: &Path) -> Result<PathBuf, AuthorityError> {
    if path.is_absolute() {
        return Err(AuthorityError::InvalidGrant(format!(
            "relative host path `{}` must not be absolute",
            path.display()
        )));
    }
    normalize_path_components(path, false)
}

fn normalize_path_components(path: &Path, absolute: bool) -> Result<PathBuf, AuthorityError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) if absolute => normalized.push(prefix.as_os_str()),
            Component::RootDir if absolute => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(AuthorityError::InvalidGrant(format!(
                    "host path `{}` contains a disallowed component",
                    path.display()
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(AuthorityError::InvalidGrant(
            "host path must not be empty".to_owned(),
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_ci_allows_only_static_and_restricted_reference_vm() {
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
                .is_ok()
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
    fn profile_parser_rejects_ambiguous_names() {
        assert_eq!("trusted-ci".parse(), Ok(DeploymentProfile::TrustedCi));
        assert!("production".parse::<DeploymentProfile>().is_err());
        assert!("untrusted-isolated".parse::<DeploymentProfile>().is_err());
    }

    #[test]
    fn trusted_local_is_explicit_and_ambient() {
        let context = ExecutionContext::trusted_local();
        assert_eq!(context.profile(), DeploymentProfile::LocalTrusted);
        assert!(context.is_ambient());
        assert!(context.authorize_filesystem_path("relative.txt").is_ok());
        assert!(
            context
                .authorize_network_endpoint("tcp", "127.0.0.1", 1)
                .is_ok()
        );
        assert!(context.authorize_process_executable("tool").is_ok());
        assert!(context.authorize_database("raw-dsn").is_ok());
        assert!(context.authorize_environment_variable("SECRET").is_ok());
        assert!(context.authorize_temp_directory().is_ok());
    }

    #[test]
    fn trusted_ci_denies_every_ambient_authority_by_default() {
        let context = ExecutionContext::trusted_ci(HostCapabilities::deny_all());
        assert_eq!(context.profile(), DeploymentProfile::TrustedCi);
        assert!(!context.is_ambient());
        assert!(
            context
                .authorize_filesystem_path(test_absolute_path("denied"))
                .is_err()
        );
        assert!(
            context
                .authorize_network_endpoint("https", "example.com", 443)
                .is_err()
        );
        assert!(
            context
                .authorize_process_executable(test_absolute_path("tool"))
                .is_err()
        );
        assert!(context.authorize_database("primary").is_err());
        assert!(context.authorize_environment_variable("TOKEN").is_err());
        assert!(context.authorize_temp_directory().is_err());
    }

    #[test]
    fn trusted_ci_authorizes_only_exact_grants() {
        let root = test_absolute_path("workspace");
        let executable = test_absolute_path("bin/tool");
        let capabilities = HostCapabilities::deny_all()
            .grant_filesystem_root(&root)
            .expect("root")
            .grant_network_endpoint(
                NetworkEndpointGrant::new("HTTPS", "Example.COM", 443).expect("endpoint"),
            )
            .grant_process_executable(&executable)
            .expect("executable")
            .grant_database("primary")
            .expect("database")
            .grant_environment_variable("CI_TOKEN")
            .expect("environment")
            .grant_temp_directory();
        let context = ExecutionContext::trusted_ci(capabilities);

        let path = context
            .authorize_filesystem_relative(0, "input/source.rss")
            .expect("relative path");
        assert_eq!(path.as_path(), root.join("input/source.rss"));
        assert_eq!(path.scope_id(), context.scope_id());
        assert!(
            context
                .authorize_filesystem_relative(0, "../escape")
                .is_err()
        );
        assert!(
            context
                .authorize_network_endpoint("https", "example.com", 443)
                .is_ok()
        );
        assert!(
            context
                .authorize_network_endpoint("https", "example.com", 444)
                .is_err()
        );
        let authorized_executable = context
            .authorize_process_executable(&executable)
            .expect("process");
        assert_eq!(authorized_executable.as_path(), executable);
        assert_eq!(authorized_executable.scope_id(), context.scope_id());
        assert!(
            context
                .authorize_process_executable(test_absolute_path("bin/other"))
                .is_err()
        );
        assert!(context.authorize_database("primary").is_ok());
        assert!(context.authorize_database("secondary").is_err());
        assert!(context.authorize_environment_variable("CI_TOKEN").is_ok());
        assert!(context.authorize_environment_variable("HOME").is_err());
        assert!(context.authorize_temp_directory().is_ok());
    }

    #[test]
    fn execution_scopes_are_unique() {
        let first_context = ExecutionContext::trusted_local();
        let second_context = ExecutionContext::trusted_ci(HostCapabilities::deny_all());
        let first = first_context
            .authorize_database("primary")
            .expect("ambient database handle");

        assert_ne!(first_context.scope_id(), second_context.scope_id());
        assert!(first_context.authorize_scope(first.scope_id()).is_ok());
        assert!(matches!(
            second_context.authorize_scope(first.scope_id()),
            Err(AuthorityError::ScopeMismatch { .. })
        ));
    }

    #[test]
    fn scoped_host_adapters_reject_capabilities_from_another_execution() {
        let path = test_absolute_path("workspace/input.rss");
        let executable = test_absolute_path("bin/tool");
        let first = ExecutionContext::trusted_local();
        let second = ExecutionContext::trusted_local();

        let authorized_path = first.authorize_filesystem_path(&path).expect("path");
        let authorized_endpoint = first
            .authorize_network_endpoint("https", "example.com", 443)
            .expect("endpoint");
        let authorized_executable = first
            .authorize_process_executable(&executable)
            .expect("executable");
        let authorized_database = first.authorize_database("primary").expect("database");

        assert_eq!(
            first
                .host_adapters()
                .filesystem_path(&authorized_path)
                .expect("same scope"),
            path
        );
        assert!(matches!(
            second.host_adapters().filesystem_path(&authorized_path),
            Err(AuthorityError::ScopeMismatch { .. })
        ));
        assert!(matches!(
            second
                .host_adapters()
                .network_endpoint(&authorized_endpoint),
            Err(AuthorityError::ScopeMismatch { .. })
        ));
        assert!(matches!(
            second
                .host_adapters()
                .process_executable(&authorized_executable),
            Err(AuthorityError::ScopeMismatch { .. })
        ));
        assert!(matches!(
            second.host_adapters().database(&authorized_database),
            Err(AuthorityError::ScopeMismatch { .. })
        ));
    }

    #[cfg(unix)]
    fn test_absolute_path(suffix: &str) -> PathBuf {
        Path::new("/rsscript-test").join(suffix)
    }

    #[cfg(windows)]
    fn test_absolute_path(suffix: &str) -> PathBuf {
        Path::new(r"C:\rsscript-test").join(suffix)
    }
}
