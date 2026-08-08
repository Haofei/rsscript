// Reuse the intrinsic-catalog generator during the crate-boundary migration.
// It emits the VM instruction enum/lookup and runtime ABI table from the same
// declarative manifest as the compiler, preventing a second source of truth.
include!("../rsscript/build.rs");
