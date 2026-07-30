#![cfg(target_os = "linux")]

use rss_process_guard::WorkerIsolationBackend;
use rsscript::{IsolatedProgram, IsolatedWorkerConfig, NativeValue, eval_isolated_reference_vm};

#[test]
#[ignore = "requires root-owned /usr/bin/bwrap and enabled unprivileged user namespaces"]
fn bubblewrap_executes_one_bounded_eval_without_host_fallback() {
    assert_eq!(
        std::env::var("RSS_RUN_BWRAP_TESTS").as_deref(),
        Ok("1"),
        "set RSS_RUN_BWRAP_TESTS=1 only in the isolated-worker CI job"
    );
    let config = IsolatedWorkerConfig::new(
        env!("CARGO_BIN_EXE_rss-execution-worker"),
        WorkerIsolationBackend::bubblewrap(),
    )
    .expect("worker config");
    let output = eval_isolated_reference_vm(
        &config,
        IsolatedProgram::source(
            "main.rss",
            "fn main() -> Int { Log.write(message: \"isolated\"); return 42 }\n",
        ),
        Vec::new(),
    )
    .expect("isolated evaluation");

    assert_eq!(output.native_value, Some(NativeValue::Int(42)));
    assert_eq!(output.stdout, "isolated\n");
    assert!(output.stderr.is_empty());
}

#[test]
#[ignore = "requires root-owned /usr/bin/bwrap and enabled unprivileged user namespaces"]
fn bubblewrap_timeout_kills_the_worker_without_fallback() {
    assert_eq!(
        std::env::var("RSS_RUN_BWRAP_TESTS").as_deref(),
        Ok("1"),
        "set RSS_RUN_BWRAP_TESTS=1 only in the isolated-worker CI job"
    );
    let config = IsolatedWorkerConfig::new(
        env!("CARGO_BIN_EXE_rss-execution-worker"),
        WorkerIsolationBackend::bubblewrap(),
    )
    .expect("worker config")
    .with_wall_timeout(std::time::Duration::from_millis(100))
    .expect("timeout");
    let error = eval_isolated_reference_vm(
        &config,
        IsolatedProgram::source("main.rss", "fn main() -> Int { while true { } return 0 }\n"),
        Vec::new(),
    )
    .expect_err("the worker must be terminated");

    assert!(
        matches!(error, rsscript::EvalError::Runtime(message) if message.contains("wall timeout"))
    );
}
