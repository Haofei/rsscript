//! In-process execution backends for the differential.
//!
//! These run RSScript source through the register VM's public entry points —
//! interpreter, tier-0 JIT, and (under the `native-jit` feature) the native
//! Cranelift tier plus its force-deopt twin. They are microsecond-fast (no
//! `rustc` involved), so the whole set can run inside a libFuzzer iteration.
//!
//! The compiled-Rust backend is intentionally absent here: it builds a crate per
//! program and is far too slow for coverage-guided fuzzing. The in-suite proptest
//! driver adds it separately (via the test crate's existing compiled runner) so
//! the bounded runs still get the full N-way check.

use rsscript::{EvalOutput, NativeValue};

/// An execution engine for a runnable RSScript program.
pub trait Backend {
    fn name(&self) -> &'static str;
    /// Run `main` and return its normalized stdout, or `Err(message)` on failure
    /// (a compile/runtime error, or a `main` that returned `Err`).
    fn run_stdout(&self, file: &str, source: &str, args: &[&str]) -> Result<String, String>;
}

/// Normalize an [`EvalOutput`] into success-stdout or failure. A `main` returning
/// the `Err` variant of `Result<Unit, E>` is a failed run (matching the compiled
/// backend's non-zero exit), so it maps to `Err`.
pub fn stdout_or_main_err(output: EvalOutput) -> Result<String, String> {
    if let Some(NativeValue::Variant { name, .. }) = &output.native_value
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
        rsscript::reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
            .map_err(|error| format!("{error:?}"))
            .and_then(stdout_or_main_err)
    }
}

/// The tier-0 JIT (specializing compile of the register bytecode, interpreter
/// fallback for unsupported instructions).
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

/// The native (Cranelift) JIT tier.
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

/// The native tier forced to bail to the interpreter on every native-eligible
/// function — exercises the deopt path, which must still agree.
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

/// Every in-process backend (the native tier and its force-deopt twin join under
/// the `native-jit` feature).
pub fn all_inprocess_backends() -> Vec<Box<dyn Backend>> {
    #[cfg(not(feature = "native-jit"))]
    {
        vec![Box::new(Interpreter), Box::new(Jit)]
    }
    #[cfg(feature = "native-jit")]
    {
        vec![
            Box::new(Interpreter),
            Box::new(Jit),
            Box::new(NativeJit),
            Box::new(NativeJitForceDeopt),
        ]
    }
}
