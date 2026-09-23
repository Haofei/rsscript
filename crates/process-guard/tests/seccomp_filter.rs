//! The strict runner seccomp filter, exercised through a dedicated
//! harness-free entry point.
//!
//! This target is declared `harness = false` on purpose. The parent re-invokes
//! *this* binary as the guarded child, and `main` writes one stderr line and
//! then branches on the environment marker, so nothing — no libtest argument
//! parsing, no test thread pool, no output capture — runs between `execve` and
//! the child-side assertions. A child that still fails therefore implicates
//! the dynamic loader, Rust's runtime startup, or the filter itself, and
//! nothing else.
//!
//! The parent pipes and reports the child's stdout, stderr, and exit status.
//! On GitHub's `ubuntu-24.04` x86-64 runners the child was reported as
//! `killed by signal 9` with both streams empty. The cause was the installer:
//! it passed `1` as the `prctl` mode, which is `SECCOMP_MODE_STRICT`, not
//! `SECCOMP_MODE_FILTER` (2). Strict mode ignores the program and SIGKILLs
//! the process on its next syscall that is not `read`, `write`, `exit` or
//! `sigreturn`, which was the next `setrlimit` in `pre_exec`, before `execve`.
//! Inside a container that already runs under a seccomp filter, the kernel
//! refuses strict mode with `EINVAL` instead, and the test took its
//! "unavailable" skip. That is why no container ever reproduced the kill.
//!
//! A bare signal number cannot say whether the kill landed before `execve`
//! completed or after `main` started, nor which control caused it. So the
//! test keeps the evidence that found this cause:
//!
//! - the child's first act is to write `seccomp child: started pid=…` to
//!   stderr, and it writes one more line after each child-side step, so the
//!   last line the parent received names the step that did not finish;
//! - when the child dies by a signal, the parent relaunches the same binary
//!   with the same marker under a bisecting set of controls, and prints every
//!   outcome together with the host facts that bear on it. The controls are:
//!   the rlimits and `no_new_privs` without the filter; the rlimits alone
//!   through the guard; the rlimits alone through a bare `pre_exec`; the
//!   filter and `no_new_privs` without the rlimits; and no controls at all.
//!
//! The test still fails when the kill happens. The controls are evidence for
//! the failure report, never a reason to pass.

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn main() {
    linux::main();
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
fn main() {
    // The reference BPF layout covers Linux x86-64 and AArch64 only.
    // `rss-process-guard`'s unit tests cover the fail-closed path everywhere
    // else.
    println!("seccomp filter test skipped: not Linux x86-64 or AArch64");
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linux {
    use std::io::{self, Read, Write};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    use rss_process_guard::{
        GuardedChild, ProcessLimits, StrictIsolationControl, StrictIsolationRequirements,
        spawn_guarded_child, spawn_guarded_child_strict_with, verify_strict_child_context_with,
    };

    /// Set by the parent on the guarded child. `main` reads it right after the
    /// `started` line so the child entry stays minimal.
    const CHILD: &str = "RSSCRIPT_SECCOMP_FILTER_CHILD";
    /// Set only on a control relaunch whose controls do not include the
    /// filter. That child proves it started and exits: the strict-context and
    /// socket assertions are meaningless without the filter.
    const PROBE: &str = "RSSCRIPT_SECCOMP_FILTER_PROBE";
    /// The line every child writes before anything else.
    const STARTED: &str = "seccomp child: started pid=";

    pub fn main() {
        // The first act, before reading the marker: a line on stderr proves
        // that `execve`, the dynamic loader, and Rust's runtime startup all
        // completed. The parent prints it too, which is harmless.
        child_line(&format!("{STARTED}{}", std::process::id()));
        if std::env::var_os(CHILD).is_some() {
            if std::env::var_os(PROBE).is_some() {
                child_line("seccomp child: probe run, exiting before the filter checks");
                return;
            }
            run_child();
            return;
        }
        run_parent();
        println!("test seccomp_filter_is_enforced_or_fails_closed_before_runner_code ... ok");
    }

    /// Write one line to stderr unbuffered, so a line the child wrote is in
    /// the pipe even if a signal kills it on the next instruction.
    fn child_line(line: &str) {
        let _ = writeln!(io::stderr().lock(), "{line}");
    }

    fn requirements() -> StrictIsolationRequirements {
        StrictIsolationRequirements::linux_runner().require(StrictIsolationControl::SeccompFilter)
    }

    /// The guarded child: prove the declared controls are actually installed,
    /// then prove the filter's observable deny and allow results.
    fn run_child() {
        verify_strict_child_context_with(requirements())
            .expect("strict child must observe installed seccomp filter");
        child_line("seccomp child: strict context verified (no_new_privs=1, seccomp=2)");
        // SAFETY: this direct syscall has no pointer arguments. The test
        // checks the filter's observable deny result without creating a
        // socket or interacting with the host network.
        let socket = unsafe { libc::syscall(libc::SYS_socket, libc::AF_INET, 1, 0) };
        let error = io::Error::last_os_error().raw_os_error();
        assert_eq!(socket, -1, "seccomp must reject socket creation");
        assert_eq!(error, Some(libc::EPERM));
        child_line("seccomp child: socket(2) denied with EPERM");
        // SAFETY: `getpid` takes no arguments, returns a plain integer, and
        // cannot fail, so this FFI call has no memory-safety preconditions.
        let pid = unsafe { libc::getpid() };
        assert!(pid > 0, "ordinary syscalls remain available");
        child_line("seccomp child: done");
    }

    /// A command that re-invokes this binary as a child, with piped output.
    fn child_command(probe: bool) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if probe {
            command.env(PROBE, "1");
        }
        command
    }

    fn run_parent() {
        let started = Instant::now();
        let mut command = child_command(false);
        let child = match spawn_guarded_child_strict_with(
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
        let outcome = Outcome::from_guarded(child, started);
        if outcome.status.success() {
            return;
        }

        let mut report = format!(
            "seccomp child did not succeed: {}\n\
             --- child stdout ---\n{}\n\
             --- child stderr ---\n{}\n\
             --- end child output ---\n",
            outcome.summary(),
            outcome.stdout.trim_end(),
            outcome.stderr.trim_end(),
        );
        if outcome.status.signal().is_some() {
            report.push_str(&host_facts());
            report.push_str(&bisect_controls());
        }
        panic!("{report}");
    }

    /// Relaunch the same binary with the same marker under each subset of the
    /// strict launch's controls, so one failing run names the control that
    /// kills the child.
    fn bisect_controls() -> String {
        let program = ProcessLimits::generated_program();
        let no_limits = ProcessLimits {
            cpu_seconds: 0,
            address_space_bytes: 0,
            open_files: 0,
            file_size_bytes: 0,
        };
        let mut lines = String::from(
            "--- control relaunches (same binary, same marker; `probe` children exit right after `started`) ---\n",
        );
        let controls: [(&str, Launch); 5] = [
            (
                "generated_program rlimits + no_new_privs, no seccomp (probe)",
                Launch::Strict(program, StrictIsolationRequirements::linux_runner()),
            ),
            (
                "generated_program rlimits through the guard, no no_new_privs, no seccomp (probe)",
                Launch::Guarded(program),
            ),
            (
                "generated_program rlimits through a bare pre_exec, no process group (probe)",
                Launch::BareRlimits(program),
            ),
            (
                "seccomp + no_new_privs, no rlimits (full child checks)",
                Launch::Strict(no_limits, requirements()),
            ),
            ("no controls at all (probe)", Launch::Plain),
        ];
        for (label, launch) in controls {
            let line = match launch.run() {
                Ok(outcome) => {
                    let stderr = outcome.stderr.lines().collect::<Vec<_>>().join(" | ");
                    format!("{}; stderr: [{stderr}]", outcome.summary())
                }
                Err(error) => format!("spawn failed: {error} ({:?})", error.kind()),
            };
            lines.push_str(&format!("control `{label}`: {line}\n"));
        }
        lines.push_str("--- end control relaunches ---");
        lines
    }

    #[derive(Clone, Copy)]
    enum Launch {
        Strict(ProcessLimits, StrictIsolationRequirements),
        Guarded(ProcessLimits),
        BareRlimits(ProcessLimits),
        Plain,
    }

    impl Launch {
        fn run(self) -> io::Result<Outcome> {
            let started = Instant::now();
            match self {
                Self::Strict(limits, requirements) => {
                    let probe = !requirements.requires(StrictIsolationControl::SeccompFilter);
                    let mut command = child_command(probe);
                    let child =
                        spawn_guarded_child_strict_with(&mut command, limits, requirements)?;
                    Ok(Outcome::from_guarded(child, started))
                }
                Self::Guarded(limits) => {
                    let mut command = child_command(true);
                    let child = spawn_guarded_child(&mut command, limits)?;
                    Ok(Outcome::from_guarded(child, started))
                }
                Self::BareRlimits(limits) => {
                    let mut command = child_command(true);
                    // SAFETY: the closure only issues async-signal-safe
                    // `setrlimit` calls on values copied into it, and builds
                    // no heap-backed state after fork.
                    unsafe {
                        command.pre_exec(move || {
                            set_limit(libc::RLIMIT_CPU, limits.cpu_seconds)?;
                            set_limit(libc::RLIMIT_AS, limits.address_space_bytes)?;
                            set_limit(libc::RLIMIT_NOFILE, limits.open_files)?;
                            set_limit(libc::RLIMIT_FSIZE, limits.file_size_bytes)
                        });
                    }
                    Ok(Outcome::from_plain(command.spawn()?, started))
                }
                Self::Plain => {
                    let mut command = child_command(true);
                    Ok(Outcome::from_plain(command.spawn()?, started))
                }
            }
        }
    }

    fn set_limit(resource: libc::__rlimit_resource_t, value: u64) -> io::Result<()> {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // SAFETY: `limit` is initialized and outlives the syscall.
        if unsafe { libc::setrlimit(resource, &limit) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    struct Outcome {
        status: ExitStatus,
        stdout: String,
        stderr: String,
        elapsed: Duration,
    }

    impl Outcome {
        fn from_guarded(mut child: GuardedChild, started: Instant) -> Self {
            let (stdout, stderr) = drain(
                child.child_mut().stdout.take(),
                child.child_mut().stderr.take(),
            );
            let status = child.wait().expect("guarded child should be reaped");
            Self {
                status,
                stdout,
                stderr,
                elapsed: started.elapsed(),
            }
        }

        fn from_plain(mut child: Child, started: Instant) -> Self {
            let (stdout, stderr) = drain(child.stdout.take(), child.stderr.take());
            let status = child.wait().expect("child should be reaped");
            Self {
                status,
                stdout,
                stderr,
                elapsed: started.elapsed(),
            }
        }

        /// The status, the elapsed time, and — the fact the bare signal
        /// number could not give — whether the child's `main` ever ran.
        fn summary(&self) -> String {
            let reached = if self.status.success() {
                "main started and the child exited cleanly".to_owned()
            } else if self.stderr.contains(STARTED) {
                let last = self
                    .stderr
                    .lines()
                    .rfind(|line| line.starts_with("seccomp child:"))
                    .unwrap_or(STARTED);
                format!("main started, so the child died after execve; last child step: `{last}`")
            } else {
                "the `started` line never arrived, so the child died before its main ran \
                 (in pre_exec, in execve, in the dynamic loader, or in Rust runtime startup)"
                    .to_owned()
            };
            format!(
                "{} after {} ms; {reached}",
                describe(self.status),
                self.elapsed.as_millis()
            )
        }
    }

    /// Read both pipes to EOF. stderr is drained on its own thread: a child
    /// that dies noisily can otherwise fill the stderr pipe while the parent
    /// is still blocked on stdout.
    fn drain(stdout: Option<ChildStdout>, stderr: Option<ChildStderr>) -> (String, String) {
        let mut child_stdout = stdout.expect("child stdout must be piped");
        let mut child_stderr = stderr.expect("child stderr must be piped");
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
        (stdout, stderr)
    }

    /// The host facts a signal-9 death depends on: the kernel, whether the
    /// parent already runs under seccomp or `no_new_privs` (a container or
    /// runner policy stacks its filters under ours), the parent's own limits,
    /// its cgroup, and how large the image is that the child has to map.
    fn host_facts() -> String {
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.trim().to_owned())
            .unwrap_or_else(|error| format!("<unreadable: {error}>"));
        let status = std::fs::read_to_string("/proc/self/status")
            .map(|status| {
                status
                    .lines()
                    .filter(|line| {
                        line.starts_with("NoNewPrivs:")
                            || line.starts_with("Seccomp")
                            || line.starts_with("VmPeak:")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|error| format!("<unreadable: {error}>"));
        let limits = std::fs::read_to_string("/proc/self/limits")
            .map(|limits| {
                limits
                    .lines()
                    .filter(|line| {
                        [
                            "Max cpu time",
                            "Max address space",
                            "Max open files",
                            "Max file size",
                        ]
                        .iter()
                        .any(|name| line.starts_with(name))
                    })
                    .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_else(|error| format!("<unreadable: {error}>"));
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")
            .map(|cgroup| cgroup.trim().replace('\n', "; "))
            .unwrap_or_else(|error| format!("<unreadable: {error}>"));
        let image = std::env::current_exe()
            .and_then(|path| {
                let bytes = std::fs::metadata(&path)?.len();
                Ok(format!("{} ({bytes} bytes)", path.display()))
            })
            .unwrap_or_else(|error| format!("<unreadable: {error}>"));
        format!(
            "--- host facts ---\n\
             kernel: {kernel}\n\
             parent status: {status}\n\
             parent limits: {limits}\n\
             parent cgroup: {cgroup}\n\
             child image: {image}\n"
        )
    }

    /// Name the exit status the way a reader needs it: a child killed by a
    /// signal (a seccomp `SIGSYS`, an out-of-memory `SIGKILL`) reports no exit
    /// code at all, which is exactly the case a bare `success()` hid.
    fn describe(status: ExitStatus) -> String {
        match (status.code(), status.signal()) {
            (Some(code), _) => format!("exit code {code}"),
            (None, Some(signal)) if status.core_dumped() => {
                format!("killed by signal {signal} (core dumped)")
            }
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => format!("unknown exit status {status}"),
        }
    }
}
