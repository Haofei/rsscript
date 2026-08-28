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
    experimental::native_jit::NativeJitOptions,
    provider_api::ProviderRegistry,
    report::ExecutionEngineTelemetry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};

const SAMPLES: usize = 7;

struct MatrixCase {
    name: &'static str,
    workload: &'static str,
    source: &'static str,
    size: &'static str,
}

const CASES: &[MatrixCase] = &[
    MatrixCase {
        name: "pure-scalar",
        workload: "pure_scalar",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_scalar_loop.rss"),
        size: "200000",
    },
    MatrixCase {
        name: "static-calls",
        workload: "static_calls",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_call_chain.rss"),
        size: "150000",
    },
    MatrixCase {
        name: "struct",
        workload: "struct",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_struct_scalar_replace.rss"),
        size: "50000",
    },
    MatrixCase {
        name: "variant",
        workload: "variant",
        source: include_str!(
            "../../../benchmarks/vm-jit/kernels/native_variant_scalar_replace.rss"
        ),
        size: "50000",
    },
    MatrixCase {
        name: "option",
        workload: "option_result",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_option_scalar_replace.rss"),
        size: "50000",
    },
    MatrixCase {
        name: "result",
        workload: "option_result",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_result_scalar_replace.rss"),
        size: "50000",
    },
    MatrixCase {
        name: "list",
        workload: "list",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_read_heap.rss"),
        size: "100000",
    },
    MatrixCase {
        name: "string",
        workload: "string",
        source: include_str!("../../../benchmarks/vm-jit/kernels/string_text_processing.rss"),
        size: "2000",
    },
];

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn generated_binary(target: &Path, package_name: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{package_name}.exe")
    } else {
        package_name.to_owned()
    };
    target.join("release").join(name)
}

fn concise_build_failure(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let detail = stderr
        .lines()
        .find(|line| line.trim_start().starts_with("error"))
        .unwrap_or("generated Rust package did not compile")
        .trim();
    format!("AOT generated package unsupported: {detail}")
}

#[test]
#[ignore = "builds and measures the experimental AOT backend"]
fn emits_measured_aot_jit_interpreter_matrix() {
    let filter = std::env::var("RSS_AOT_JIT_CASE").ok();
    let samples = std::env::var("RSS_AOT_JIT_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(SAMPLES)
        .clamp(3, 30);
    let temp = tempfile::tempdir().expect("temporary AOT matrix workspace");
    let target_dir = temp.path().join("target");
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aot-runtime")
        .canonicalize()
        .expect("AOT runtime path");
    let mut cases_run = 0usize;
    for case in CASES {
        if filter
            .as_deref()
            .is_some_and(|selected| selected != case.name)
        {
            continue;
        }
        cases_run += 1;
        let source_name = format!("{}.rss", case.name);
        let built = Compiler
            .compile(&source_name, case.source)
            .unwrap_or_else(|error| panic!("{} Core compilation failed: {error}", case.name));
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{} Artifact verification failed: {error}", case.name))
            .admit_trusted_input();
        let linked = Runtime::new(ProviderRegistry::default())
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{} linking failed: {error}", case.name));
        let request =
            || ExecutionRequest::new([case.size]).limits(RunLimits::unbounded_for_trusted_host());
        let native_request = || request().native_jit(NativeJitOptions::default());
        let expected = linked.execute(request());
        let mut latest_native = linked.execute(native_request());
        assert_eq!(
            latest_native.outcome(),
            expected.outcome(),
            "{} outcome",
            case.name
        );
        assert_eq!(
            latest_native.stdout, expected.stdout,
            "{} stdout",
            case.name
        );
        assert_eq!(
            latest_native.stderr, expected.stderr,
            "{} stderr",
            case.name
        );

        let package_name = format!("rsscript-aot-matrix-{}", case.name);
        let package_dir = temp.path().join(&package_name);
        let lowered = lower_source_to_rust_package(
            &source_name,
            case.source,
            &package_name,
            &runtime_path.to_string_lossy(),
        );
        let mut unsupported_reason = None;
        let mut aot_compile = None;
        let mut binary = None;
        match lowered {
            Ok(package) => {
                write_generated_rust_package(&package_dir, &package)
                    .unwrap_or_else(|error| panic!("{} package write failed: {error}", case.name));
                let compile_started = Instant::now();
                let build = Command::new("cargo")
                    .args(["build", "--release"])
                    .current_dir(&package_dir)
                    .env("CARGO_TARGET_DIR", &target_dir)
                    .output()
                    .expect("run generated AOT build");
                if build.status.success() {
                    aot_compile = Some(compile_started.elapsed());
                    let path = generated_binary(&target_dir, &package_name);
                    let warm = Command::new(&path)
                        .arg(case.size)
                        .output()
                        .unwrap_or_else(|error| panic!("{} AOT warmup failed: {error}", case.name));
                    assert!(warm.status.success(), "{} AOT warmup failed", case.name);
                    assert_eq!(String::from_utf8_lossy(&warm.stdout), expected.stdout);
                    assert_eq!(String::from_utf8_lossy(&warm.stderr), expected.stderr);
                    binary = Some(path);
                } else {
                    unsupported_reason = Some(concise_build_failure(&build.stderr));
                }
            }
            Err(diagnostics) => {
                unsupported_reason = Some(format!(
                    "AOT lowering unsupported: {}",
                    diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.summary.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
        }

        let mut interpreter_samples = Vec::with_capacity(samples);
        let mut jit_samples = Vec::with_capacity(samples);
        let mut aot_samples = Vec::with_capacity(samples);
        for sample in 0..samples {
            let mut interpreter = || {
                let started = Instant::now();
                let report = linked.execute(request());
                interpreter_samples.push(started.elapsed());
                assert_eq!(
                    report.outcome(),
                    expected.outcome(),
                    "{} interpreter",
                    case.name
                );
                assert_eq!(
                    report.stdout, expected.stdout,
                    "{} interpreter stdout",
                    case.name
                );
            };
            let mut jit = || {
                let started = Instant::now();
                latest_native = linked.execute(native_request());
                jit_samples.push(started.elapsed());
                assert_eq!(
                    latest_native.outcome(),
                    expected.outcome(),
                    "{} JIT",
                    case.name
                );
                assert_eq!(
                    latest_native.stdout, expected.stdout,
                    "{} JIT stdout",
                    case.name
                );
            };
            let mut aot = || {
                let Some(binary) = binary.as_ref() else {
                    return;
                };
                let started = Instant::now();
                let output = Command::new(binary)
                    .arg(case.size)
                    .output()
                    .unwrap_or_else(|error| panic!("{} AOT run failed: {error}", case.name));
                aot_samples.push(started.elapsed());
                assert!(output.status.success(), "{} AOT process failed", case.name);
                assert_eq!(String::from_utf8_lossy(&output.stdout), expected.stdout);
                assert_eq!(String::from_utf8_lossy(&output.stderr), expected.stderr);
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

        let (compile_nanos, transitions, helper_calls, bounds_checks, jit_status, jit_reason) =
            match latest_native.telemetry.engine {
                ExecutionEngineTelemetry::Native {
                    compile_nanos,
                    native_calls,
                    osr_entries,
                    continuation_entries,
                    runtime_helper_call_sites,
                    direct_list_bounds_check_sites,
                    direct_list_bounds_checks_elided,
                    ..
                } => {
                    let transitions = native_calls
                        .saturating_add(osr_entries)
                        .saturating_add(continuation_entries);
                    (
                        compile_nanos,
                        transitions,
                        runtime_helper_call_sites,
                        direct_list_bounds_check_sites
                            .saturating_sub(direct_list_bounds_checks_elided),
                        if transitions > 0 {
                            "measured"
                        } else {
                            "declined"
                        },
                        (transitions == 0).then_some("native tier declined this workload"),
                    )
                }
                ExecutionEngineTelemetry::Interpreter => (
                    0,
                    0,
                    0,
                    0,
                    "declined",
                    Some("native tier returned interpreter-only telemetry"),
                ),
            };
        let (aot_status, aot_execution, aot_compile_nanos, aot_reason) =
            if let Some(reason) = unsupported_reason {
                ("unsupported", None, None, Some(reason))
            } else {
                (
                    "measured",
                    Some(median(aot_samples).as_nanos()),
                    aot_compile.map(|duration| duration.as_nanos()),
                    Some(
                        "process-spawn end-to-end execution; generated package release build"
                            .to_owned(),
                    ),
                )
            };
        let record = serde_json::json!({
            "schema": "rsscript.aot_jit_matrix.v1",
            "workload": case.workload,
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
                    "status": jit_status,
                    "execution_ns": median(jit_samples).as_nanos(),
                    "compile_ns": compile_nanos,
                    "transitions": transitions,
                    "host_helper_calls": helper_calls,
                    "bounds_checks": bounds_checks,
                    "allocations_eliminated": null,
                    "reason": jit_reason
                },
                "aot": {
                    "status": aot_status,
                    "execution_ns": aot_execution,
                    "compile_ns": aot_compile_nanos,
                    "transitions": if aot_status == "measured" { Some(0_u64) } else { None },
                    "host_helper_calls": null,
                    "bounds_checks": null,
                    "allocations_eliminated": null,
                    "reason": aot_reason
                }
            }
        });
        println!("AOT_JIT_MATRIX {record}");
    }
    assert!(
        cases_run > 0,
        "RSS_AOT_JIT_CASE did not match an AOT/JIT matrix case"
    );
}
