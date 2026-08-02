//! Dynamic loading of native package bindings into the VM.
//!
//! A native package's `native fn`s are implemented in a separate Rust crate that
//! the interpreter does not link. To run such a package in the VM, this module
//! generates a small cdylib "shim" from the package's interface + bindings,
//! builds it, and `dlopen`s it to obtain host bindings — no per-package
//! hand-written glue, and nothing hard-coded in the host. Native dependencies are
//! collected transitively, so a pure-RSS facade over a native package (e.g.
//! a plain RSS facade over a native binding package) works too.
//!
//! Supported binding shapes (params and returns), composable:
//! `Unit`, `String`, `Int`, `Float`, `Bool`, `Bytes`, `Path`, `List<T>`,
//! `Option<T>`, and `Result<T, String>` returns.
//!
//! `mut` parameters are supported: the shim passes them by `&mut` and returns an
//! envelope `List[result, mutated...]`, and the host writes the mutated values
//! back to the caller's registers so mutable native arguments update in place.
//!
//! Bindings whose parameter or return *types* aren't in the supported set are
//! **skipped** (not registered) rather than fatal — a program only fails
//! (`no host binding for ...`) if it actually calls one.
//!
//! Cargo build and dynamic loading are type-state guarded: executable native
//! inputs are captured by `package::prepare_executable_package`, and the
//! execution entry point requires the resulting `ExecutablePackageSnapshot`.

mod loader;
mod shim_gen;

pub use loader::{load_package_bindings, load_package_bindings_from_snapshot};
