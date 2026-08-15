#![forbid(unsafe_code)]

use rsscript_compiler::{
    artifact::ArtifactVerifier,
    compile::{Compiler, FrontendInputSnapshot},
    operation::CancellationToken,
    report::TerminationReason,
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};

const SOURCE: &str = include_str!("../script/main.rss");

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let input = FrontendInputSnapshot::single("main.rss", SOURCE);
    let artifact = Compiler.compile_snapshot(&input)?;
    let admitted = ArtifactVerifier.verify(artifact)?.admit_trusted_input();

    // The program imports no external symbols, so the empty registry is a
    // complete link. A missing Provider would fail here, before execution.
    let runtime = Runtime::default();
    let linked = runtime.link(&admitted)?;
    let report = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded())
            .trace(TracePolicy::MetadataOnly),
    );

    if report.termination_reason() != TerminationReason::Completed {
        let failure = report
            .failure()
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| report.termination_reason().as_str().to_string());
        return Err(failure.into());
    }
    assert_eq!(report.stdout, "user\nprofile\n");
    assert!(report.provider_call_traces.is_empty());

    // Reuse the exact same linked Artifact with a host-owned cancellation
    // request. This makes the execution boundary concrete: cancellation is a
    // per-run control, not an Artifact property and not a Provider authority.
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = linked.execute(
        ExecutionRequest::default().limits(
            RunLimits::bounded()
                .with_cancellation(cancellation)
                .with_step_budget(1_000),
        ),
    );
    assert_eq!(cancelled.termination_reason(), TerminationReason::Cancelled);
    assert!(cancelled.failure().is_some());
    assert!(cancelled.stdout.is_empty());
    assert!(cancelled.provider_call_traces.is_empty());

    println!("artifact digest: {}", report.artifact_digest);
    println!("termination: {}", report.termination_reason().as_str());
    println!("steps: {}", report.usage.steps_consumed);
    print!("structured output:\n{}", report.stdout);
    println!(
        "cancelled termination: {} (steps: {})",
        cancelled.termination_reason().as_str(),
        cancelled.usage.steps_consumed
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_artifact_supports_success_and_host_cancellation_runs() {
        super::run().expect("structured async example must run through both paths");
    }
}
