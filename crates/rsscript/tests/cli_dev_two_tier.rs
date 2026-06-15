//! `rss dev --run` two-tier execution: the fast reg-VM dev tier (default) and
//! the Rust-lowering AOT tier (`--release`) must produce the same program output.

mod common;

use std::fs;
use std::process::Command;

#[test]
fn dev_run_uses_the_fast_vm_tier_by_default() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let dir = common::unique_temp_dir("rss-dev-two-tier");
    fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("prog.rss");
    fs::write(
        &file,
        "fn main() -> Unit {\n    Log.write(message: read \"two-tier-ok\")\n    return Unit\n}\n",
    )
    .expect("fixture");
    let path = file.to_str().unwrap();

    let out = Command::new(bin)
        .args(["dev", "--run", "--once", path])
        .output()
        .expect("rss dev --run runs");
    assert!(
        out.status.success(),
        "dev --run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Routed through the VM tier (no rustc), program output present.
    assert!(stdout.contains("run (dev VM)"), "stdout:\n{stdout}");
    assert!(stdout.contains("two-tier-ok"), "stdout:\n{stdout}");

    let _ = fs::remove_dir_all(&dir);
}
