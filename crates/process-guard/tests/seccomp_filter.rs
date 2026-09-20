//! The strict runner seccomp filter, exercised through a dedicated
//! harness-free entry point.
//!
//! This target is declared `harness = false` on purpose. The parent re-invokes
//! *this* binary as the guarded child, and `main` branches on the environment
//! marker as its first act, so nothing — no libtest argument parsing, no test
//! thread pool, no output capture — runs between `execve` and the child-side
//! assertions. A child that still fails therefore implicates the dynamic
//! loader, Rust's runtime startup, or the filter itself, and nothing else.
//!
//! The parent also pipes and reports the child's stdout, stderr, and exit
//! status. The previous unit test asserted only
//! `child.wait()...success()`, and when it failed on GitHub's `ubuntu-24.04`
//! runners the log carried no cause at all: the child, whose stdio was
//! inherited, printed nothing before dying. Capturing the streams and naming
//! the exit code or terminating signal is what makes the next failure
//! diagnosable wherever it happens.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    linux_x86_64::main();
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    // The reference BPF layout is Linux x86-64 only. `rss-process-guard`'s
    // unit tests cover the fail-closed path everywhere else.
    println!("seccomp filter test skipped: not Linux x86-64");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_x86_64 {
    use std::io::{self, Read};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, ExitStatus, Stdio};

    use rss_process_guard::{
        ProcessLimits, StrictIsolationControl, StrictIsolationRequirements,
        spawn_guarded_child_strict_with, verify_strict_child_context_with,
    };

    /// Set by the parent on the guarded child. `main` reads it before any
    /// other work so the child entry stays minimal.
    const CHILD: &str = "RSSCRIPT_SECCOMP_FILTER_CHILD";

    pub fn main() {
        if std::env::var_os(CHILD).is_some() {
            run_child();
            return;
        }
        run_parent();
        println!("test seccomp_filter_is_enforced_or_fails_closed_before_runner_code ... ok");
    }

    fn requirements() -> StrictIsolationRequirements {
        StrictIsolationRequirements::linux_runner().require(StrictIsolationControl::SeccompFilter)
    }

    /// The guarded child: prove the declared controls are actually installed,
    /// then prove the filter's observable deny and allow results.
    fn run_child() {
        verify_strict_child_context_with(requirements())
            .expect("strict child must observe installed seccomp filter");
        // SAFETY: this direct syscall has no pointer arguments. The test
        // checks the filter's observable deny result without creating a
        // socket or interacting with the host network.
        let socket = unsafe { libc::syscall(libc::SYS_socket, libc::AF_INET, 1, 0) };
        assert_eq!(socket, -1, "seccomp must reject socket creation");
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
        // SAFETY: `getpid` takes no arguments, returns a plain integer, and
        // cannot fail, so this FFI call has no memory-safety preconditions.
        let pid = unsafe { libc::getpid() };
        assert!(pid > 0, "ordinary syscalls remain available");
    }

    fn run_parent() {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match spawn_guarded_child_strict_with(
            &mut command,
            ProcessLimits::generated_program(),
            requirements(),
        ) {
            Ok(child) => child,
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
                return;
            }
            Err(error) => panic!("seccomp filter must install or fail closed: {error}"),
        };

        // Drain stderr on its own thread: a child that dies noisily (a panic
        // with a full backtrace, say) can otherwise fill the stderr pipe while
        // the parent is still blocked on stdout.
        let mut child_stdout = child
            .child_mut()
            .stdout
            .take()
            .expect("child stdout must be piped");
        let mut child_stderr = child
            .child_mut()
            .stderr
            .take()
            .expect("child stderr must be piped");
        let stderr_reader = std::thread::spawn(move || {
            let mut stderr = String::new();
            child_stderr
                .read_to_string(&mut stderr)
                .map(|_| stderr)
                .unwrap_or_else(|error| format!("<unreadable: {error}>"))
        });
        let mut stdout = String::new();
        if let Err(error) = child_stdout.read_to_string(&mut stdout) {
            stdout = format!("<unreadable: {error}>");
        }
        let stderr = stderr_reader.join().expect("stderr reader must not panic");

        let status = child.wait().expect("seccomp child should exit");
        assert!(
            status.success(),
            "seccomp child did not succeed: {}\n\
             --- child stdout ---\n{}\n\
             --- child stderr ---\n{}\n\
             --- end child output ---",
            describe(status),
            stdout.trim_end(),
            stderr.trim_end(),
        );
    }

    /// Name the exit status the way a reader needs it: a child killed by a
    /// signal (a seccomp `SIGSYS`, an out-of-memory `SIGKILL`) reports no exit
    /// code at all, which is exactly the case a bare `success()` hid.
    fn describe(status: ExitStatus) -> String {
        match (status.code(), status.signal()) {
            (Some(code), _) => format!("exit code {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => format!("unknown exit status {status}"),
        }
    }
}
