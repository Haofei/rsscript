#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use rsscript_provider_api::CancellationToken;
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
    NativeInterpreterFn, ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
    ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION,
    ResourceCleanupContract,
};
use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};
use serde::{Deserialize, Serialize};

const METRICS_SCHEMA: &str = "rsscript.core_metrics.v1";
const SLO_SCHEMA: &str = "rsscript.core_slo.v1";
const MIGRATION_STATUS_SCHEMA: &str = "rsscript.migration_status.v1";
const WORKLOAD: &str = r#"
fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = (total + index) % 1000000
        index = index + 1
    }
    return total
}
"#;
const CANCELLATION_WORKLOAD: &str = r#"
fn main() -> Int {
    let mut value = 0
    while true {
        value = value + 1
    }
    return value
}
"#;
const PROVIDER_WORKLOAD: &str = r#"
module metrics
use host.metrics.*

fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = echo(value: index)
        index = index + 1
    }
    return total
}
"#;
const PROVIDER_INTERFACE: &str = "module host.metrics\npub fn echo(value: read Int) -> Int\n";

#[derive(Debug, Serialize)]
struct MetricDistribution {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
struct CoreMetrics {
    schema: &'static str,
    iterations: usize,
    environment: Environment,
    check: MetricDistribution,
    compile: MetricDistribution,
    artifact_verify: MetricDistribution,
    vm_execute: MetricDistribution,
    provider_execute: MetricDistribution,
    pre_cancel_rejection: MetricDistribution,
    artifact_bytes: usize,
    execution_steps: u64,
    execution_allocated_bytes: usize,
    provider_calls: u64,
    provider_request_bytes: usize,
    provider_response_bytes: usize,
    provider_total_duration_ns: u64,
    provider_max_duration_ns: u64,
}

#[derive(Debug, Serialize)]
struct Environment {
    os: &'static str,
    arch: &'static str,
    profile: &'static str,
    git_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreSlo {
    schema: String,
    max_p95_ms: LatencySlo,
    max_artifact_bytes: usize,
    max_execution_steps: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LatencySlo {
    check: f64,
    compile: f64,
    artifact_verify: f64,
    vm_execute: f64,
    provider_execute: f64,
    pre_cancel_rejection: f64,
}

#[derive(Debug)]
struct Arguments {
    iterations: usize,
    output: Option<PathBuf>,
    check: Option<PathBuf>,
}

#[derive(Debug)]
struct MigrationStatusArguments {
    json: bool,
    open_only: bool,
    required_items: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationStatus {
    schema: &'static str,
    source: &'static str,
    completed: usize,
    open: usize,
    items: Vec<MigrationStatusItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationStatusItem {
    id: String,
    title: String,
    completed: bool,
    line: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("core-metrics") => run_core_metrics(parse_arguments(arguments)?),
        Some("migration-status") => run_migration_status(parse_migration_status_arguments(arguments)?),
        _ => Err(
            "usage:\n  cargo run -p rsscript-xtask --release -- core-metrics [--iterations N] [--output FILE] [--check SLO]\n  cargo run -p rsscript-xtask -- migration-status [--json] [--open] [--require ITEM]"
                .into(),
        ),
    }
}

fn parse_migration_status_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<MigrationStatusArguments, Box<dyn Error>> {
    let mut parsed = MigrationStatusArguments {
        json: false,
        open_only: false,
        required_items: Vec::new(),
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => parsed.json = true,
            "--open" => parsed.open_only = true,
            "--require" => parsed.required_items.push(
                arguments
                    .next()
                    .ok_or("--require requires a migration item ID")?,
            ),
            _ => return Err(format!("unknown migration-status argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn run_migration_status(arguments: MigrationStatusArguments) -> Result<(), Box<dyn Error>> {
    let path = workspace_root().join("docs/architecture/migration-baseline.md");
    let status = migration_status(&fs::read_to_string(&path)?)?;
    for required in &arguments.required_items {
        let item = status
            .items
            .iter()
            .find(|item| item.id == *required)
            .ok_or_else(|| format!("migration item `{required}` is not declared"))?;
        if !item.completed {
            return Err(format!(
                "migration item `{required}` remains open (line {}: {})",
                item.line, item.title
            )
            .into());
        }
    }

    if arguments.json {
        let mut output = status;
        if arguments.open_only {
            output.items.retain(|item| !item.completed);
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!(
        "Migration checklist: {} complete, {} open ({})",
        status.completed,
        status.open,
        path.display()
    );
    for item in status
        .items
        .iter()
        .filter(|item| {
            arguments.required_items.is_empty()
                || arguments
                    .required_items
                    .iter()
                    .any(|required| required == &item.id)
        })
        .filter(|item| !arguments.open_only || !item.completed)
    {
        let marker = if item.completed { 'x' } else { ' ' };
        println!(
            "- [{marker}] {} — {} (line {})",
            item.id, item.title, item.line
        );
    }
    Ok(())
}

fn migration_status(document: &str) -> Result<MigrationStatus, Box<dyn Error>> {
    let mut items = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let lines = document.lines().collect::<Vec<_>>();
    let mut offset = 0;
    while offset < lines.len() {
        let line = lines[offset];
        let trimmed = line.trim_start();
        let (completed, label) = match trimmed
            .strip_prefix("- [x] **")
            .map(|label| (true, label))
            .or_else(|| trimmed.strip_prefix("- [ ] **").map(|label| (false, label)))
        {
            Some(item) => item,
            None => {
                offset += 1;
                continue;
            }
        };
        let start_line = offset + 1;
        let mut label = label.to_string();
        while !label.contains("**") {
            offset += 1;
            let continuation = lines
                .get(offset)
                .ok_or_else(|| format!("unterminated migration item on line {start_line}"))?;
            label.push(' ');
            label.push_str(continuation.trim());
        }
        let (label, _) = label
            .split_once("**")
            .expect("migration item loop must stop at closing bold marker");
        let (id, title) = label
            .split_once(" — ")
            .ok_or_else(|| format!("migration item on line {start_line} is missing ` — `"))?;
        if id.is_empty() || title.is_empty() {
            return Err(format!("invalid migration item on line {start_line}").into());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate migration item `{id}`").into());
        }
        items.push(MigrationStatusItem {
            id: id.to_string(),
            title: title.to_string(),
            completed,
            line: start_line,
        });
        offset += 1;
    }
    if items.is_empty() {
        return Err("migration checklist contains no parseable items".into());
    }
    let completed = items.iter().filter(|item| item.completed).count();
    Ok(MigrationStatus {
        schema: MIGRATION_STATUS_SCHEMA,
        source: "docs/architecture/migration-baseline.md",
        completed,
        open: items.len() - completed,
        items,
    })
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask must remain under the workspace tools directory")
        .to_path_buf()
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Arguments, Box<dyn Error>> {
    let mut parsed = Arguments {
        iterations: 20,
        output: None,
        check: None,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => {
                parsed.iterations = arguments
                    .next()
                    .ok_or("--iterations requires a value")?
                    .parse()?;
                if parsed.iterations < 2 {
                    return Err("--iterations must be at least 2".into());
                }
            }
            "--output" => {
                parsed.output = Some(PathBuf::from(
                    arguments.next().ok_or("--output requires a path")?,
                ));
            }
            "--check" => {
                parsed.check = Some(PathBuf::from(
                    arguments.next().ok_or("--check requires a path")?,
                ));
            }
            _ => return Err(format!("unknown core-metrics argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn run_core_metrics(arguments: Arguments) -> Result<(), Box<dyn Error>> {
    let compiler = Compiler;
    let provider_package = compiler.compile_with_interfaces(
        &[("provider-metrics.rss", PROVIDER_WORKLOAD)],
        &[("provider-metrics.rssi", PROVIDER_INTERFACE)],
    )?;
    let provider_package = ArtifactVerifier
        .verify(provider_package)?
        .admit_trusted_input();
    let provider_runtime = metrics_provider_runtime()?;

    // Warm each path before collecting distributions so the report measures
    // steady Core behavior rather than one-time process and allocator setup.
    for _ in 0..3 {
        let _ = compiler.check("core-metrics.rss", WORKLOAD);
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        let bytes = package.bundle_bytes()?;
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        Runtime::default()
            .link(&loaded)?
            .execute(ExecutionRequest::default());
        provider_runtime
            .link(&provider_package)?
            .execute(ExecutionRequest::default());
    }

    let mut check = Vec::with_capacity(arguments.iterations);
    let mut compile = Vec::with_capacity(arguments.iterations);
    let mut verify = Vec::with_capacity(arguments.iterations);
    let mut execute = Vec::with_capacity(arguments.iterations);
    let mut provider_execute = Vec::with_capacity(arguments.iterations);
    let mut cancel = Vec::with_capacity(arguments.iterations);
    let mut artifact_bytes = 0;
    let mut execution_steps = 0;
    let mut execution_allocated_bytes = 0;
    let mut provider_calls = 0;
    let mut provider_request_bytes = 0;
    let mut provider_response_bytes = 0;
    let mut provider_total_duration_ns = 0;
    let mut provider_max_duration_ns = 0;

    let cancellation_package = ArtifactVerifier
        .verify(compiler.compile("cancel.rss", CANCELLATION_WORKLOAD)?)?
        .admit_trusted_input();
    for _ in 0..arguments.iterations {
        check.push(measure(|| {
            let diagnostics = compiler.check("core-metrics.rss", WORKLOAD);
            assert!(
                diagnostics.is_empty(),
                "Core metric workload must remain valid"
            );
        }));

        let started = Instant::now();
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        compile.push(elapsed_ms(started));

        let bytes = package.bundle_bytes()?;
        artifact_bytes = bytes.len();
        let started = Instant::now();
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        verify.push(elapsed_ms(started));

        let runtime = Runtime::default();
        let linked = runtime.link(&loaded)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        execute.push(elapsed_ms(started));
        execution_steps = report.usage.steps_consumed;
        execution_allocated_bytes = report.usage.allocation_bytes_consumed;

        let linked = provider_runtime.link(&provider_package)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        provider_execute.push(elapsed_ms(started));
        let summary = report
            .telemetry
            .provider_functions
            .first()
            .ok_or("Provider metric workload did not record its external call")?;
        provider_calls = summary.calls;
        provider_request_bytes = summary.request_bytes;
        provider_response_bytes = summary.response_bytes;
        provider_total_duration_ns = summary.total_duration_ns;
        provider_max_duration_ns = summary.max_duration_ns;

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = Runtime::new(ProviderRegistry::default());
        let linked = runtime.link(&cancellation_package)?;
        let started = Instant::now();
        let report = linked.execute(
            ExecutionRequest::default()
                .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(cancellation)),
        );
        cancel.push(elapsed_ms(started));
        if report.termination_reason().as_str() != "cancelled" {
            return Err(format!(
                "pre-cancel workload terminated as {}",
                report.termination_reason().as_str()
            )
            .into());
        }
    }

    let metrics = CoreMetrics {
        schema: METRICS_SCHEMA,
        iterations: arguments.iterations,
        environment: Environment {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            git_revision: std::env::var("GITHUB_SHA").ok(),
        },
        check: distribution(check),
        compile: distribution(compile),
        artifact_verify: distribution(verify),
        vm_execute: distribution(execute),
        provider_execute: distribution(provider_execute),
        pre_cancel_rejection: distribution(cancel),
        artifact_bytes,
        execution_steps,
        execution_allocated_bytes,
        provider_calls,
        provider_request_bytes,
        provider_response_bytes,
        provider_total_duration_ns,
        provider_max_duration_ns,
    };
    let json = serde_json::to_string_pretty(&metrics)?;
    println!("{json}");
    if let Some(output) = arguments.output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, format!("{json}\n"))?;
    }
    if let Some(slo_path) = arguments.check {
        check_slo(&metrics, &slo_path)?;
    }
    Ok(())
}

fn metrics_provider_runtime() -> Result<Runtime, Box<dyn Error>> {
    let symbol = ExternalSymbol::new("host.metrics.echo")
        .map_err(|_| "invalid built-in metrics provider symbol")?;
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect: DataEffect::Read,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Int".into(),
        asynchronous: false,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "rsscript.metrics".into(),
        provider_version: "1.0.0".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "echo".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers.register(
        &descriptor,
        BTreeMap::from([(
            symbol,
            ProviderFunction {
                signature,
                callable: NativeInterpreterFn::new(|mut values| {
                    values.pop().filter(|_| values.is_empty()).ok_or_else(|| {
                        ProviderError::invalid_argument("metrics echo expects one argument")
                    })
                }),
            },
        )]),
    )?;
    Ok(Runtime::new(providers))
}

fn measure(action: impl FnOnce()) -> f64 {
    let started = Instant::now();
    action();
    elapsed_ms(started)
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn distribution(mut samples: Vec<f64>) -> MetricDistribution {
    samples.sort_by(f64::total_cmp);
    MetricDistribution {
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        max_ms: *samples.last().expect("at least two metric samples"),
    }
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    let index = ((samples.len() - 1) as f64 * quantile).ceil() as usize;
    samples[index]
}

fn check_slo(metrics: &CoreMetrics, path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let slo: CoreSlo = serde_json::from_slice(&fs::read(path)?)?;
    if slo.schema != SLO_SCHEMA {
        return Err(format!("unsupported Core SLO schema: {}", slo.schema).into());
    }
    let latency = [
        ("check", metrics.check.p95_ms, slo.max_p95_ms.check),
        ("compile", metrics.compile.p95_ms, slo.max_p95_ms.compile),
        (
            "artifact_verify",
            metrics.artifact_verify.p95_ms,
            slo.max_p95_ms.artifact_verify,
        ),
        (
            "vm_execute",
            metrics.vm_execute.p95_ms,
            slo.max_p95_ms.vm_execute,
        ),
        (
            "provider_execute",
            metrics.provider_execute.p95_ms,
            slo.max_p95_ms.provider_execute,
        ),
        (
            "pre_cancel_rejection",
            metrics.pre_cancel_rejection.p95_ms,
            slo.max_p95_ms.pre_cancel_rejection,
        ),
    ];
    let mut failures = latency
        .into_iter()
        .filter(|(_, observed, maximum)| observed > maximum)
        .map(|(name, observed, maximum)| {
            format!("{name} p95 {observed:.3} ms exceeds {maximum:.3} ms")
        })
        .collect::<Vec<_>>();
    if metrics.artifact_bytes > slo.max_artifact_bytes {
        failures.push(format!(
            "artifact size {} exceeds {} bytes",
            metrics.artifact_bytes, slo.max_artifact_bytes
        ));
    }
    if metrics.execution_steps > slo.max_execution_steps {
        failures.push(format!(
            "execution steps {} exceed {}",
            metrics.execution_steps, slo.max_execution_steps
        ));
    }
    if failures.is_empty() {
        println!("Core SLO check passed: {}", path.display());
        Ok(())
    } else {
        Err(format!("Core SLO check failed:\n- {}", failures.join("\n- ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_a_conservative_nearest_rank() {
        let samples = (1..=20).map(f64::from).collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 0.50), 11.0);
        assert_eq!(percentile(&samples, 0.95), 20.0);
    }

    #[test]
    fn argument_parser_rejects_single_sample_runs() {
        let error = parse_arguments(["--iterations".into(), "1".into()].into_iter())
            .expect_err("one iteration cannot produce a useful distribution");
        assert!(error.to_string().contains("at least 2"));
    }

    #[test]
    fn migration_status_parses_nested_and_wrapped_checklist_items() {
        let status = migration_status(
            "- [x] **G01 — Completed parent.**\n  - [ ] **G01.1 — Open child\n    item.**\n",
        )
        .expect("well-formed checklist");
        assert_eq!(status.completed, 1);
        assert_eq!(status.open, 1);
        assert_eq!(
            status.items,
            vec![
                MigrationStatusItem {
                    id: "G01".into(),
                    title: "Completed parent.".into(),
                    completed: true,
                    line: 1,
                },
                MigrationStatusItem {
                    id: "G01.1".into(),
                    title: "Open child item.".into(),
                    completed: false,
                    line: 2,
                },
            ]
        );
    }

    #[test]
    fn migration_status_rejects_duplicate_item_ids() {
        let error = migration_status("- [x] **G01 — First.**\n- [ ] **G01 — Second.**\n")
            .expect_err("duplicate IDs make a checklist gate ambiguous");
        assert!(error.to_string().contains("duplicate migration item `G01`"));
    }

    #[test]
    fn published_migration_checklist_is_machine_readable() {
        let status = migration_status(include_str!(
            "../../../docs/architecture/migration-baseline.md"
        ))
        .expect("published migration checklist must remain parseable");
        assert!(status.items.len() > 100);
        assert!(status.items.iter().any(|item| item.id == "S02"));
        assert!(status.items.iter().any(|item| item.id == "A09"));
    }

    #[test]
    fn metrics_shape_matches_the_published_schema() {
        let distribution = || MetricDistribution {
            p50_ms: 1.0,
            p95_ms: 2.0,
            max_ms: 3.0,
        };
        let metrics = CoreMetrics {
            schema: METRICS_SCHEMA,
            iterations: 20,
            environment: Environment {
                os: "test",
                arch: "test",
                profile: "release",
                git_revision: None,
            },
            check: distribution(),
            compile: distribution(),
            artifact_verify: distribution(),
            vm_execute: distribution(),
            provider_execute: distribution(),
            pre_cancel_rejection: distribution(),
            artifact_bytes: 1,
            execution_steps: 1,
            execution_allocated_bytes: 0,
            provider_calls: 1000,
            provider_request_bytes: 8000,
            provider_response_bytes: 8000,
            provider_total_duration_ns: 1000,
            provider_max_duration_ns: 1,
        };
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/rsscript.core_metrics.v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::to_value(metrics).unwrap();
        assert!(validator.is_valid(&instance));
    }
}
