mod common;

use std::fs;
use std::process::Command;

use rsscript::{
    EvalError, eval_source_main, lower_source_to_rust_package, write_generated_rust_package,
};

#[test]
fn eval_runs_pure_arithmetic_main() {
    let source = r#"
fn main() -> Int {
    let x = 2
    let y = 3
    return x + y * 4
}
"#;

    let output = eval_source_main("eval-arithmetic.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "14");
}

#[test]
fn eval_runs_user_function_and_assignment() {
    let source = r#"
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main() -> Int {
    let mut total = add(a: 1, b: 2)
    total = total + 4
    return total
}
"#;

    let output = eval_source_main("eval-function.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "7");
}

#[test]
fn eval_runs_nested_pattern_match() {
    let source = r#"
fn main() -> String {
    let value = Some(Some("rss"))
    match value {
        Some(Some(text)) => {
            return read text
        }
        Some(None) => {
            return "inner none"
        }
        None => {
            return "none"
        }
    }
}
"#;

    let output = eval_source_main("eval-nested-match.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "rss");
}

#[test]
fn eval_reports_unsupported_runtime_intrinsic() {
    let source = r#"
fn main() -> Int {
    return Random.int(min: 0, max: 10)
}
"#;

    let error = eval_source_main("eval-unsupported.rss", source)
        .expect_err("unsupported intrinsic should fail");

    assert!(matches!(error, EvalError::Runtime(message) if message.contains("Random.int")));
}

#[test]
fn eval_matches_lowered_rust_for_pure_core_example() {
    let source_path = "examples/scripts/core/interpreter_pure_parity.rss";
    let source = fs::read_to_string(source_path).expect("parity fixture should be readable");
    let eval = eval_source_main(source_path, &source).expect("eval should succeed");
    assert_eq!(eval.value, "Unit");

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package =
        lower_source_to_rust_package(source_path, &source, "rsscript_eval_parity", &runtime_path)
            .expect("parity fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, eval.stdout);
    assert_eq!(stderr, eval.stderr);
}

#[test]
fn eval_fails_closed_where_lowered_rust_crosses_declared_host_boundary() {
    let source_path = "examples/scripts/core/interpreter_host_boundary.rss";
    let source = fs::read_to_string(source_path).expect("host boundary fixture should be readable");
    let eval = eval_source_main(source_path, &source).expect_err("eval should fail closed");
    assert!(
        matches!(&eval, EvalError::Runtime(message) if message.contains("Env.get_or_default")),
        "{eval:?}"
    );

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package = lower_source_to_rust_package(
        source_path,
        &source,
        "rsscript_eval_host_boundary",
        &runtime_path,
    )
    .expect("host boundary fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-host-boundary");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("RSSCRIPT_EVAL_HOST_BOUNDARY", "host-ok")
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "host-ok\n");
    assert_eq!(stderr, "");
}
