#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use rsscript_sdk::{CancellationToken, Compiler, ProviderRegistry, RunLimits, Runtime};
use serde::{Deserialize, Serialize};

const METRICS_SCHEMA: &str = "rsscript.core_metrics.v1";
const SLO_SCHEMA: &str = "rsscript.core_slo.v1";
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
    pre_cancel_rejection: MetricDistribution,
    artifact_bytes: usize,
    execution_steps: u64,
    execution_allocated_bytes: usize,
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
    pre_cancel_rejection: f64,
}

#[derive(Debug)]
struct Arguments {
    iterations: usize,
    output: Option<PathBuf>,
    check: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("core-metrics") => run_core_metrics(parse_arguments(arguments)?),
        _ => Err("usage: cargo run -p rsscript-xtask --release -- core-metrics [--iterations N] [--output FILE] [--check SLO]".into()),
    }
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

    // Warm each path before collecting distributions so the report measures
    // steady Core behavior rather than one-time process and allocator setup.
    for _ in 0..3 {
        let _ = compiler.check("core-metrics.rss", WORKLOAD);
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        let bytes = package.bytecode()?;
        let loaded = compiler.load_verified(&bytes)?;
        Runtime::default()
            .link(&loaded)?
            .run(Vec::<String>::new())?;
    }

    let mut check = Vec::with_capacity(arguments.iterations);
    let mut compile = Vec::with_capacity(arguments.iterations);
    let mut verify = Vec::with_capacity(arguments.iterations);
    let mut execute = Vec::with_capacity(arguments.iterations);
    let mut cancel = Vec::with_capacity(arguments.iterations);
    let mut artifact_bytes = 0;
    let mut execution_steps = 0;
    let mut execution_allocated_bytes = 0;

    let cancellation_package = compiler.compile("cancel.rss", CANCELLATION_WORKLOAD)?;
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

        let bytes = package.bytecode()?;
        artifact_bytes = bytes.len();
        let started = Instant::now();
        let loaded = compiler.load_verified(&bytes)?;
        verify.push(elapsed_ms(started));

        let runtime = Runtime::default();
        let linked = runtime.link(&loaded)?;
        let started = Instant::now();
        let report = linked.run(Vec::<String>::new())?;
        execute.push(elapsed_ms(started));
        execution_steps = report.usage.steps_consumed;
        execution_allocated_bytes = report.usage.allocation_bytes_consumed;

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = Runtime::new(
            ProviderRegistry::default(),
            RunLimits {
                cancellation: Some(cancellation),
                ..RunLimits::unbounded_for_trusted_host()
            },
        );
        let linked = runtime.link(&cancellation_package)?;
        let started = Instant::now();
        let report = linked.execute(Vec::<String>::new());
        cancel.push(elapsed_ms(started));
        if report.termination_reason.as_str() != "cancelled" {
            return Err(format!(
                "pre-cancel workload terminated as {}",
                report.termination_reason.as_str()
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
        pre_cancel_rejection: distribution(cancel),
        artifact_bytes,
        execution_steps,
        execution_allocated_bytes,
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
            pre_cancel_rejection: distribution(),
            artifact_bytes: 1,
            execution_steps: 1,
            execution_allocated_bytes: 0,
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
