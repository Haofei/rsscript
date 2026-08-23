//! Controlled cross-engine evidence harness.
//!
//! This lives in the experiments workspace so Core never depends on AOT. It is
//! ignored by default because it invokes a nested release Cargo build. A
//! controlled runner can execute it with pinned CPU/governor settings and
//! retain the emitted `AOT_JIT_MATRIX` record as CI evidence.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use rsscript_aot_backend::{lower_source_to_rust_package, write_generated_rust_package};
use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    report::ExecutionEngineTelemetry,
    runtime::{ExecutionRequest, NativeJitOptions, RunLimits, Runtime},
};

const SOURCE: &str = include_str!("../../../benchmarks/vm-jit/kernels/native_scalar_loop.rss");
const SIZE: &str = "200000";
const SAMPLES: usize = 7;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn generated_binary(target: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "rsscript-aot-jit-matrix.exe"
    } else {
        "rsscript-aot-jit-matrix"
    };
    target.join("release").join(name)
}

#[test]
#[ignore = "builds and measures the experimental AOT backend"]
fn emits_measured_aot_jit_interpreter_matrix() {
    let temp = tempfile::tempdir().expect("temporary AOT package");
    let package_dir = temp.path().join("package");
    let target_dir = temp.path().join("target");
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aot-runtime")
        .canonicalize()
        .expect("AOT runtime path");
    let package = lower_source_to_rust_package(
        "native_scalar_loop.rss",
        SOURCE,
        "rsscript-aot-jit-matrix",
        &runtime_path.to_string_lossy(),
    )
    .unwrap_or_else(|diagnostics| panic!("AOT lowering failed: {diagnostics:#?}"));
    write_generated_rust_package(&package_dir, &package).expect("publish generated package");

    let compile_started = Instant::now();
    let build = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&package_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("run generated AOT build");
    let aot_compile = compile_started.elapsed();
    assert!(
        build.status.success(),
        "generated AOT build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = generated_binary(&target_dir);

    let built = Compiler
        .compile("native_scalar_loop.rss", SOURCE)
        .expect("compile benchmark Artifact");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify benchmark Artifact")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("link benchmark");
    let request = || ExecutionRequest::new([SIZE]).limits(RunLimits::unbounded_for_trusted_host());
    let native_request = || request().native_jit(NativeJitOptions::default());

    let expected = linked.execute(request());
    let native_warm = linked.execute(native_request());
    assert_eq!(native_warm.outcome(), expected.outcome());
    assert_eq!(native_warm.stdout, expected.stdout);
    let aot_warm = Command::new(&binary)
        .arg(SIZE)
        .output()
        .expect("run generated AOT binary");
    assert!(aot_warm.status.success());
    assert_eq!(String::from_utf8_lossy(&aot_warm.stdout), expected.stdout);

    let mut interpreter_samples = Vec::with_capacity(SAMPLES);
    let mut jit_samples = Vec::with_capacity(SAMPLES);
    let mut aot_samples = Vec::with_capacity(SAMPLES);
    let mut latest_native = native_warm;
    for sample in 0..SAMPLES {
        let mut interpreter = || {
            let started = Instant::now();
            let report = linked.execute(request());
            interpreter_samples.push(started.elapsed());
            assert_eq!(report.outcome(), expected.outcome());
        };
        let mut jit = || {
            let started = Instant::now();
            latest_native = linked.execute(native_request());
            jit_samples.push(started.elapsed());
            assert_eq!(latest_native.outcome(), expected.outcome());
        };
        let mut aot = || {
            let started = Instant::now();
            let output = Command::new(&binary)
                .arg(SIZE)
                .output()
                .expect("run generated AOT binary");
            aot_samples.push(started.elapsed());
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout), expected.stdout);
        };
        match sample % 3 {
            0 => {
                interpreter();
                jit();
                aot();
            }
            1 => {
                aot();
                interpreter();
                jit();
            }
            _ => {
                jit();
                aot();
                interpreter();
            }
        }
    }

    let (compile_nanos, transitions) = match latest_native.telemetry.engine {
        ExecutionEngineTelemetry::Native {
            compile_nanos,
            native_calls,
            osr_entries,
            continuation_entries,
            ..
        } => (
            compile_nanos,
            native_calls
                .saturating_add(osr_entries)
                .saturating_add(continuation_entries),
        ),
        ExecutionEngineTelemetry::Interpreter => (0, 0),
    };
    let record = serde_json::json!({
        "schema": "rsscript.aot_jit_matrix.v1",
        "workload": "pure_scalar",
        "semantic_match": true,
        "engines": {
            "interpreter": {
                "status": "measured",
                "execution_ns": median(interpreter_samples).as_nanos(),
                "compile_ns": 0,
                "transitions": 0,
                "host_helper_calls": null,
                "bounds_checks": null,
                "allocations_eliminated": null,
                "reason": null
            },
            "jit": {
                "status": "measured",
                "execution_ns": median(jit_samples).as_nanos(),
                "compile_ns": compile_nanos,
                "transitions": transitions,
                "host_helper_calls": null,
                "bounds_checks": null,
                "allocations_eliminated": null,
                "reason": null
            },
            "aot": {
                "status": "measured",
                "execution_ns": median(aot_samples).as_nanos(),
                "compile_ns": aot_compile.as_nanos(),
                "transitions": 0,
                "host_helper_calls": null,
                "bounds_checks": null,
                "allocations_eliminated": null,
                "reason": "process-spawn end-to-end execution; generated package release build"
            }
        }
    });
    println!("AOT_JIT_MATRIX {record}");
}
