//! N-way differential execution: run one program on every execution backend
//! (VM interpreter, compiled Rust, and — once it exists — the JIT) and require
//! they agree. This is the verification-first safety net: a JIT plugs in as a
//! third `Backend` and is checked against the other two from its first commit.
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

/// The register-VM interpreter.
pub struct Interpreter;

impl Backend for Interpreter {
    fn name(&self) -> &'static str {
        "vm-interpreter"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        super::run_vm_source(file, source, args)
            .map(|output| output.stdout)
            .map_err(|error| format!("{error:?}"))
    }
}

/// The Rust-lowering compiled backend (built + run, cached).
pub struct Compiled;

impl Backend for Compiled {
    fn name(&self) -> &'static str {
        "rust-compiled"
    }

    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String> {
        let (stdout, _stderr) = super::run_compiled_source(file, source, args);
        Ok(stdout)
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
            .map(|output| output.stdout)
            .map_err(|error| format!("{error:?}"))
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
            .map(|output| output.stdout)
            .map_err(|error| format!("{error:?}"))
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
            .map(|output| output.stdout)
            .map_err(|error| format!("{error:?}"))
    }
}

/// The standard set of execution backends to cross-check. With the `native-jit`
/// feature, the native tier and its force-deopt twin join as additional backends.
pub fn all_backends() -> Vec<Box<dyn Backend>> {
    #[cfg(not(feature = "native-jit"))]
    {
        vec![Box::new(Interpreter), Box::new(Jit), Box::new(Compiled)]
    }
    #[cfg(feature = "native-jit")]
    {
        vec![
            Box::new(Interpreter),
            Box::new(Jit),
            Box::new(NativeJit),
            Box::new(NativeJitForceDeopt),
            Box::new(Compiled),
        ]
    }
}

/// Run `source` on every backend and require identical successful stdout. Panics
/// (so proptest shrinks) with the diverging pair on mismatch.
pub fn assert_backends_agree(file: &str, source: &str, args: &[&str]) {
    assert_backends_agree_on(file, source, args, &all_backends());
}

/// Like [`assert_backends_agree`] but with an explicit backend set.
pub fn assert_backends_agree_on(
    file: &str,
    source: &str,
    args: &[&str],
    backends: &[Box<dyn Backend>],
) {
    let mut reference: Option<(&'static str, String)> = None;
    for backend in backends {
        let stdout = backend.run_stdout(file, source, args).unwrap_or_else(|error| {
            panic!(
                "backend `{}` failed on {file}: {error}\n--- source ---\n{source}",
                backend.name()
            )
        });
        match &reference {
            None => reference = Some((backend.name(), stdout)),
            Some((reference_name, reference_stdout)) => assert_eq!(
                &stdout, reference_stdout,
                "backend divergence on {file}: `{reference_name}` vs `{}`\n--- source ---\n{source}",
                backend.name()
            ),
        }
    }
}
