#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rsscript_abi_model::{ExternalSymbol, WireResourceTypeId, WireValue};
use rsscript_compiler::{
    artifact::ArtifactVerifier,
    compile::{Compiler, FrontendInputSnapshot},
    operation::CancellationToken,
    provider_api::{ProviderError, ProviderFunction, ProviderRegistry, WireInterpreterFn},
    report::TerminationReason,
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};
use rsscript_provider_api::ProviderResource;

const SOURCE: &str = include_str!("../script/main.rss");
const SESSION_INTERFACE: &str = include_str!("../interfaces/session.rssi");

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

#[derive(Debug)]
struct CountedSession {
    cleanups: Arc<AtomicU64>,
    cleanup_events: Arc<Mutex<Vec<String>>>,
    backend: &'static str,
    fail_cleanup: bool,
}

impl ProviderResource for CountedSession {
    fn cleanup(&mut self) -> Result<(), ProviderError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        self.cleanup_events
            .lock()
            .map_err(|_| ProviderError::internal("session cleanup event lock poisoned"))?
            .push(self.backend.to_owned());
        if self.fail_cleanup {
            Err(ProviderError::internal(
                "intentional session cleanup failure",
            ))
        } else {
            Ok(())
        }
    }
}

fn session_functions(
    cleanups: Arc<AtomicU64>,
    cleanup_events: Arc<Mutex<Vec<String>>>,
    backend: &'static str,
    fail_cleanup: bool,
) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let function = descriptor()
        .functions
        .into_iter()
        .next()
        .expect("generated session descriptor must contain open");
    // The generated descriptor has one resource in the signature-scoped type
    // table. This numeric identity is part of the typed wire contract; the
    // Provider never returns the resource's source spelling in WireValue.
    let resource_type = WireResourceTypeId::new(0);
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new_contextual(move |context, arguments| {
                if !arguments.is_empty() {
                    return Err(ProviderError::invalid_argument("open expects no arguments"));
                }
                let handle = context.register_resource(CountedSession {
                    cleanups: Arc::clone(&cleanups),
                    cleanup_events: Arc::clone(&cleanup_events),
                    backend,
                    fail_cleanup,
                })?;
                Ok(WireValue::Resource {
                    handle: handle.to_wire(resource_type),
                })
            }),
        },
    )])
}

fn runtime_with_session(
    cleanups: Arc<AtomicU64>,
    cleanup_events: Arc<Mutex<Vec<String>>>,
    backend: &'static str,
    fail_cleanup: bool,
) -> Runtime {
    let mut providers = ProviderRegistry::default();
    providers
        .register(
            &descriptor(),
            session_functions(cleanups, cleanup_events, backend, fail_cleanup),
        )
        .expect("generated session Provider must register against its descriptor");
    Runtime::new(providers)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let input = FrontendInputSnapshot::from_sources(
        [("main.rss", SOURCE)],
        [("session.rssi", SESSION_INTERFACE)],
    );
    let artifact = Compiler.compile_snapshot(&input)?;
    let admitted = ArtifactVerifier.verify(artifact)?.admit_trusted_input();
    assert_eq!(
        admitted.external_imports(),
        &[rsscript_abi_model::ExternalImport {
            symbol: ExternalSymbol::new("host.session.open")
                .expect("generated session symbol is valid"),
            signature: descriptor().functions[0].signature.clone(),
            signature_hash: descriptor().functions[0].signature.hash(),
            abi_version: rsscript_abi_model::RUNTIME_ABI_VERSION,
        }],
        "compiler Artifact imports and generated Provider descriptor must share one structural contract"
    );

    // Provider selection is host-owned. The Artifact requires `host.session`
    // but remains unchanged as this in-memory resource implementation is
    // replaced by a production implementation with the same descriptor.
    let cleanups = Arc::new(AtomicU64::new(0));
    let memory_events = Arc::new(Mutex::new(Vec::new()));
    let runtime = runtime_with_session(
        Arc::clone(&cleanups),
        Arc::clone(&memory_events),
        "memory",
        false,
    );
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
    assert_eq!(report.provider_call_traces.len(), 1);
    assert_eq!(report.usage.resources_created, 1);
    assert_eq!(report.usage.resources_cleaned, 1);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(
        memory_events.lock().expect("memory event lock").as_slice(),
        ["memory"]
    );

    // A second host implementation links the unchanged Artifact through the
    // same generated descriptor. Its cleanup event is distinct host evidence,
    // while the script result and Artifact identity remain identical.
    let production_cleanups = Arc::new(AtomicU64::new(0));
    let production_events = Arc::new(Mutex::new(Vec::new()));
    let production_runtime = runtime_with_session(
        Arc::clone(&production_cleanups),
        Arc::clone(&production_events),
        "production-like",
        false,
    );
    let production_report = production_runtime.link(&admitted)?.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded())
            .trace(TracePolicy::MetadataOnly),
    );
    assert_eq!(
        production_report.termination_reason(),
        TerminationReason::Completed
    );
    assert_eq!(production_report.artifact_digest, report.artifact_digest);
    assert_eq!(production_report.stdout, report.stdout);
    assert_eq!(production_cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(
        production_events
            .lock()
            .expect("production event lock")
            .as_slice(),
        ["production-like"]
    );

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

    // Cleanup faults are preserved as execution evidence instead of being
    // discarded by a convenience API. The resource is still finalized exactly
    // once and the report accounts for the failed cleanup separately.
    let failed_cleanups = Arc::new(AtomicU64::new(0));
    let failure_events = Arc::new(Mutex::new(Vec::new()));
    let failed_runtime = runtime_with_session(
        Arc::clone(&failed_cleanups),
        Arc::clone(&failure_events),
        "cleanup-failure",
        true,
    );
    let cleanup_failure = failed_runtime.link(&admitted)?.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded())
            .trace(TracePolicy::MetadataOnly),
    );
    assert_eq!(failed_cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(cleanup_failure.usage.resources_created, 1);
    assert_eq!(cleanup_failure.usage.resources_cleaned, 0);
    assert_eq!(cleanup_failure.usage.resource_cleanup_failures, 1);
    assert_eq!(
        failure_events
            .lock()
            .expect("failure event lock")
            .as_slice(),
        ["cleanup-failure"]
    );

    println!("artifact digest: {}", report.artifact_digest);
    println!("termination: {}", report.termination_reason().as_str());
    println!("steps: {}", report.usage.steps_consumed);
    print!("structured output:\n{}", report.stdout);
    println!(
        "cancelled termination: {} (steps: {})",
        cancelled.termination_reason().as_str(),
        cancelled.usage.steps_consumed
    );
    println!(
        "cleanup failure accounting: {} failed cleanup(s)",
        cleanup_failure.usage.resource_cleanup_failures
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
