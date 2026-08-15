//! Resource limits for untrusted or resource-intensive child processes.
//!
//! This crate is intentionally small: it contains the Unix `pre_exec` unsafe
//! boundary so the compiler and runtime crates can remain safe Rust.

use std::io;
use std::process::{Child, Command, ExitStatus};
#[cfg(target_os = "linux")]
use std::{
    ffi::{CStr, CString},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::{fs::File, os::fd::AsRawFd, path::Path};
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
            StrictIsolationControl::NoNewPrivileges
            | StrictIsolationControl::UserNamespace
            | StrictIsolationControl::MountNamespace
            | StrictIsolationControl::NetworkNamespace => LimitSupport::Enforced,
            StrictIsolationControl::SeccompFilter => {
                #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
                {
                    LimitSupport::Enforced
                }
                #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
                {
                    LimitSupport::Unsupported
                }
            }
            StrictIsolationControl::CgroupV2 => {
                #[cfg(target_os = "linux")]
                {
                    LimitSupport::Enforced
                }
                #[cfg(not(target_os = "linux"))]
                {
                    LimitSupport::Unsupported
                }
            }
        };
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = control;
        LimitSupport::Unsupported
    }
}

/// Access granted beneath a host-selected filesystem root by the Linux
/// Landlock adapter. This is an execution-host control; it is not a language
/// capability or a request-supplied policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemRootAccess {
    ReadOnly,
    ReadWrite,
}

/// A parent-created cgroup-v2 directory that the guarded child enters before
/// `exec`. The directory is deliberately selected by the host process, never
/// by runner protocol input or script code.
#[derive(Debug)]
pub struct CgroupV2Boundary {
    #[cfg(target_os = "linux")]
    directory: PathBuf,
    #[cfg(target_os = "linux")]
    procs_path: CString,
}

impl CgroupV2Boundary {
    /// Create a unique child cgroup beneath the current process's delegated
    /// cgroup-v2 directory. A read-only hierarchy, missing delegation, or a
    /// v1-only host is an error; strict callers must not fall back to the
    /// parent's ambient cgroup.
    #[cfg(target_os = "linux")]
    pub fn prepare_for_current_process() -> io::Result<Self> {
        let mount = Path::new("/sys/fs/cgroup");
        if !mount.join("cgroup.controllers").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cgroup v2 is not mounted at /sys/fs/cgroup",
            ));
        }
        let membership = std::fs::read_to_string("/proc/self/cgroup")?;
        let relative = membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "current process is not in a cgroup-v2 hierarchy",
                )
            })?;
        let parent = mount.join(relative.trim_start_matches('/'));
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "delegated cgroup-v2 parent does not exist: {}",
                    parent.display()
                ),
            ));
        }

        static NEXT_BOUNDARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..32 {
            let sequence = NEXT_BOUNDARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory =
                parent.join(format!("rsscript-runner-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&directory) {
                Ok(()) => {
                    let procs_path = directory.join("cgroup.procs");
                    let procs_path =
                        CString::new(procs_path.as_os_str().as_bytes()).map_err(|_| {
                            let _ = std::fs::remove_dir(&directory);
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "cgroup-v2 path contains an interior NUL byte",
                            )
                        })?;
                    return Ok(Self {
                        directory,
                        procs_path,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::EROFS | libc::EPERM | libc::EACCES)
                    ) =>
                {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!("cgroup-v2 delegation is unavailable: {error}"),
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique cgroup-v2 runner boundary",
        ))
    }

    /// Non-Linux platforms cannot claim the Linux cgroup-v2 boundary.
    #[cfg(not(target_os = "linux"))]
    pub fn prepare_for_current_process() -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cgroup-v2 runner boundaries are available only on Linux",
        ))
    }

    #[cfg(target_os = "linux")]
    fn procs_path(&self) -> &CStr {
        &self.procs_path
    }
}

#[cfg(target_os = "linux")]
impl Drop for CgroupV2Boundary {
    fn drop(&mut self) {
        // ProcessGuard terminates and reaps the owned tree before this boundary
        // is dropped. A failed cleanup remains host evidence, but must not
        // panic while unwinding a runner failure.
        let _ = std::fs::remove_dir(&self.directory);
    }
}

/// Install a fail-closed Landlock allowlist for the current process.
///
/// This is intentionally called after the runner executable and its dynamic
/// libraries are loaded but before the Artifact is decoded. Only `root` is
/// granted filesystem access; inherited stdin/stdout/stderr descriptors remain
/// usable. Linux Landlock ABI v5 is required so rename/link, truncate, and
/// device-ioctl operations are handled as well as ordinary reads and writes.
/// Unsupported kernels, disabled LSMs, or invalid host roots return an error.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn restrict_current_process_to_root(
    root: &Path,
    access: FilesystemRootAccess,
) -> io::Result<()> {
    const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
    const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
    const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
    const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
    const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
    const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
    const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
    const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
    const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
    const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
    const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
    const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
    const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
    const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
    const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
    const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;
    const LANDLOCK_ACCESS_FS_IOCTL_DEV: u64 = 1 << 15;
    const HANDLED: u64 = LANDLOCK_ACCESS_FS_EXECUTE
        | LANDLOCK_ACCESS_FS_WRITE_FILE
        | LANDLOCK_ACCESS_FS_READ_FILE
        | LANDLOCK_ACCESS_FS_READ_DIR
        | LANDLOCK_ACCESS_FS_REMOVE_DIR
        | LANDLOCK_ACCESS_FS_REMOVE_FILE
        | LANDLOCK_ACCESS_FS_MAKE_CHAR
        | LANDLOCK_ACCESS_FS_MAKE_DIR
        | LANDLOCK_ACCESS_FS_MAKE_REG
        | LANDLOCK_ACCESS_FS_MAKE_SOCK
        | LANDLOCK_ACCESS_FS_MAKE_FIFO
        | LANDLOCK_ACCESS_FS_MAKE_BLOCK
        | LANDLOCK_ACCESS_FS_MAKE_SYM
        | LANDLOCK_ACCESS_FS_REFER
        | LANDLOCK_ACCESS_FS_TRUNCATE
        | LANDLOCK_ACCESS_FS_IOCTL_DEV;
    let allowed = match access {
        FilesystemRootAccess::ReadOnly => {
            LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR
        }
        FilesystemRootAccess::ReadWrite => HANDLED,
    };

    let abi = landlock_create_ruleset_version()?;
    if abi < 5 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("Landlock ABI v5 is required for complete filesystem mediation, found v{abi}"),
        ));
    }
    let no_new_privileges = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
    if no_new_privileges != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Landlock filesystem restriction requires no_new_privs=1",
        ));
    }

    let root = File::open(root)?;
    let ruleset = LandlockRulesetAttr {
        handled_access_fs: HANDLED,
        handled_access_net: 0,
        scoped: 0,
    };
    let ruleset_fd = landlock_syscall(
        libc::SYS_landlock_create_ruleset,
        (&raw const ruleset).cast::<libc::c_void>() as usize,
        std::mem::size_of::<LandlockRulesetAttr>(),
        0,
        0,
    )?;
    let rule = LandlockPathBeneathAttr {
        allowed_access: allowed,
        parent_fd: root.as_raw_fd(),
        reserved: 0,
    };
    let add_result = landlock_syscall(
        libc::SYS_landlock_add_rule,
        ruleset_fd as usize,
        LANDLOCK_RULE_PATH_BENEATH as usize,
        (&raw const rule).cast::<libc::c_void>() as usize,
        0,
    );
    if let Err(error) = add_result {
        let _ = close_raw_fd(ruleset_fd);
        return Err(error);
    }
    let restrict_result = landlock_syscall(
        libc::SYS_landlock_restrict_self,
        ruleset_fd as usize,
        0,
        0,
        0,
    );
    let close_result = close_raw_fd(ruleset_fd);
    restrict_result.and(close_result)
}

/// Non-Linux hosts do not silently claim the Linux filesystem boundary.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn restrict_current_process_to_root(
    _root: &std::path::Path,
    _access: FilesystemRootAccess,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Landlock filesystem restriction is available only on Linux/Android",
    ))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: libc::c_int,
    reserved: u32,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn landlock_create_ruleset_version() -> io::Result<i64> {
    landlock_syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1, 0)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn landlock_syscall(
    number: libc::c_long,
    first: usize,
    second: usize,
    third: usize,
    fourth: usize,
) -> io::Result<i64> {
    // SAFETY: each caller supplies the exact kernel UAPI argument layout and
    // retains every pointed-to value for the duration of this syscall.
    let result = unsafe { libc::syscall(number, first, second, third, fourth) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn close_raw_fd(descriptor: i64) -> io::Result<()> {
    let descriptor = libc::c_int::try_from(descriptor)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Landlock fd out of range"))?;
    // SAFETY: the descriptor is returned by landlock_create_ruleset and closed
    // exactly once by its owning call path.
    if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Install the runner's narrow seccomp filter for the current Linux process.
///
/// The filter rejects ambient network socket creation/use and process-control
/// syscalls while leaving ordinary runtime, allocator, and dynamic-loader
/// syscalls available. It is intentionally a defence-in-depth deny-list, not
/// a claim of complete syscall or container isolation. The caller must select
/// it explicitly and treat an unsupported kernel as a hard error.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn install_current_process_runner_seccomp_filter() -> io::Result<()> {
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;
    const SECCOMP_MODE_FILTER: libc::c_ulong = 1;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_DATA_NR_OFFSET: u32 = 0;
    const DENY_ERRNO: u32 = SECCOMP_RET_ERRNO | (libc::EPERM as u32);

    const fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> SeccompFilter {
        SeccompFilter { code, jt, jf, k }
    }
    const fn deny(syscall: libc::c_long) -> [SeccompFilter; 2] {
        [
            instruction(BPF_JMP_JEQ_K, 0, 1, syscall as u32),
            instruction(BPF_RET_K, 0, 0, DENY_ERRNO),
        ]
    }

    // Keep this list deliberately small and auditable. The pre-exec installer
    // must leave `execve` and the dynamic loader's ordinary syscalls available
    // so the runner can start; the filter instead removes socket entry points
    // and kernel interfaces that would widen process authority after launch.
    const FILTER: [SeccompFilter; 40] = [
        instruction(BPF_LD_W_ABS, 0, 0, SECCOMP_DATA_NR_OFFSET),
        deny(libc::SYS_socket)[0],
        deny(libc::SYS_socket)[1],
        deny(libc::SYS_socketpair)[0],
        deny(libc::SYS_socketpair)[1],
        deny(libc::SYS_connect)[0],
        deny(libc::SYS_connect)[1],
        deny(libc::SYS_bind)[0],
        deny(libc::SYS_bind)[1],
        deny(libc::SYS_listen)[0],
        deny(libc::SYS_listen)[1],
        deny(libc::SYS_accept)[0],
        deny(libc::SYS_accept)[1],
        deny(libc::SYS_accept4)[0],
        deny(libc::SYS_accept4)[1],
        deny(libc::SYS_sendto)[0],
        deny(libc::SYS_sendto)[1],
        deny(libc::SYS_sendmsg)[0],
        deny(libc::SYS_sendmsg)[1],
        deny(libc::SYS_recvfrom)[0],
        deny(libc::SYS_recvfrom)[1],
        deny(libc::SYS_recvmsg)[0],
        deny(libc::SYS_recvmsg)[1],
        deny(libc::SYS_shutdown)[0],
        deny(libc::SYS_shutdown)[1],
        deny(libc::SYS_ptrace)[0],
        deny(libc::SYS_ptrace)[1],
        deny(libc::SYS_bpf)[0],
        deny(libc::SYS_bpf)[1],
        deny(libc::SYS_perf_event_open)[0],
        deny(libc::SYS_perf_event_open)[1],
        deny(libc::SYS_kexec_load)[0],
        deny(libc::SYS_kexec_load)[1],
        deny(libc::SYS_init_module)[0],
        deny(libc::SYS_init_module)[1],
        deny(libc::SYS_finit_module)[0],
        deny(libc::SYS_finit_module)[1],
        deny(libc::SYS_delete_module)[0],
        deny(libc::SYS_delete_module)[1],
        instruction(BPF_RET_K, 0, 0, 0x7fff_0000),
    ];

    let no_new_privileges = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
    if no_new_privileges != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "seccomp runner filter requires no_new_privs=1",
        ));
    }
    let program = SeccompProgram {
        length: u16::try_from(FILTER.len()).expect("static filter length fits u16"),
        filter: FILTER.as_ptr(),
    };
    // SAFETY: `program` points to the fixed, bounded BPF program above for the
    // duration of the syscall. The kernel validates every instruction before
    // installing this irreversible filter.
    if unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            (&raw const program).cast::<libc::c_void>(),
            0,
            0,
        )
    } == 0
    {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("runner seccomp filter is unavailable: {error}"),
            )),
            _ => Err(error),
        }
    }
}

/// The reference BPF layout is currently implemented only for Linux x86-64.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn install_current_process_runner_seccomp_filter() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "runner seccomp filter is available only on Linux x86-64",
    ))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct SeccompFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[repr(C)]
struct SeccompProgram {
    length: u16,
    filter: *const SeccompFilter,
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
        cgroup: None,
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
    spawn_guarded_child_strict_with_cgroup(command, limits, requirements, None)
}

/// Strictly spawn a child with the declared kernel controls and an optional
/// parent-owned cgroup-v2 boundary. A cgroup requirement without a prepared
/// boundary is rejected before `Command::spawn`; the child enters the supplied
/// boundary in `pre_exec`, before runner code can parse input.
pub fn spawn_guarded_child_strict_with_cgroup(
    command: &mut Command,
    limits: ProcessLimits,
    requirements: StrictIsolationRequirements,
    cgroup: Option<CgroupV2Boundary>,
) -> io::Result<GuardedChild> {
    platform_limit_support().require_fully_enforced(limits)?;
    requirements.require_fully_enforced()?;
    if requirements.requires(StrictIsolationControl::CgroupV2) != cgroup.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "strict cgroup-v2 requirement must match a prepared boundary",
        ));
    }
    configure_strict_platform(command, requirements, cgroup.as_ref())?;
    let mut child = spawn_guarded_child(command, limits)?;
    child.cgroup = cgroup;
    Ok(child)
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
        let checks_no_new_privileges = requirements
            .requires(StrictIsolationControl::NoNewPrivileges)
            || requirements.requires(StrictIsolationControl::SeccompFilter);
        if !checks_no_new_privileges {
            return Ok(());
        }
        let status = std::fs::read_to_string("/proc/self/status")?;
        if parse_no_new_privileges(&status) != Some(true) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "strict child context requires Linux no_new_privs=1",
            ));
        }
        if requirements.requires(StrictIsolationControl::SeccompFilter)
            && parse_seccomp_filter(&status) != Some(true)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "strict child context requires an installed seccomp filter",
            ));
        }
        Ok(())
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn parse_seccomp_filter(status: &str) -> Option<bool> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("Seccomp:")?.trim();
        match value {
            "0" | "1" => Some(false),
            "2" => Some(true),
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
    cgroup: Option<&CgroupV2Boundary>,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    if !requirements.requires(StrictIsolationControl::NoNewPrivileges)
        && !requirements.requires(StrictIsolationControl::UserNamespace)
        && !requirements.requires(StrictIsolationControl::MountNamespace)
        && !requirements.requires(StrictIsolationControl::NetworkNamespace)
        && !requirements.requires(StrictIsolationControl::SeccompFilter)
        && !requirements.requires(StrictIsolationControl::CgroupV2)
    {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    let cgroup_procs_path = cgroup.map(|boundary| boundary.procs_path().to_owned());
    #[cfg(target_os = "android")]
    let _ = cgroup;

    // SAFETY: the closure invokes only raw kernel syscalls and direct procfs
    // descriptor writes, obtains no locks or heap-backed state after fork, and
    // returns every failure to `Command::spawn`. A strict caller therefore
    // never receives a child that silently missed a declared control.
    unsafe {
        command.pre_exec(move || {
            configure_linux_namespaces(requirements)?;
            if (requirements.requires(StrictIsolationControl::NoNewPrivileges)
                || requirements.requires(StrictIsolationControl::SeccompFilter))
                && libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if requirements.requires(StrictIsolationControl::SeccompFilter) {
                install_current_process_runner_seccomp_filter()?;
            }
            #[cfg(target_os = "linux")]
            if let Some(procs_path) = &cgroup_procs_path {
                attach_current_process_to_cgroup(procs_path)?;
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn configure_linux_namespaces(requirements: StrictIsolationRequirements) -> io::Result<()> {
    let user = requirements.requires(StrictIsolationControl::UserNamespace);
    let mount = requirements.requires(StrictIsolationControl::MountNamespace);
    let network = requirements.requires(StrictIsolationControl::NetworkNamespace);
    if !user && !mount && !network {
        return Ok(());
    }

    let mut flags = 0;
    if user {
        flags |= libc::CLONE_NEWUSER;
    }
    if mount {
        flags |= libc::CLONE_NEWNS;
    }
    // SAFETY: `flags` contains only Linux namespace flags and no pointers are
    // passed to the kernel.
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if user {
        write_current_identity_maps()?;
    }
    if mount {
        make_mount_propagation_private()?;
    }
    if network {
        // Enter this namespace only after user-namespace mapping. That gives
        // the child the namespace-scoped capability required by Linux instead
        // of relying on ambient host privileges. A disabled user namespace or
        // network namespace is returned to `Command::spawn` as a hard error.
        // SAFETY: no pointers are passed and the flag names only a new network
        // namespace for the current pre-exec child.
        if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Install a single-entry uid/gid map for the current process after entering a
/// user namespace.  This deliberately uses raw descriptors because it runs in
/// `pre_exec`; standard-library file APIs could acquire locks after fork.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn write_current_identity_maps() -> io::Result<()> {
    // SAFETY: the calls have no pointer arguments and return the caller's
    // numeric identity, which remains valid for this pre-exec sequence.
    let uid = unsafe { libc::getuid() };
    // SAFETY: see `getuid` above.
    let gid = unsafe { libc::getgid() };

    // Linux requires this write before an unprivileged process can create a
    // gid map. Some kernels do not expose the file; in that case the gid-map
    // write below is the authoritative fail-closed check.
    match write_procfs_file(b"/proc/self/setgroups\0", b"deny\n") {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
        Err(error) => return Err(error),
    }
    write_identity_map(b"/proc/self/uid_map\0", uid as u64)?;
    write_identity_map(b"/proc/self/gid_map\0", gid as u64)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn write_identity_map(path: &[u8], outside_id: u64) -> io::Result<()> {
    let mut line = [0_u8; 32];
    let mut offset = 0;
    line[offset] = b'0';
    offset += 1;
    line[offset] = b' ';
    offset += 1;
    offset += append_decimal(&mut line[offset..], outside_id);
    line[offset] = b' ';
    offset += 1;
    line[offset] = b'1';
    offset += 1;
    line[offset] = b'\n';
    offset += 1;
    write_procfs_file(path, &line[..offset])
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn append_decimal(output: &mut [u8], mut value: u64) -> usize {
    let mut reversed = [0_u8; 20];
    let mut length = 0;
    loop {
        reversed[length] = b'0' + (value % 10) as u8;
        length += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in 0..length {
        output[index] = reversed[length - index - 1];
    }
    length
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn write_procfs_file(path: &[u8], bytes: &[u8]) -> io::Result<()> {
    debug_assert_eq!(path.last(), Some(&0));
    // SAFETY: `path` is NUL-terminated and the flags do not require a mode
    // argument. The descriptor is owned by this function until `close`.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr().cast::<libc::c_char>(),
            libc::O_WRONLY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut offset = 0;
    let result = loop {
        // SAFETY: `offset` is bounded by `bytes.len()` and the pointer remains
        // valid for the syscall. `write` does not retain it.
        let written = unsafe {
            libc::write(
                descriptor,
                bytes[offset..].as_ptr().cast::<libc::c_void>(),
                bytes.len() - offset,
            )
        };
        if written < 0 {
            break Err(io::Error::last_os_error());
        }
        let written = written as usize;
        if written == 0 {
            break Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short procfs namespace-map write",
            ));
        }
        offset += written;
        if offset == bytes.len() {
            break Ok(());
        }
    };
    // SAFETY: `descriptor` was returned by `open` above and has not been
    // closed on any previous path.
    let close_result = if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    };
    result.and(close_result)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn make_mount_propagation_private() -> io::Result<()> {
    // SAFETY: the constant C string names the mount root. Null source/type/data
    // pointers are valid for this propagation-only mount operation.
    let result = unsafe {
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Attach the pre-exec child to its parent-created cgroup before it can run
/// runner code. The caller passes a preallocated C path, so this path performs
/// no allocation or locking after `fork`.
#[cfg(target_os = "linux")]
fn attach_current_process_to_cgroup(procs_path: &CStr) -> io::Result<()> {
    // SAFETY: `procs_path` is a NUL-terminated path created by the parent
    // before fork; the raw descriptor is owned by this function.
    let descriptor = unsafe { libc::open(procs_path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut line = [0_u8; 32];
    // SAFETY: getpid has no pointer arguments and the child identity is stable
    // through this pre-exec sequence.
    let length = append_decimal(&mut line, unsafe { libc::getpid() } as u64);
    line[length] = b'\n';
    let bytes = &line[..length + 1];
    // SAFETY: `bytes` is initialized and remains valid for this write. cgroup
    // `cgroup.procs` accepts one process ID atomically.
    let written = unsafe { libc::write(descriptor, bytes.as_ptr().cast(), bytes.len()) };
    let write_result = if written == bytes.len() as isize {
        Ok(())
    } else if written < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short cgroup-v2 process attachment write",
        ))
    };
    // SAFETY: `descriptor` was opened above and is closed exactly once here.
    let close_result = if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    };
    write_result.and(close_result)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn configure_strict_platform(
    _command: &mut Command,
    _requirements: StrictIsolationRequirements,
    _cgroup: Option<&CgroupV2Boundary>,
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
    cgroup: Option<CgroupV2Boundary>,
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

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
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
            StrictIsolationRequirements::none().require(StrictIsolationControl::SeccompFilter);
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
        .expect_err("unsupported strict control must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("seccomp filter"));
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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn seccomp_filter_is_enforced_or_fails_closed_before_runner_code() {
        const CHILD: &str = "RSSCRIPT_SECCOMP_FILTER_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let requirements = StrictIsolationRequirements::linux_runner()
                .require(StrictIsolationControl::SeccompFilter);
            verify_strict_child_context_with(requirements)
                .expect("strict child must observe installed seccomp filter");
            // SAFETY: this direct syscall has no pointer arguments. The test
            // checks the filter's observable deny result without creating a
            // socket or interacting with the host network.
            let socket = unsafe { libc::syscall(libc::SYS_socket, libc::AF_INET, 1, 0) };
            assert_eq!(socket, -1, "seccomp must reject socket creation");
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert!(
                unsafe { libc::getpid() } > 0,
                "ordinary syscalls remain available"
            );
            return;
        }

        let requirements = StrictIsolationRequirements::linux_runner()
            .require(StrictIsolationControl::SeccompFilter);
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "tests::seccomp_filter_is_enforced_or_fails_closed_before_runner_code",
            ])
            .env(CHILD, "1");
        match spawn_guarded_child_strict_with(
            &mut command,
            ProcessLimits::generated_program(),
            requirements,
        ) {
            Ok(child) => {
                assert!(child.wait().expect("seccomp child should exit").success());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported
                        | io::ErrorKind::PermissionDenied
                        // User-mode Linux emulators can reject `prctl` before
                        // the kernel reaches the filter verifier. That is an
                        // unavailable boundary, not permission to continue.
                        | io::ErrorKind::InvalidInput
                ) =>
            {
                eprintln!("seccomp unavailable or denied: {error}");
            }
            Err(error) => panic!("seccomp filter must install or fail closed: {error}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_boundary_is_attached_before_runner_code_or_fails_closed() {
        const CHILD: &str = "RSSCRIPT_CGROUP_V2_CHILD";
        const EXPECTED: &str = "RSSCRIPT_CGROUP_V2_EXPECTED";
        if std::env::var_os(CHILD).is_some() {
            let expected = std::env::var(EXPECTED).expect("expected cgroup directory name");
            let membership =
                std::fs::read_to_string("/proc/self/cgroup").expect("child cgroup membership");
            assert!(
                membership
                    .lines()
                    .any(|line| line.starts_with("0::") && line.ends_with(&expected)),
                "child must enter the parent-created cgroup before test code: {membership}"
            );
            return;
        }

        let boundary = match CgroupV2Boundary::prepare_for_current_process() {
            Ok(boundary) => boundary,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
                ) =>
            {
                eprintln!("cgroup-v2 delegation unavailable or denied: {error}");
                return;
            }
            Err(error) => panic!("cgroup-v2 preparation must fail closed: {error}"),
        };
        let directory = boundary.directory.clone();
        let expected = directory
            .file_name()
            .and_then(|name| name.to_str())
            .expect("ASCII cgroup directory name")
            .to_string();
        let requirements =
            StrictIsolationRequirements::linux_runner().require(StrictIsolationControl::CgroupV2);
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "tests::cgroup_v2_boundary_is_attached_before_runner_code_or_fails_closed",
            ])
            .env(CHILD, "1")
            .env(EXPECTED, expected);
        let child = spawn_guarded_child_strict_with_cgroup(
            &mut command,
            ProcessLimits::generated_program(),
            requirements,
            Some(boundary),
        )
        .expect("delegated cgroup child must spawn");
        assert!(child.wait().expect("cgroup child should exit").success());
        assert!(
            !directory.exists(),
            "cgroup boundary must be cleaned after the guarded tree exits"
        );
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
            LimitSupport::Enforced
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
    fn user_mount_and_network_namespace_adapter_is_enforced_or_fails_closed() {
        use std::io::Read;
        use std::process::Stdio;

        let requirements = StrictIsolationRequirements::linux_runner()
            .require(StrictIsolationControl::UserNamespace)
            .require(StrictIsolationControl::MountNamespace)
            .require(StrictIsolationControl::NetworkNamespace);
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "grep '^NoNewPrivs:' /proc/self/status; cat /proc/self/uid_map; ls -1 /sys/class/net",
            ])
            .stdout(Stdio::piped());
        match spawn_guarded_child_strict_with(
            &mut command,
            ProcessLimits::generated_program(),
            requirements,
        ) {
            Ok(mut child) => {
                let mut stdout = String::new();
                child
                    .child_mut()
                    .stdout
                    .take()
                    .expect("stdout must be piped")
                    .read_to_string(&mut stdout)
                    .expect("read child status");
                assert!(child.wait().expect("child should exit").success());
                let mut lines = stdout.lines();
                assert_eq!(lines.next(), Some("NoNewPrivs:\t1"));
                assert!(
                    lines
                        .next()
                        .is_some_and(|line| line.starts_with("         0")),
                    "user namespace map must expose an inside uid 0 entry: {stdout:?}"
                );
                assert_eq!(
                    lines.collect::<Vec<_>>(),
                    vec!["lo"],
                    "a new network namespace must not retain host interfaces: {stdout:?}"
                );
            }
            Err(error) => {
                // Many hardened Linux hosts disable unprivileged user
                // namespaces. The strict launcher must reject that kernel
                // policy before exec rather than run a weakened child.
                assert!(
                    matches!(
                        error.kind(),
                        io::ErrorKind::PermissionDenied
                            | io::ErrorKind::Unsupported
                            | io::ErrorKind::Other
                    ),
                    "namespace setup failed with an unexpected error: {error}"
                );
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn decimal_namespace_map_encoder_is_canonical() {
        let mut bytes = [0; 20];
        let length = append_decimal(&mut bytes, 4_294_967_295);
        assert_eq!(&bytes[..length], b"4294967295");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn rooted_filesystem_adapter_is_enforced_or_fails_closed() {
        const ROOT: &str = "RSSCRIPT_LANDLOCK_ROOT";
        const OUTSIDE: &str = "RSSCRIPT_LANDLOCK_OUTSIDE";
        if let (Some(root), Some(outside)) = (std::env::var_os(ROOT), std::env::var_os(OUTSIDE)) {
            // SAFETY: the helper is a fresh test process and installs the same
            // irreversible kernel prerequisite that the strict runner launcher
            // applies before its child begins execution.
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
                0
            );
            match restrict_current_process_to_root(Path::new(&root), FilesystemRootAccess::ReadOnly)
            {
                Ok(()) => {
                    assert!(File::open(Path::new(&root).join("allowed.txt")).is_ok());
                    assert!(File::open(outside).is_err());
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                    eprintln!("Landlock unavailable: {error}");
                    std::process::exit(77);
                }
                Err(error) => panic!("Landlock restriction must install or fail closed: {error}"),
            }
        }

        let root = tempfile::tempdir().expect("root tempdir");
        let outside = tempfile::NamedTempFile::new().expect("outside temp file");
        std::fs::write(root.path().join("allowed.txt"), "allowed").expect("root fixture");
        std::fs::write(outside.path(), "outside").expect("outside fixture");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::rooted_filesystem_adapter_is_enforced_or_fails_closed",
            ])
            .env(ROOT, root.path())
            .env(OUTSIDE, outside.path())
            .output()
            .expect("run isolated Landlock test child");
        match output.status.code() {
            Some(0) => {}
            // The helper reaches this code only after mapping a documented
            // unsupported adapter result to the reserved fail-closed exit.
            Some(77) => {}
            _ => panic!(
                "Landlock child failed: status={}, stdout={}, stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        }
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
