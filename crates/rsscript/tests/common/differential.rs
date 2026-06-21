//! N-way differential execution: run one program on the configured execution
//! backend set and require agreement. Default runs stay in-process
//! (VM interpreter + JIT, plus native tiers when built); full parity mode adds
//! generated Rust.
//!
//! Each `Backend` returns normalized stdout (or an error string). The harness
//! requires every backend to agree on success+stdout (and, when a divergence is
//! found, names the two backends that disagree so a JIT bug is easy to localize:
//! the backend that disagrees with the *other two* is the culprit).

/// An execution engine for RSScript source.
pub trait Backend {
    fn name(&self) -> &'static str;
    /// Run `main` and return normalized stdout, or `Err(message)` on failure.
    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String>;
}

/// Normalized stdout, or `Err` if `main` returned `Err` (ledger SH-005: a
/// `main` returning `Err` is a failed run, matching the AOT backend's non-zero
/// exit). A runnable `main` is `Unit` or `Result<Unit, E>`, so an `Err` variant
/// in the return value is unambiguously the failure case.
fn stdout_or_main_err(output: rsscript::EvalOutput) -> Result<String, String> {
    if let Some(rsscript::NativeValue::Variant { name, .. }) = &output.native_value
        && name == "Err"
    {
        return Err(format!("main returned {}", output.value));
    }
    Ok(output.stdout)
}

/// The register-VM interpreter.
pub struct Interpreter;

impl Backend for Interpreter {
    fn name(&self) -> &'static str {
        "vm-interpreter"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        super::run_vm_source(file, source, args)
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The Rust-lowering compiled backend (built + run, cached).
pub struct Compiled;

impl Backend for Compiled {
    fn name(&self) -> &'static str {
        "rust-compiled"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        super::try_run_compiled_source(file, source, args).map(|(stdout, _stderr)| stdout)
    }
}

/// The JIT execution mode (specializing compile of the register bytecode with
/// fallback to the interpreter for unsupported instructions). Correct by the
/// fallback rule; this differential harness guards what it *does* compile.
pub struct Jit;

impl Backend for Jit {
    fn name(&self) -> &'static str {
        "vm-jit"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        rsscript::reg_vm_eval_source_main_jit(file, source, args.iter().copied())
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The native (Cranelift) JIT execution mode: the integer/control core runs as
/// machine code, tier-0 covers the rest of the supported subset, and the
/// interpreter the remainder (with bail-to-interpreter on arithmetic edges).
/// Correct by the same fallback rule; this differential guards what it compiles.
#[cfg(feature = "native-jit")]
pub struct NativeJit;

#[cfg(feature = "native-jit")]
impl Backend for NativeJit {
    fn name(&self) -> &'static str {
        "vm-jit-native"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        rsscript::reg_vm_eval_source_main_native(file, source, args.iter().copied())
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The native tier in deopt stress mode: the native code always bails, so every
/// native-eligible function exercises the fallback-to-interpreter path. Must
/// still agree with every other backend (verifies deopt has no gap).
#[cfg(feature = "native-jit")]
pub struct NativeJitForceDeopt;

#[cfg(feature = "native-jit")]
impl Backend for NativeJitForceDeopt {
    fn name(&self) -> &'static str {
        "vm-jit-native-force-deopt"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        rsscript::reg_vm_eval_source_main_native_force_deopt(file, source, args.iter().copied())
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The native tier with J5.2 OSR forced on: a function with a qualifying
/// native-subset hot loop runs that loop natively *mid-function* (OSR-entry at the
/// loop header reading the live-in window; OSR-exit / precise-resume at the
/// post-loop ip). Must agree byte-for-byte with every other backend — the OSR loop
/// is required to be observably identical to interpreting it.
#[cfg(feature = "native-jit")]
pub struct NativeJitOsr;

#[cfg(feature = "native-jit")]
impl Backend for NativeJitOsr {
    fn name(&self) -> &'static str {
        "vm-jit-native-osr"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        rsscript::reg_vm_eval_source_main_native_osr(file, source, args.iter().copied())
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The fast default execution backends. These stay in-process, so broad
/// differential sweeps do not spawn Cargo for every generated program.
pub fn fast_backends() -> Vec<Box<dyn Backend>> {
    #[cfg(feature = "native-jit")]
    {
        let mut backends: Vec<Box<dyn Backend>> = vec![Box::new(Interpreter), Box::new(Jit)];
        backends.push(Box::new(NativeJit));
        backends.push(Box::new(NativeJitForceDeopt));
        backends.push(Box::new(NativeJitOsr));
        backends
    }
    #[cfg(not(feature = "native-jit"))]
    {
        vec![Box::new(Interpreter), Box::new(Jit)]
    }
}

/// The full backend set, including the generated Rust backend. Use this for
/// focused smoke coverage or full parity runs via `RSSCRIPT_FULL_BACKEND_PARITY`.
pub fn full_backends() -> Vec<Box<dyn Backend>> {
    let mut backends = fast_backends();
    backends.push(Box::new(Compiled));
    backends
}

/// The standard set of execution backends to cross-check. Default runs use the
/// fast in-process set; set `RSSCRIPT_FULL_BACKEND_PARITY=1` to include the
/// generated Rust backend in every differential assertion.
pub fn all_backends() -> Vec<Box<dyn Backend>> {
    if super::full_backend_parity_enabled() {
        full_backends()
    } else {
        fast_backends()
    }
}

/// Run `source` on every backend and require identical successful stdout. Panics
/// (so proptest shrinks) with the diverging pair on mismatch.
pub fn assert_backends_agree(file: &str, source: &str, args: &[&str]) {
    assert_backends_agree_on(file, source, args, &all_backends());
}

/// Run `source` on every backend, including generated Rust, regardless of the
/// default fast/full parity mode.
pub fn assert_backends_agree_full(file: &str, source: &str, args: &[&str]) {
    assert_backends_agree_on(file, source, args, &full_backends());
}

/// Failure-path differential: assert that **every** backend fails (returns
/// `Err`) on `source`. This is the semantic-hardening check the success-path
/// [`assert_backends_agree`] can't make — it catches a backend that silently
/// succeeds (or, for the native tier, loops/returns garbage) where the others
/// error. Error *messages* aren't compared (each backend formats differently);
/// the contract is that a failing program fails on all of them.
pub fn assert_backends_all_fail(file: &str, source: &str, args: &[&str]) {
    for backend in all_backends() {
        if let Ok(stdout) = backend.run_stdout(file, source, args) {
            panic!(
                "backend `{}` unexpectedly succeeded on a failure-path program \
                 {file} (stdout: {stdout:?})\n--- source ---\n{source}",
                backend.name()
            );
        }
    }
}

/// Like [`assert_backends_agree`] but with an explicit backend set. Returns the
/// stdout every backend agreed on (useful for comparing a program against a
/// semantics-preserving variant of it).
pub fn assert_backends_agree_on(
    file: &str,
    source: &str,
    args: &[&str],
    backends: &[Box<dyn Backend>],
) -> String {
    let mut reference: Option<(&'static str, String)> = None;
    for backend in backends {
        let stdout = backend
            .run_stdout(file, source, args)
            .unwrap_or_else(|error| {
                panic!(
                    "backend `{}` failed on {file}: {error}\n--- source ---\n{source}",
                    backend.name()
                )
            });
        match &reference {
            None => reference = Some((backend.name(), stdout)),
            Some((reference_name, reference_stdout)) => assert_eq!(
                &stdout,
                reference_stdout,
                "backend divergence on {file}: `{reference_name}` vs `{}`\n--- source ---\n{source}",
                backend.name()
            ),
        }
    }
    reference.expect("at least one backend").1
}

/// Run `source` on every backend, assert they all agree, and return the agreed
/// stdout. The metamorphic-test entry point: it both enforces N-way parity and
/// hands back the value to compare across equivalent program variants.
pub fn backends_agreed_stdout(file: &str, source: &str, args: &[&str]) -> String {
    assert_backends_agree_on(file, source, args, &all_backends())
}
