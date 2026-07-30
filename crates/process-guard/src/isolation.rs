use crate::{GuardedChild, ProcessLimits};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

const FIXED_BUBBLEWRAP_LAUNCHER: &str = "/usr/bin/bwrap";

/// The launcher used to create an isolated worker.
///
/// Construction only accepts absolute paths. The executable is independently
/// verified immediately before each spawn and must be a root-owned,
/// non-symlink regular file that is not writable by group or other.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerIsolationBackend {
    launcher: PathBuf,
}

impl WorkerIsolationBackend {
    /// Use the distribution bubblewrap location without consulting `PATH`.
    pub fn bubblewrap() -> Self {
        Self {
            launcher: PathBuf::from(FIXED_BUBBLEWRAP_LAUNCHER),
        }
    }

    /// Use an explicitly trusted bubblewrap executable.
    pub fn bubblewrap_at(launcher: impl Into<PathBuf>) -> io::Result<Self> {
        let launcher = launcher.into();
        require_absolute(&launcher, "bubblewrap launcher")?;
        Ok(Self { launcher })
    }

    pub fn launcher(&self) -> &Path {
        &self.launcher
    }
}

/// Complete filesystem and resource policy for one worker.
///
/// The sandbox starts with an empty temporary root. The worker executable and
/// every declared input are mounted read-only at their canonical absolute host
/// paths. No host path is implicitly exposed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerSandbox {
    worker: PathBuf,
    args: Vec<OsString>,
    read_only_inputs: Vec<PathBuf>,
    read_only_system_inputs: Vec<PathBuf>,
    limits: ProcessLimits,
}

impl WorkerSandbox {
    pub fn new(worker: impl Into<PathBuf>, limits: ProcessLimits) -> io::Result<Self> {
        let worker = worker.into();
        require_absolute(&worker, "worker executable")?;
        require_bounded_limits(limits)?;
        Ok(Self {
            worker,
            args: Vec::new(),
            read_only_inputs: Vec::new(),
            read_only_system_inputs: Vec::new(),
            limits,
        })
    }

    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn read_only_input(&mut self, path: impl Into<PathBuf>) -> io::Result<&mut Self> {
        push_absolute(
            &mut self.read_only_inputs,
            path.into(),
            "worker read-only input",
        )?;
        Ok(self)
    }

    pub fn read_only_system_input(&mut self, path: impl Into<PathBuf>) -> io::Result<&mut Self> {
        push_absolute(
            &mut self.read_only_system_inputs,
            path.into(),
            "worker read-only system input",
        )?;
        Ok(self)
    }

    pub const fn limits(&self) -> ProcessLimits {
        self.limits
    }
}

/// Non-forgeable evidence that the verified launcher acknowledged sandbox setup.
///
/// This proof covers launcher verification, namespace/filesystem policy
/// construction, strict process limits, and bubblewrap's setup acknowledgement.
/// The caller must still validate every protocol response and fail closed if the
/// worker does not become ready; readiness never weakens this isolation proof.
#[derive(Debug)]
pub struct WorkerIsolationProof {
    child_pid: u32,
    _private: (),
}

impl WorkerIsolationProof {
    pub const fn child_pid(&self) -> u32 {
        self.child_pid
    }
}

/// Spawn a Linux worker behind the configured bubblewrap boundary.
///
/// macOS, Windows, Android, and other targets fail with
/// [`io::ErrorKind::Unsupported`]; this function never falls back to a weaker
/// process-only guard.
pub fn spawn_isolated_worker(
    backend: &WorkerIsolationBackend,
    sandbox: &WorkerSandbox,
) -> io::Result<(GuardedChild, WorkerIsolationProof)> {
    spawn_isolated_worker_platform(backend, sandbox)
}

fn require_absolute(path: &Path, description: &str) -> io::Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be an absolute path"),
        ))
    }
}

fn push_absolute(paths: &mut Vec<PathBuf>, path: PathBuf, description: &str) -> io::Result<()> {
    require_absolute(&path, description)?;
    paths.push(path);
    Ok(())
}

fn require_bounded_limits(limits: ProcessLimits) -> io::Result<()> {
    let limits = [
        ("CPU", limits.cpu_seconds),
        ("address-space", limits.address_space_bytes),
        ("open-file", limits.open_files),
        ("file-size", limits.file_size_bytes),
    ];
    if let Some((name, _)) = limits.into_iter().find(|(_, value)| *value == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("isolated worker requires a nonzero {name} limit"),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn spawn_isolated_worker_platform(
    backend: &WorkerIsolationBackend,
    sandbox: &WorkerSandbox,
) -> io::Result<(GuardedChild, WorkerIsolationProof)> {
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::time::Duration;

    verify_launcher(&backend.launcher)?;
    let prepared = PreparedSandbox::new(sandbox)?;
    crate::platform_limit_support().require_fully_enforced(prepared.limits)?;

    let (info_reader, info_writer) = pipe_for_launcher_status()?;
    let info_fd = info_writer.as_raw_fd();
    let mut command = build_bubblewrap_command(&backend.launcher, &prepared, info_fd);
    let mut child = crate::spawn_guarded_child_strict(&mut command, prepared.limits)?;
    let child_pid = child.child_mut().id();
    drop(info_writer);

    // SAFETY: ownership of the read descriptor is transferred exactly once.
    let info_reader = unsafe { File::from_raw_fd(info_reader.into_raw_fd()) };
    let acknowledgement =
        read_setup_acknowledgement(info_reader, Duration::from_secs(5)).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("bubblewrap did not acknowledge sandbox setup: {error}"),
            )
        })?;
    validate_setup_acknowledgement(&acknowledgement)?;

    Ok((
        child,
        WorkerIsolationProof {
            child_pid,
            _private: (),
        },
    ))
}

#[cfg(not(target_os = "linux"))]
fn spawn_isolated_worker_platform(
    _backend: &WorkerIsolationBackend,
    _sandbox: &WorkerSandbox,
) -> io::Result<(GuardedChild, WorkerIsolationProof)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "isolated workers require Linux and a verified bubblewrap launcher",
    ))
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct PreparedSandbox {
    worker: PathBuf,
    args: Vec<OsString>,
    read_only_inputs: Vec<PathBuf>,
    read_only_system_inputs: Vec<PathBuf>,
    #[cfg(target_os = "linux")]
    limits: ProcessLimits,
}

#[cfg(target_os = "linux")]
impl PreparedSandbox {
    fn new(sandbox: &WorkerSandbox) -> io::Result<Self> {
        let worker = canonical_file(&sandbox.worker, "worker executable")?;
        let metadata = std::fs::metadata(&worker)?;
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "worker executable has no executable mode bit",
            ));
        }

        Ok(Self {
            worker,
            args: sandbox.args.clone(),
            read_only_inputs: canonical_inputs(&sandbox.read_only_inputs)?,
            read_only_system_inputs: canonical_inputs(&sandbox.read_only_system_inputs)?,
            limits: sandbox.limits,
        })
    }
}

#[cfg(target_os = "linux")]
fn canonical_file(path: &Path, description: &str) -> io::Result<PathBuf> {
    require_absolute(path, description)?;
    let canonical = std::fs::canonicalize(path)?;
    if !std::fs::metadata(&canonical)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be a regular file"),
        ));
    }
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn canonical_inputs(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    paths
        .iter()
        .map(|path| {
            require_absolute(path, "read-only sandbox input")?;
            std::fs::canonicalize(path)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn verify_launcher(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    require_absolute(path, "bubblewrap launcher")?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bubblewrap launcher must be a non-symlink regular file",
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bubblewrap launcher must not be writable by group or other",
        ));
    }
    if metadata.uid() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bubblewrap launcher must be owned by root",
        ));
    }
    if metadata.mode() & 0o111 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bubblewrap launcher must be executable",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn build_bubblewrap_command(
    launcher: &Path,
    sandbox: &PreparedSandbox,
    info_fd: i32,
) -> std::process::Command {
    use std::process::{Command, Stdio};

    let mut command = Command::new(launcher);
    command.env_clear();
    command
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-net",
            "--cap-drop",
            "ALL",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--chdir",
            "/",
            "--tmpfs",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--info-fd",
        ])
        .arg(info_fd.to_string());

    for path in sandbox
        .read_only_system_inputs
        .iter()
        .chain(&sandbox.read_only_inputs)
        .chain(std::iter::once(&sandbox.worker))
    {
        if let Some(parent) = path.parent() {
            command.arg("--dir").arg(parent);
        }
        command.arg("--ro-bind").arg(path).arg(path);
    }
    command
        .arg("--")
        .arg(&sandbox.worker)
        .args(&sandbox.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(target_os = "linux")]
fn pipe_for_launcher_status() -> io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let mut fds = [-1; 2];
    // SAFETY: `fds` points to two initialized integers for the kernel to fill.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful pipe2 returned two uniquely owned descriptors.
    let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: successful pipe2 returned two uniquely owned descriptors.
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    // Bubblewrap must inherit its status descriptor across exec.
    if unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFD, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((reader, writer))
}

#[cfg(target_os = "linux")]
fn read_setup_acknowledgement(
    reader: std::fs::File,
    timeout: std::time::Duration,
) -> io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let mut poll_fd = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    // SAFETY: `poll_fd` is valid for the single-element array described here.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    if ready == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for --info-fd",
        ));
    }
    let mut bytes = Vec::new();
    reader.take(64 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn validate_setup_acknowledgement(bytes: &[u8]) -> io::Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid --info-fd JSON: {error}"),
        )
    })?;
    if value
        .get("child-pid")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "--info-fd acknowledgement omitted child-pid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_launcher_must_be_absolute() {
        let error = WorkerIsolationBackend::bubblewrap_at("bwrap")
            .expect_err("PATH lookup must not be accepted");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn sandbox_paths_must_be_absolute() {
        let error = WorkerSandbox::new("worker", ProcessLimits::generated_program())
            .expect_err("relative worker must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let mut sandbox =
            WorkerSandbox::new("/worker", ProcessLimits::generated_program()).unwrap();
        let error = sandbox
            .read_only_input("input")
            .expect_err("relative input must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn sandbox_requires_finite_limits() {
        let error = WorkerSandbox::new(
            "/worker",
            ProcessLimits {
                cpu_seconds: 0,
                ..ProcessLimits::generated_program()
            },
        )
        .expect_err("unbounded resources must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn bubblewrap_command_has_closed_isolation_policy() {
        let sandbox = PreparedSandbox {
            worker: PathBuf::from("/opt/rss/worker"),
            args: vec![OsString::from("--serve")],
            read_only_inputs: vec![PathBuf::from("/work/input.bin")],
            read_only_system_inputs: vec![PathBuf::from("/usr")],
            #[cfg(target_os = "linux")]
            limits: ProcessLimits::generated_program(),
        };
        let command = build_bubblewrap_command(Path::new("/usr/bin/bwrap"), &sandbox, 42);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        for required in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-net",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--chdir",
            "--new-session",
            "--die-with-parent",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "/tmp",
        ] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        for read_only in ["/usr", "/work/input.bin", "/opt/rss/worker"] {
            assert!(
                args.windows(3)
                    .any(|window| window == ["--ro-bind", read_only, read_only]),
                "{read_only} is not mounted read-only"
            );
        }
        assert!(args.ends_with(&[
            "--".to_owned(),
            "/opt/rss/worker".to_owned(),
            "--serve".to_owned()
        ]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn launcher_rejects_group_or_other_writable_mode() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "rss-process-guard-insecure-launcher-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"not a launcher").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = verify_launcher(&path).expect_err("writable launcher must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("writable by group or other"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn unsupported_platform_fails_closed() {
        let backend = WorkerIsolationBackend::bubblewrap();
        let sandbox = WorkerSandbox::new("/worker", ProcessLimits::generated_program()).unwrap();
        let error = spawn_isolated_worker(&backend, &sandbox)
            .expect_err("non-Linux must not fall back to a weaker guard");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires RSS_PROCESS_GUARD_BWRAP_INTEGRATION=1 and host bubblewrap support"]
    fn bubblewrap_integration() {
        if std::env::var_os("RSS_PROCESS_GUARD_BWRAP_INTEGRATION").is_none() {
            return;
        }
        let mut sandbox =
            WorkerSandbox::new("/usr/bin/true", ProcessLimits::generated_program()).unwrap();
        for path in ["/usr", "/lib", "/lib64"] {
            if Path::new(path).exists() {
                sandbox.read_only_system_input(path).unwrap();
            }
        }
        let (mut child, proof) =
            spawn_isolated_worker(&WorkerIsolationBackend::bubblewrap(), &sandbox).unwrap();
        assert_eq!(proof.child_pid(), child.child_mut().id());
        assert!(child.wait().unwrap().success());
    }
}
