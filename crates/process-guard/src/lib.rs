//! Resource limits for untrusted or resource-intensive child processes.
//!
//! This crate is intentionally small: it contains the Unix `pre_exec` unsafe
//! boundary so the compiler and runtime crates can remain safe Rust.

use std::io;
use std::process::{Child, Command};
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

/// Arrange for `limits` to be installed in the child immediately before exec.
///
/// Windows limits that require a process handle are completed by
/// [`ProcessGuard::attach`] immediately after `Command::spawn`.
pub fn configure(command: &mut Command, limits: ProcessLimits) -> io::Result<()> {
    configure_platform(command, limits)
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
    pub fn attach(child: &Child, limits: ProcessLimits) -> io::Result<Self> {
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
    let value = libc::rlim_t::try_from(value).unwrap_or(libc::RLIM_INFINITY);
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
        configure(
            &mut command,
            ProcessLimits {
                cpu_seconds: 10,
                address_space_bytes: 0,
                open_files: 64,
                file_size_bytes: 0,
            },
        )
        .expect("limits should configure");

        let output = command.output().expect("guarded child should run");
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
            configure(&mut command, limits).expect("limits should configure");
            let mut child = command
                .spawn()
                .unwrap_or_else(|error| panic!("{name} guarded child should spawn: {error}"));
            let _guard =
                ProcessGuard::attach(&child, limits).expect("spawned child should be guarded");
            assert!(child.wait().expect("child wait should succeed").success());
        }
    }

    #[test]
    fn platform_reports_implemented_memory_limit_kind() {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert_eq!(memory_limit_kind(), MemoryLimitKind::AddressSpace);
        #[cfg(target_os = "macos")]
        assert_eq!(memory_limit_kind(), MemoryLimitKind::DataSegmentBestEffort);
    }
}
