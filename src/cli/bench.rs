use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rsscript::write_generated_rust_package;

use super::{
    cleanup_temp_dir, default_runtime_path, lower_cli_input_to_rust_package, print_usage,
    required_flag_value,
};

#[derive(Debug)]
struct BenchOptions<'a> {
    json: bool,
    iterations: usize,
    warmup: usize,
    path: &'a str,
    program_args: Vec<&'a str>,
}

#[derive(Debug)]
struct BenchResult {
    name: String,
    iterations: usize,
    warmup: usize,
    min: Duration,
    mean: Duration,
    max: Duration,
}

fn parse_bench_args(args: &[String]) -> Result<BenchOptions<'_>, String> {
    let mut json = false;
    let mut iterations = 10usize;
    let mut warmup = 1usize;
    let mut path = None;
    let mut program_args = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--" {
            program_args.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--iterations" {
            index += 1;
            iterations = parse_positive_usize(required_flag_value(args, index, "--iterations")?)?;
        } else if arg == "--warmup" {
            index += 1;
            warmup = parse_usize(required_flag_value(args, index, "--warmup")?)?;
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            program_args.push(arg.as_str());
        }
        index += 1;
    }

    let Some(path) = path else {
        return Err("missing benchmark path.".to_string());
    };
    Ok(BenchOptions {
        json,
        iterations,
        warmup,
        path,
        program_args,
    })
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = parse_usize(value)?;
    if parsed == 0 {
        return Err("benchmark iterations must be greater than zero.".to_string());
    }
    Ok(parsed)
}

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("invalid integer `{value}`: {error}"))
}

pub(crate) fn run_bench(args: &[String]) -> ExitCode {
    let options = match parse_bench_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(2);
        }
    };
    let result = match run_bench_inner(&options) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    if options.json {
        println!("{}", bench_result_json(&result));
    } else {
        println!("{}", bench_result_human(&result));
    }
    ExitCode::SUCCESS
}

fn run_bench_inner(options: &BenchOptions<'_>) -> Result<BenchResult, String> {
    let runtime_path = default_runtime_path()?;
    let package = lower_cli_input_to_rust_package(options.path, &runtime_path, false)
        .map_err(|code| format!("failed to lower benchmark input; exit code {code:?}"))?;
    if package.main_rs.is_none() {
        return Err("rss bench requires an executable `main`.".to_string());
    }
    let package_dir = bench_temp_dir(&package.package_name);
    write_generated_rust_package(&package_dir, &package)?;
    build_benchmark_binary(&package_dir)?;
    let binary = benchmark_binary_path(&package_dir, &package.package_name);

    for _ in 0..options.warmup {
        run_binary_once(&binary, &options.program_args)?;
    }

    let mut measurements = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let start = Instant::now();
        run_binary_once(&binary, &options.program_args)?;
        measurements.push(start.elapsed());
    }
    cleanup_temp_dir(&package_dir);
    Ok(summarize_measurements(
        package.package_name,
        options.iterations,
        options.warmup,
        &measurements,
    ))
}

fn build_benchmark_binary(package_dir: &Path) -> Result<(), String> {
    let output = Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--release")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", package_dir.join("target"))
        .output()
        .map_err(|error| format!("failed to run cargo build: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "benchmark build failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn run_binary_once(binary: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run {}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "benchmark iteration failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn benchmark_binary_path(package_dir: &Path, package_name: &str) -> PathBuf {
    let binary_name = if cfg!(windows) {
        format!("{package_name}.exe")
    } else {
        package_name.to_string()
    };
    package_dir.join("target").join("release").join(binary_name)
}

fn summarize_measurements(
    name: String,
    iterations: usize,
    warmup: usize,
    measurements: &[Duration],
) -> BenchResult {
    let min = measurements.iter().copied().min().unwrap_or_default();
    let max = measurements.iter().copied().max().unwrap_or_default();
    let total_nanos: u128 = measurements.iter().map(Duration::as_nanos).sum();
    let mean = Duration::from_nanos((total_nanos / measurements.len() as u128) as u64);
    BenchResult {
        name,
        iterations,
        warmup,
        min,
        mean,
        max,
    }
}

fn bench_result_human(result: &BenchResult) -> String {
    format!(
        "bench {} iterations={} warmup={} min_ms={:.3} mean_ms={:.3} max_ms={:.3}",
        result.name,
        result.iterations,
        result.warmup,
        millis(result.min),
        millis(result.mean),
        millis(result.max)
    )
}

fn bench_result_json(result: &BenchResult) -> String {
    serde_json::json!({
        "name": result.name,
        "iterations": result.iterations,
        "warmup": result.warmup,
        "min_ms": millis(result.min),
        "mean_ms": millis(result.mean),
        "max_ms": millis(result.max),
    })
    .to_string()
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn bench_temp_dir(package_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::var_os("RSSCRIPT_TEMP_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "rsscript-bench-{package_name}-{}-{nanos}",
            std::process::id()
        ))
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_bench_args_accepts_path_options_and_program_args() {
        let values = args(&[
            "--json",
            "--iterations",
            "3",
            "--warmup",
            "2",
            "examples/scripts/basic/hello.rss",
            "--",
            "arg",
        ]);
        let options = super::parse_bench_args(&values).expect("bench args should parse");

        assert!(options.json);
        assert_eq!(options.iterations, 3);
        assert_eq!(options.warmup, 2);
        assert_eq!(options.path, "examples/scripts/basic/hello.rss");
        assert_eq!(options.program_args, vec!["arg"]);
    }

    #[test]
    fn parse_bench_args_rejects_missing_path_and_zero_iterations() {
        assert_eq!(
            super::parse_bench_args(&args(&[])).expect_err("path should be required"),
            "missing benchmark path."
        );
        assert_eq!(
            super::parse_bench_args(&args(&["--iterations", "0", "bench.rss"]))
                .expect_err("zero iterations should fail"),
            "benchmark iterations must be greater than zero."
        );
    }
}
