#![forbid(unsafe_code)]

use rsscript_compiler::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    report::TerminationReason,
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};

const SOURCE: &str = include_str!("../script/main.rss");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let artifact = Compiler.compile("main.rss", SOURCE)?;
    let verified = ArtifactVerifier.verify(artifact)?;

    // The program imports no external symbols, so the empty registry is a
    // complete link. A missing Provider would fail here, before execution.
    let report = Runtime::default().link(&verified)?.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded())
            .trace(TracePolicy::MetadataOnly),
    );

    if report.termination_reason != TerminationReason::Completed {
        let failure = report
            .failure
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| report.termination_reason.as_str().to_string());
        return Err(failure.into());
    }
    assert_eq!(report.stdout, "user\nprofile\n");
    assert!(report.provider_call_traces.is_empty());

    println!("artifact digest: {}", report.artifact_digest);
    println!("termination: {}", report.termination_reason.as_str());
    println!("steps: {}", report.usage.steps_consumed);
    print!("structured output:\n{}", report.stdout);
    Ok(())
}
