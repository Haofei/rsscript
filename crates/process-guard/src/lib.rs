//! Resource limits for untrusted or resource-intensive child processes.
//!
//! This crate is intentionally small: it contains the Unix `pre_exec` unsafe
//! boundary so the compiler and runtime crates can remain safe Rust.

use std::io;
use std::process::{Child, Command, ExitStatus};
#[cfg(windows)]
use std::{fs::File, path::Path};

#[cfg(windows)]
pub fn same_file_identity(path: &Path, open_file: &File) -> io::Result<bool> {
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        GetFileInformationByHandle,
    };

    let path_file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;

    fn identity(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { info.assume_init() })
    }

    let left = identity(&path_file)?;
    let right = identity(open_file)?;
    Ok(left.dwVolumeSerialNumber == right.dwVolumeSerialNumber
        && left.nFileIndexHigh == right.nFileIndexHigh
        && left.nFileIndexLow == right.nFileIndexLow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLimits {
    pub cpu_seconds: u64,
    pub address_space_bytes: u64,
    pub open_files: u64,
    pub file_size_bytes: u64,
}

impl ProcessLimits {
    pub const fn compiler_worker() -> Self {
        Self {
            cpu_seconds: 600,
            address_space_bytes: 8 * 1024 * 1024 * 1024,
            open_files: 1024,
            file_size_bytes: 512 * 1024 * 1024,
        }
    }

    pub const fn generated_program() -> Self {
        Self {
            cpu_seconds: 600,
            address_space_bytes: 2 * 1024 * 1024 * 1024,
            open_files: 256,
            file_size_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLimitKind {
    /// `RLIMIT_AS` constrains the child virtual address space.
    AddressSpace,
    /// Darwin's `RLIMIT_DATA` constrains the data segment, but not every mmap.
    DataSegmentBestEffort,
    /// A Windows Job Object constrains each process and the aggregate job tree.
    WindowsJob,
}

pub const fn memory_limit_kind() -> MemoryLimitKind {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        return MemoryLimitKind::AddressSpace;
    }
    #[cfg(target_os = "macos")]
    {
        return MemoryLimitKind::DataSegmentBestEffort;
    }
    #[cfg(windows)]
    {
        return MemoryLimitKind::WindowsJob;
    }
    #[allow(unreachable_code)]
    MemoryLimitKind::DataSegmentBestEffort
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSupport {
    Enforced,
    BestEffort,
    Unsupported,
}

/// A kernel control which can be required by a strict runner profile.
///
/// This is deliberately a small, host-side contract rather than a language
/// capability model.  A profile may only require a control that the launcher
/// can install before the child starts executing runner code.  Controls that
/// do not yet have an adapter are reported as unsupported and fail closed;
/// they are never silently treated as advisory hardening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictIsolationControl {
    /// Linux's `PR_SET_NO_NEW_PRIVS` privilege-transition guard.
    NoNewPrivileges,
    /// A private Linux user namespace.
    UserNamespace,
    /// A private Linux mount namespace.
    MountNamespace,
    /// A private Linux network namespace.
    NetworkNamespace,
    /// A restrictive Linux seccomp filter.
    SeccompFilter,
    /// A dedicated cgroup-v2 boundary.
    CgroupV2,
}

impl StrictIsolationControl {
    const fn name(self) -> &'static str {
        match self {
            Self::NoNewPrivileges => "no_new_privs",
            Self::UserNamespace => "user namespace",
            Self::MountNamespace => "mount namespace",
            Self::NetworkNamespace => "network namespace",
            Self::SeccompFilter => "seccomp filter",
            Self::CgroupV2 => "cgroup v2",
        }
    }
}

/// The explicitly declared kernel controls for one strict child launch.
///
/// `linux_runner` is the current reference-runner baseline.  It establishes
/// `no_new_privs` before execution on Linux/Android.  The remaining controls
/// are represented here now so callers can require them without accidentally
/// receiving a weaker child while their adapters are being implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictIsolationRequirements {
    no_new_privileges: bool,
    user_namespace: bool,
    mount_namespace: bool,
    network_namespace: bool,
    seccomp_filter: bool,
    cgroup_v2: bool,
}

impl StrictIsolationRequirements {
    pub const fn none() -> Self {
        Self {
            no_new_privileges: false,
            user_namespace: false,
            mount_namespace: false,
            network_namespace: false,
            seccomp_filter: false,
            cgroup_v2: false,
        }
    }

    /// The controls installed by the current strict reference runner.
    pub const fn linux_runner() -> Self {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            return Self::none().require(StrictIsolationControl::NoNewPrivileges);
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        Self::none()
    }

    /// Require one control.  The launch is rejected if it cannot be enforced.
    pub const fn require(mut self, control: StrictIsolationControl) -> Self {
        match control {
            StrictIsolationControl::NoNewPrivileges => self.no_new_privileges = true,
            StrictIsolationControl::UserNamespace => self.user_namespace = true,
            StrictIsolationControl::MountNamespace => self.mount_namespace = true,
            StrictIsolationControl::NetworkNamespace => self.network_namespace = true,
            StrictIsolationControl::SeccompFilter => self.seccomp_filter = true,
            StrictIsolationControl::CgroupV2 => self.cgroup_v2 = true,
        }
        self
    }

    pub const fn requires(self, control: StrictIsolationControl) -> bool {
        match control {
            StrictIsolationControl::NoNewPrivileges => self.no_new_privileges,
            StrictIsolationControl::UserNamespace => self.user_namespace,
            StrictIsolationControl::MountNamespace => self.mount_namespace,
            StrictIsolationControl::NetworkNamespace => self.network_namespace,
            StrictIsolationControl::SeccompFilter => self.seccomp_filter,
            StrictIsolationControl::CgroupV2 => self.cgroup_v2,
        }
    }

    fn require_fully_enforced(self) -> io::Result<()> {
        for control in [
            StrictIsolationControl::NoNewPrivileges,
            StrictIsolationControl::UserNamespace,
            StrictIsolationControl::MountNamespace,
            StrictIsolationControl::NetworkNamespace,
            StrictIsolationControl::SeccompFilter,
            StrictIsolationControl::CgroupV2,
        ] {
            if self.requires(control) && strict_isolation_support(control) != LimitSupport::Enforced
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "required strict isolation control `{}` is unavailable on this platform",
                        control.name()
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Report whether a strict isolation control is actually enforced by this
/// crate.  This is intentionally about implementation support, not a claim
/// that a particular host policy is sufficient for untrusted code.
pub const fn strict_isolation_support(control: StrictIsolationControl) -> LimitSupport {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        return match control {
            StrictIsolationControl::NoNewPrivileges => LimitSupport::Enforced,
            StrictIsolationControl::UserNamespace
            | StrictIsolationControl::MountNamespace
            | StrictIsolationControl::NetworkNamespace
            | StrictIsolationControl::SeccompFilter
            | StrictIsolationControl::CgroupV2 => LimitSupport::Unsupported,
        };
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = control;
        LimitSupport::Unsupported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedProcessLimits {
    pub cpu: LimitSupport,
    pub address_space: LimitSupport,
    pub open_files: LimitSupport,
    pub file_size: LimitSupport,
    /// Whether the process is constrained before any user code can execute.
    pub atomic_process_tree_containment: LimitSupport,
}

impl AppliedProcessLimits {
    fn validate_requested(self, limits: ProcessLimits) -> io::Result<()> {
        let requested = [
            ("CPU", limits.cpu_seconds, self.cpu),
            (
                "address-space",
                limits.address_space_bytes,
                self.address_space,
            ),
            ("open-file", limits.open_files, self.open_files),
            ("file-size", limits.file_size_bytes, self.file_size),
        ];
        for (name, value, support) in requested {
            if value > 0 && support == LimitSupport::Unsupported {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("required {name} process limit is unsupported on this platform"),
                ));
            }
        }
        if self.atomic_process_tree_containment == LimitSupport::Unsupported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic process-tree containment is unsupported on this platform",
            ));
        }
        Ok(())
    }

    pub fn require_fully_enforced(self, limits: ProcessLimits) -> io::Result<()> {
        self.validate_requested(limits)?;
        let requested = [
            ("CPU", limits.cpu_seconds, self.cpu),
            (
                "address-space",
                limits.address_space_bytes,
                self.address_space,
            ),
            ("open-file", limits.open_files, self.open_files),
            ("file-size", limits.file_size_bytes, self.file_size),
        ];
        for (name, value, support) in requested {
            if value > 0 && support != LimitSupport::Enforced {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("required {name} process limit is not fully enforced on this platform"),
                ));
            }
        }
        if self.atomic_process_tree_containment != LimitSupport::Enforced {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic process-tree containment is not fully enforced on this platform",
            ));
        }
        Ok(())
    }
}

pub const fn platform_limit_support() -> AppliedProcessLimits {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        return AppliedProcessLimits {
            cpu: LimitSupport::Enforced,
            address_space: LimitSupport::Enforced,
            open_files: LimitSupport::Enforced,
            file_size: LimitSupport::Enforced,
            atomic_process_tree_containment: LimitSupport::Enforced,
        };
    }
    #[cfg(target_os = "macos")]
    {
        return AppliedProcessLimits {
            cpu: LimitSupport::Enforced,
            address_space: LimitSupport::BestEffort,
            open_files: LimitSupport::Enforced,
            file_size: LimitSupport::Enforced,
            atomic_process_tree_containment: LimitSupport::Enforced,
        };
    }
    #[cfg(windows)]
    {
        return AppliedProcessLimits {
            cpu: LimitSupport::Unsupported,
            address_space: LimitSupport::Enforced,
            open_files: LimitSupport::Unsupported,
            file_size: LimitSupport::Unsupported,
            // std::process cannot create suspended and assign a Job Object
            // before user code starts. Refuse strict guarded execution.
            atomic_process_tree_containment: LimitSupport::Unsupported,
        };
    }
    #[allow(unreachable_code)]
    AppliedProcessLimits {
        cpu: LimitSupport::Unsupported,
        address_space: LimitSupport::Unsupported,
        open_files: LimitSupport::Unsupported,
        file_size: LimitSupport::Unsupported,
        atomic_process_tree_containment: LimitSupport::Unsupported,
    }
}

fn configure(command: &mut Command, limits: ProcessLimits) -> io::Result<AppliedProcessLimits> {
    let applied = platform_limit_support();
    applied.validate_requested(limits)?;
    configure_platform(command, limits).map(|()| applied)
}

/// Configure, spawn, and attach the platform process-tree guard as one
/// fail-closed operation.
///
/// If post-spawn attachment fails, the child is killed and reaped before the
/// error is returned. Callers therefore never receive an unguarded child.
pub fn spawn_guarded(
    command: &mut Command,
    limits: ProcessLimits,
) -> io::Result<(Child, ProcessGuard)> {
    spawn_guarded_with(command, limits, ProcessGuard::attach)
        .map(|(child, guard, _)| (child, guard))
}

/// Spawn a child whose root process and descendants have one RAII owner.
///
/// Dropping the returned value terminates the process tree and reaps the root
/// child. Normal completion also terminates descendants before returning.
pub fn spawn_guarded_child(
    command: &mut Command,
    limits: ProcessLimits,
) -> io::Result<GuardedChild> {
    let (child, guard, applied_limits) = spawn_guarded_with(command, limits, ProcessGuard::attach)?;
    Ok(GuardedChild {
        child: Some(child),
        guard,
        applied_limits,
        finished: false,
        tree_terminated: false,
    })
}

/// Strict variant that rejects both unsupported and best-effort limits.
pub fn spawn_guarded_child_strict(
    command: &mut Command,
    limits: ProcessLimits,
) -> io::Result<GuardedChild> {
    spawn_guarded_child_strict_with(command, limits, StrictIsolationRequirements::linux_runner())
}

/// Strictly spawn a child with the exact kernel controls declared by the
/// caller.  Every requested control must be enforced before the child begins
/// executing; unavailable adapters are an error rather than best effort.
pub fn spawn_guarded_child_strict_with(
    command: &mut Command,
    limits: ProcessLimits,
    requirements: StrictIsolationRequirements,
) -> io::Result<GuardedChild> {
    platform_limit_support().require_fully_enforced(limits)?;
    requirements.require_fully_enforced()?;
    configure_strict_platform(command, requirements)?;
    spawn_guarded_child(command, limits)
}

/// Verify that a Linux/Android child is running with the strict privilege
/// transition guard installed by [`spawn_guarded_child_strict`].
///
/// The parent-side `pre_exec` call is necessary but not sufficient evidence
/// for a runner entrypoint: this query lets the child fail closed before it
/// parses an Artifact when it was launched outside the guarded path. Other
/// platforms return success because the strict launcher does not claim an
/// equivalent kernel control there.
pub fn verify_strict_child_context() -> io::Result<()> {
    verify_strict_child_context_with(StrictIsolationRequirements::linux_runner())
}

/// Verify the strict controls expected by a child-side runner entrypoint.
///
/// The check mirrors [`spawn_guarded_child_strict_with`] and deliberately
/// rejects an entrypoint launched without the required control.  Future
/// controls are checked here as their adapters become available.
pub fn verify_strict_child_context_with(
    requirements: StrictIsolationRequirements,
) -> io::Result<()> {
    requirements.require_fully_enforced()?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if !requirements.requires(StrictIsolationControl::NoNewPrivileges) {
            return Ok(());
        }
        let status = std::fs::read_to_string("/proc/self/status")?;
        if parse_no_new_privileges(&status) == Some(true) {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "strict child context requires Linux no_new_privs=1",
        ));
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn parse_no_new_privileges(status: &str) -> Option<bool> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("NoNewPrivs:")?.trim();
        match value {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }
    })
}

/// Add strict-only process controls before the ordinary resource-limit setup.
///
/// Linux `no_new_privs` is deliberately attached in the child's `pre_exec`
/// sequence. A successful parent-side spawn therefore means the runner starts
/// with the kernel restriction already set; failure aborts `Command::spawn`.
/// Other platforms retain the existing strict resource/process-tree checks but
/// do not claim an equivalent privilege-transition control.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn configure_strict_platform(
    command: &mut Command,
    requirements: StrictIsolationRequirements,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    if !requirements.requires(StrictIsolationControl::NoNewPrivileges) {
        return Ok(());
    }

    // SAFETY: the closure invokes only `prctl` with integer arguments and
    // obtains no locks or heap-backed state after fork. Any kernel failure is
    // returned to `Command::spawn`, so a strict caller never receives a child
    // that silently missed this control.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn configure_strict_platform(
    _command: &mut Command,
    _requirements: StrictIsolationRequirements,
) -> io::Result<()> {
    Ok(())
}

fn spawn_guarded_with(
    command: &mut Command,
    limits: ProcessLimits,
    attach: impl FnOnce(&Child, ProcessLimits) -> io::Result<ProcessGuard>,
) -> io::Result<(Child, ProcessGuard, AppliedProcessLimits)> {
    let applied_limits = configure(command, limits)?;
    let mut child = command.spawn()?;
    match attach(&child, limits) {
        Ok(guard) => Ok((child, guard, applied_limits)),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

#[derive(Debug)]
pub struct GuardedChild {
    child: Option<Child>,
    guard: ProcessGuard,
    applied_limits: AppliedProcessLimits,
    finished: bool,
    tree_terminated: bool,
}

impl GuardedChild {
    pub fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("guarded child is still owned")
    }

    pub fn applied_limits(&self) -> AppliedProcessLimits {
        self.applied_limits
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child_mut().try_wait()
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        let tree_result = if self.tree_terminated {
            Ok(())
        } else {
            self.guard.terminate()
        };
        if tree_result.is_ok() {
            self.tree_terminated = true;
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
        tree_result
    }

    pub fn wait(mut self) -> io::Result<ExitStatus> {
        let status = self.child_mut().wait()?;
        // The root may exit after starting background descendants. The process
        // group/job remains the ownership boundary until all descendants are
        // explicitly terminated.
        let tree_result = if self.tree_terminated {
            Ok(())
        } else {
            self.guard.terminate()
        };
        if tree_result.is_ok() {
            self.tree_terminated = true;
        }
        self.finished = true;
        self.child.take();
        tree_result?;
        Ok(status)
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if !self.tree_terminated {
            let _ = self.guard.terminate();
            self.tree_terminated = true;
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.finished = true;
    }
}

/// Owns the platform process-tree boundary for a spawned child.
///
/// Keep this value alive until the child has exited. On Windows, dropping it
/// closes a kill-on-close Job Object, so descendants cannot outlive the owner.
#[derive(Debug)]
pub struct ProcessGuard {
    platform: PlatformGuard,
}

impl ProcessGuard {
    fn attach(child: &Child, limits: ProcessLimits) -> io::Result<Self> {
        Ok(Self {
            platform: attach_platform(child, limits)?,
        })
    }

    pub fn terminate(&self) -> io::Result<()> {
        terminate_platform(&self.platform)
    }
}

/// Terminate the Unix process group created for a bounded child.
///
/// New cross-platform callers should retain a [`ProcessGuard`] and call
/// [`ProcessGuard::terminate`]. This function remains for Unix compatibility.
pub fn terminate_process_group(pid: u32) -> io::Result<()> {
    terminate_process_group_platform(pid)
}

#[cfg(unix)]
#[derive(Debug)]
struct PlatformGuard {
    pid: u32,
}

#[cfg(unix)]
fn attach_platform(child: &Child, _limits: ProcessLimits) -> io::Result<PlatformGuard> {
    Ok(PlatformGuard { pid: child.id() })
}

#[cfg(unix)]
fn terminate_platform(guard: &PlatformGuard) -> io::Result<()> {
    terminate_process_group_platform(guard.pid)
}

#[cfg(unix)]
fn configure_platform(command: &mut Command, limits: ProcessLimits) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    // SAFETY: the pre-exec closure only invokes async-signal-safe setrlimit
    // syscalls and constructs no heap-backed state after fork. Values are copied
    // into the closure. Any failure is returned to Command::spawn.
    unsafe {
        command.pre_exec(move || {
            if limits.cpu_seconds > 0 {
                set_limit(libc::RLIMIT_CPU, limits.cpu_seconds)?;
            }
            set_address_space_limit(limits.address_space_bytes)?;
            if limits.open_files > 0 {
                set_limit(libc::RLIMIT_NOFILE, limits.open_files)?;
            }
            if limits.file_size_bytes > 0 {
                set_limit(libc::RLIMIT_FSIZE, limits.file_size_bytes)?;
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn set_address_space_limit(value: u64) -> io::Result<()> {
    if value > 0 {
        set_limit(libc::RLIMIT_AS, value)?;
    }
    Ok(())
}

// Darwin rejects RLIMIT_AS. RLIMIT_DATA is a verifiable best-effort ceiling for
// malloc-backed data, but does not constrain every mmap or provide Linux-equivalent
// address-space isolation.
#[cfg(target_os = "macos")]
fn set_address_space_limit(value: u64) -> io::Result<()> {
    if value == 0 {
        return Ok(());
    }
    match set_limit(libc::RLIMIT_DATA, value) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOTSUP | libc::EPERM)
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn set_address_space_limit(_value: u64) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
#[cfg(any(target_os = "linux", target_os = "android"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn set_limit(resource: RlimitResource, value: u64) -> io::Result<()> {
    let value = libc::rlim_t::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "resource limit exceeds the platform representation",
        )
    })?;
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` is initialized and its pointer remains valid for the
    // duration of the syscall.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn terminate_process_group_platform(pid: u32) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "child pid exceeds i32"))?;
    // SAFETY: a negative pid addresses the process group created by
    // CommandExt::process_group(0); no memory is dereferenced.
    if unsafe { libc::kill(-pid, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::{ProcessLimits, io};
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

    #[derive(Debug)]
    pub(super) struct PlatformGuard {
        job: HANDLE,
    }

    // HANDLE ownership is unique and Windows kernel handles may be transferred
    // between threads.
    unsafe impl Send for PlatformGuard {}
    unsafe impl Sync for PlatformGuard {}

    impl Drop for PlatformGuard {
        fn drop(&mut self) {
            // SAFETY: `job` is owned by this value and closed exactly once.
            unsafe {
                CloseHandle(self.job);
            }
        }
    }

    pub(super) fn configure(command: &mut Command, _limits: ProcessLimits) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        Ok(())
    }

    pub(super) fn attach(child: &Child, limits: ProcessLimits) -> io::Result<PlatformGuard> {
        // SAFETY: null security attributes and name request a private Job Object.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let guard = PlatformGuard { job };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if limits.address_space_bytes > 0 {
            let memory = usize::try_from(limits.address_space_bytes).unwrap_or(usize::MAX);
            info.BasicLimitInformation.LimitFlags |=
                JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.ProcessMemoryLimit = memory;
            info.JobMemoryLimit = memory;
        }
        // SAFETY: the information pointer and byte size match the requested
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION class.
        if unsafe {
            SetInformationJobObject(
                guard.job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .expect("job information size fits u32"),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let process = child.as_raw_handle() as HANDLE;
        // SAFETY: both handles are live for this call. The returned guard keeps
        // the Job Object alive for the child lifetime.
        if unsafe { AssignProcessToJobObject(guard.job, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(guard)
    }

    pub(super) fn terminate(guard: &PlatformGuard) -> io::Result<()> {
        // SAFETY: `job` remains live while `guard` is borrowed.
        if unsafe { TerminateJobObject(guard.job, 1) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(windows)]
use windows::PlatformGuard;

#[cfg(windows)]
fn configure_platform(command: &mut Command, limits: ProcessLimits) -> io::Result<()> {
    windows::configure(command, limits)
}

#[cfg(windows)]
fn attach_platform(child: &Child, limits: ProcessLimits) -> io::Result<PlatformGuard> {
    windows::attach(child, limits)
}

#[cfg(windows)]
fn terminate_platform(guard: &PlatformGuard) -> io::Result<()> {
    windows::terminate(guard)
}

#[cfg(not(unix))]
fn terminate_process_group_platform(_pid: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "retain ProcessGuard and call terminate() on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
#[derive(Debug)]
struct PlatformGuard;

#[cfg(not(any(unix, windows)))]
fn configure_platform(_command: &mut Command, _limits: ProcessLimits) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "kernel process resource limits are unavailable on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn attach_platform(_child: &Child, _limits: ProcessLimits) -> io::Result<PlatformGuard> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "kernel process resource limits are unavailable on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn terminate_platform(_guard: &PlatformGuard) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process-tree termination is unavailable on this platform",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn guarded_child_observes_open_file_limit() {
        let mut command = Command::new("sh");
        command.args(["-c", "ulimit -n"]);
        command.stdout(std::process::Stdio::piped());
        let (child, _guard) = spawn_guarded(
            &mut command,
            ProcessLimits {
                cpu_seconds: 10,
                address_space_bytes: 0,
                open_files: 64,
                file_size_bytes: 0,
            },
        )
        .expect("guarded child should run");
        let output = child
            .wait_with_output()
            .expect("guarded child output should be collected");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "64");
    }

    #[test]
    fn production_limit_profiles_can_spawn_a_child() {
        for (name, limits) in [
            (
                "cpu",
                ProcessLimits {
                    cpu_seconds: 600,
                    address_space_bytes: 0,
                    open_files: 0,
                    file_size_bytes: 0,
                },
            ),
            (
                "address-space",
                ProcessLimits {
                    cpu_seconds: 0,
                    address_space_bytes: 8 * 1024 * 1024 * 1024,
                    open_files: 0,
                    file_size_bytes: 0,
                },
            ),
            (
                "file-size",
                ProcessLimits {
                    cpu_seconds: 0,
                    address_space_bytes: 0,
                    open_files: 0,
                    file_size_bytes: 512 * 1024 * 1024,
                },
            ),
            ("compiler", ProcessLimits::compiler_worker()),
            ("program", ProcessLimits::generated_program()),
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            let (mut child, _guard) = spawn_guarded(&mut command, limits)
                .unwrap_or_else(|error| panic!("{name} guarded child should spawn: {error}"));
            assert!(child.wait().expect("child wait should succeed").success());
        }
    }

    #[test]
    fn attach_failure_kills_and_reaps_the_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let mut child_pid = None;
        let result = spawn_guarded_with(
            &mut command,
            ProcessLimits::generated_program(),
            |child, _| {
                child_pid = Some(child.id());
                Err(io::Error::other("injected attach failure"))
            },
        );
        assert!(result.is_err());

        let pid = i32::try_from(child_pid.expect("child should have spawned")).expect("pid fits");
        // SAFETY: signal zero only probes for a live process and dereferences no memory.
        let status = unsafe { libc::kill(pid, 0) };
        assert_eq!(status, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn platform_reports_implemented_memory_limit_kind() {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert_eq!(memory_limit_kind(), MemoryLimitKind::AddressSpace);
        #[cfg(target_os = "macos")]
        assert_eq!(memory_limit_kind(), MemoryLimitKind::DataSegmentBestEffort);
    }

    #[test]
    fn guarded_child_terminates_descendants_after_root_exit() {
        let marker = std::env::temp_dir().join(format!(
            "rss-process-guard-descendant-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("(sleep 1; touch '{}') & exit 0", marker.display()),
        ]);
        let child = spawn_guarded_child(&mut command, ProcessLimits::generated_program())
            .expect("guarded child should spawn");
        assert!(child.wait().expect("root should finish").success());
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert!(!marker.exists(), "background descendant escaped the guard");
    }

    #[test]
    fn applied_limits_are_queryable() {
        let applied = platform_limit_support();
        assert_eq!(applied.cpu, LimitSupport::Enforced);
        assert_eq!(applied.open_files, LimitSupport::Enforced);
        assert_eq!(applied.file_size, LimitSupport::Enforced);
    }

    #[test]
    fn strict_limits_reject_best_effort_memory_boundaries() {
        let applied = platform_limit_support();
        let result = applied.require_fully_enforced(ProcessLimits {
            cpu_seconds: 0,
            address_space_bytes: 1024,
            open_files: 0,
            file_size_bytes: 0,
        });
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert!(result.is_ok());
        #[cfg(target_os = "macos")]
        assert!(result.is_err());
    }

    #[test]
    fn unavailable_strict_control_fails_before_child_spawn() {
        let mut command = if cfg!(windows) {
            Command::new("cmd")
        } else {
            Command::new("sh")
        };
        if cfg!(windows) {
            command.args(["/C", "exit 0"]);
        } else {
            command.args(["-c", "exit 0"]);
        }
        let requirements =
            StrictIsolationRequirements::none().require(StrictIsolationControl::NetworkNamespace);
        let error = spawn_guarded_child_strict_with(
            &mut command,
            ProcessLimits {
                cpu_seconds: 0,
                address_space_bytes: 0,
                open_files: 0,
                file_size_bytes: 0,
            },
            requirements,
        )
        .expect_err("unimplemented strict control must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("network namespace"));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn strict_child_starts_with_no_new_privileges() {
        use std::io::Read;
        use std::process::Stdio;

        let mut command = Command::new("sh");
        command
            .args(["-c", "grep '^NoNewPrivs:' /proc/self/status"])
            .stdout(Stdio::piped());
        let mut child =
            spawn_guarded_child_strict(&mut command, ProcessLimits::generated_program())
                .expect("strict guarded child should spawn");
        let mut stdout = String::new();
        child
            .child_mut()
            .stdout
            .take()
            .expect("stdout must be piped")
            .read_to_string(&mut stdout)
            .expect("read child status");
        assert!(child.wait().expect("child should exit").success());
        assert_eq!(stdout.trim(), "NoNewPrivs:\t1");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn linux_runner_contract_requires_and_verifies_no_new_privileges() {
        let requirements = StrictIsolationRequirements::linux_runner();
        assert!(requirements.requires(StrictIsolationControl::NoNewPrivileges));
        assert_eq!(
            strict_isolation_support(StrictIsolationControl::NoNewPrivileges),
            LimitSupport::Enforced
        );
        assert_eq!(
            strict_isolation_support(StrictIsolationControl::MountNamespace),
            LimitSupport::Unsupported
        );
        let status = std::fs::read_to_string("/proc/self/status").expect("read process status");
        let result = verify_strict_child_context_with(requirements);
        assert_eq!(
            result.is_ok(),
            parse_no_new_privileges(&status) == Some(true)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn strict_context_parser_accepts_only_the_kernel_enabled_value() {
        assert_eq!(
            parse_no_new_privileges("Name:\trss\nNoNewPrivs:\t1\n"),
            Some(true)
        );
        assert_eq!(parse_no_new_privileges("NoNewPrivs:\t0\n"), Some(false));
        assert_eq!(parse_no_new_privileges("NoNewPrivs:\t2\n"), None);
        assert_eq!(parse_no_new_privileges("Name:\trss\n"), None);
    }
}
