//! Bounded terminal-state differential for the experiment-owned VM backends.
//!
//! The reviewed SDK corpus owns Provider failure and run-owned resource cleanup.
//! These no-Provider fixtures cover the shared engine states which every
//! in-process backend can observe under exactly the same `VmLimits`.

use std::time::Duration;

use rss_testgen::backends::{Backend, all_inprocess_backends};
use rsscript_sdk::operation::{CancellationToken, MonotonicDeadline};
use rsscript_sdk::{EvalError, ExecutionFailureKind, VmLimits};

const COMPLETED: &str = include_str!("corpus/execution_state/completed.rss");
const INFINITE_LOOP: &str = include_str!("corpus/execution_state/infinite_loop.rss");

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalState {
    Completed { value: String, stdout: String },
    Execution(ExecutionFailureKind),
    OtherFailure(String),
}

fn terminal_state(
    backend: &dyn Backend,
    name: &str,
    source: &str,
    limits: VmLimits,
) -> TerminalState {
    match backend.run_with_limits(name, source, &[], limits) {
        Ok(output) => TerminalState::Completed {
            value: output.value,
            stdout: output.stdout,
        },
        Err(EvalError::Execution { kind, .. }) => TerminalState::Execution(kind),
        Err(error) => TerminalState::OtherFailure(format!("{error:?}")),
    }
}

fn assert_all_backends_agree(
    name: &str,
    source: &str,
    make_limits: impl Fn() -> VmLimits,
    expected: TerminalState,
) {
    for backend in all_inprocess_backends() {
        let actual = terminal_state(backend.as_ref(), name, source, make_limits());
        assert_eq!(
            actual,
            expected,
            "{} terminal state diverged on fixture `{name}`",
            backend.name()
        );
    }
}

#[test]
fn execution_terminal_state_corpus_agrees_across_experimental_backends() {
    assert_all_backends_agree(
        "completed.rss",
        COMPLETED,
        VmLimits::default,
        TerminalState::Completed {
            value: "Unit".to_owned(),
            stdout: String::new(),
        },
    );
    assert_all_backends_agree(
        "infinite_loop.rss",
        INFINITE_LOOP,
        || VmLimits {
            step_budget: Some(32),
            ..VmLimits::default()
        },
        TerminalState::Execution(ExecutionFailureKind::StepBudgetExceeded),
    );
    assert_all_backends_agree(
        "cancelled.rss",
        COMPLETED,
        || {
            let cancellation = CancellationToken::new();
            cancellation.cancel();
            VmLimits {
                cancel: Some(cancellation),
                ..VmLimits::default()
            }
        },
        TerminalState::Execution(ExecutionFailureKind::Cancelled),
    );
    assert_all_backends_agree(
        "expired.rss",
        COMPLETED,
        || VmLimits {
            deadline: Some(MonotonicDeadline::after(Duration::ZERO)),
            ..VmLimits::default()
        },
        TerminalState::Execution(ExecutionFailureKind::DeadlineExceeded),
    );
}
