//! Data-driven execution-boundary state-machine corpus.
//!
//! The fixtures prove that a verified, provider-neutral Artifact retains its
//! audit report and exactly-once runtime resource cleanup across every terminal
//! path. They deliberately use the reviewed `WireValue` Provider API: the
//! legacy dynamic adapter is not part of this product contract.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rsscript_sdk::{
    ProviderRegistry,
    artifact::ArtifactVerifier,
    compile::Compiler,
    operation::{CancellationToken, MonotonicDeadline},
    provider_api::{
        BlockingBehavior, CancellationBehavior, ExternalSymbol, FunctionSignature,
        ProviderCallMode, ProviderDescriptor, ProviderError, ProviderErrorMapping,
        ProviderFunction, ProviderFunctionDescriptor, ProviderResource, RUNTIME_ABI_VERSION,
        ResourceCleanupContract, WireInterpreterFn, WireValue,
    },
    runtime::{ExecutionRequest, RunLimits, Runtime},
};

const INTERFACE: &str = include_str!("corpus/execution_state/ops.rssi");
const SUCCESS: &str = include_str!("corpus/execution_state/success.rss");
const SCRIPT_ERROR: &str = include_str!("corpus/execution_state/script_error.rss");
const LOOP: &str = include_str!("corpus/execution_state/loop.rss");

#[derive(Debug, serde::Deserialize)]
struct Cases {
    case: Vec<Case>,
}

#[derive(Debug, serde::Deserialize)]
struct Case {
    name: String,
    source: String,
    termination: String,
    #[serde(default)]
    provider_failure: bool,
    #[serde(default)]
    cancel_after_register: bool,
    #[serde(default)]
    sleep_millis: u64,
    #[serde(default)]
    deadline_millis: u64,
    #[serde(default)]
    step_budget: u64,
    #[serde(default)]
    cleanup_failure: bool,
    resources_cleaned: u64,
    cleanup_failures: u64,
}

struct CountedResource {
    cleanups: Arc<AtomicU64>,
    fail: bool,
}

impl ProviderResource for CountedResource {
    fn cleanup(&mut self) -> Result<(), ProviderError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(ProviderError::internal(
                "intentional corpus cleanup failure",
            ))
        } else {
            Ok(())
        }
    }
}

fn source(name: &str) -> &'static str {
    match name {
        "success" => SUCCESS,
        "script_error" => SCRIPT_ERROR,
        "loop" => LOOP,
        other => panic!("unknown execution-state source `{other}`"),
    }
}

fn descriptor(symbol: ExternalSymbol) -> ProviderDescriptor {
    let signature = FunctionSignature {
        parameters: Vec::new(),
        result: "Unit".into(),
        asynchronous: false,
    };
    ProviderDescriptor {
        provider_id: "corpus.ops".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol,
            signature,
            entry: "open".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::RuntimeRegistered,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    }
}

fn registry(
    case: &Case,
    cancellation: Option<CancellationToken>,
    cleanups: Arc<AtomicU64>,
) -> ProviderRegistry {
    let symbol = ExternalSymbol::new("host.ops.open").expect("corpus symbol is valid");
    let descriptor = descriptor(symbol.clone());
    let signature = descriptor.functions[0].signature.clone();
    let mut providers = ProviderRegistry::default();
    let case_name = case.name.clone();
    let fail_after_register = case.provider_failure;
    let sleep = Duration::from_millis(case.sleep_millis);
    let cleanup_failure = case.cleanup_failure;
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new_contextual(move |context, arguments| {
                        if !arguments.is_empty() {
                            return Err(ProviderError::invalid_argument(
                                "corpus open accepts no arguments",
                            ));
                        }
                        context.register_resource(CountedResource {
                            cleanups: Arc::clone(&cleanups),
                            fail: cleanup_failure,
                        })?;
                        if let Some(cancellation) = &cancellation {
                            cancellation.cancel();
                        }
                        if !sleep.is_zero() {
                            std::thread::sleep(sleep);
                        }
                        if fail_after_register {
                            Err(ProviderError::internal(format!(
                                "intentional corpus Provider failure for {case_name}"
                            )))
                        } else {
                            Ok(WireValue::Unit)
                        }
                    }),
                },
            )]),
        )
        .expect("corpus Provider descriptor and canonical callable agree");
    providers
}

#[test]
fn execution_state_corpus_preserves_reports_and_exact_once_cleanup() {
    let cases: Cases = toml::from_str(include_str!("corpus/execution_state/cases.toml"))
        .expect("execution-state corpus manifest is valid TOML");

    for case in &cases.case {
        let built = Compiler
            .compile_with_interfaces(
                &[("main.rss", source(&case.source))],
                &[("ops.rssi", INTERFACE)],
            )
            .unwrap_or_else(|error| panic!("{} must compile: {error}", case.name));
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{} must verify: {error}", case.name))
            .admit_trusted_input();

        let cancellation = case.cancel_after_register.then(CancellationToken::new);
        let cleanups = Arc::new(AtomicU64::new(0));
        let mut limits = RunLimits::bounded();
        if let Some(cancellation) = cancellation.as_ref() {
            limits = limits.with_cancellation(cancellation.clone());
        }
        if case.deadline_millis > 0 {
            limits = limits.with_deadline(MonotonicDeadline::after(Duration::from_millis(
                case.deadline_millis,
            )));
        }
        if case.step_budget > 0 {
            limits = limits.with_step_budget(case.step_budget);
        }
        let report = Runtime::new(registry(case, cancellation, Arc::clone(&cleanups)))
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{} must link: {error}", case.name))
            .execute(ExecutionRequest::default().limits(limits));

        assert_eq!(
            report.termination_reason().as_str(),
            case.termination,
            "{} must preserve its declared terminal reason",
            case.name
        );
        assert_eq!(
            cleanups.load(Ordering::SeqCst),
            1,
            "{} must invoke Provider cleanup exactly once",
            case.name
        );
        assert_eq!(report.usage.resources_created, 1, "{}", case.name);
        assert_eq!(report.usage.resources_live_at_return, 0, "{}", case.name);
        assert_eq!(
            report.usage.resources_cleaned, case.resources_cleaned,
            "{}",
            case.name
        );
        assert_eq!(
            report.usage.resource_cleanup_failures, case.cleanup_failures,
            "{}",
            case.name
        );
        assert_eq!(
            report.failure().is_some(),
            case.termination != "completed",
            "{} must preserve whether the execution terminated normally",
            case.name
        );
    }
}
