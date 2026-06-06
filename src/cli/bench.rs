use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rsscript::{
    EvalError, format_diagnostics_human, reg_vm_compile_source, reg_vm_eval_source_main_with_args,
    write_generated_rust_package,
};

use super::{
    cleanup_temp_dir, default_runtime_path, is_package_directory, lower_cli_input_to_rust_package,
    print_usage, required_flag_value,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchMode {
    Eval,
    Vm,
    VmInternal,
    Run,
    Release,
    ReleaseInternal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchVm {
    Reg,
}

impl BenchMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "eval" => Ok(Self::Eval),
            "vm" => Ok(Self::Vm),
            "vm-internal" => Ok(Self::VmInternal),
            "run" => Ok(Self::Run),
            "release" => Ok(Self::Release),
            "release-internal" => Ok(Self::ReleaseInternal),
            _ => Err(format!(
                "invalid benchmark mode `{value}`; expected eval, vm, vm-internal, run, release, or release-internal."
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Eval => "eval",
            Self::Vm => "vm",
            Self::VmInternal => "vm-internal",
            Self::Run => "run",
            Self::Release => "release",
            Self::ReleaseInternal => "release-internal",
        }
    }

    fn cargo_profile_dir(self) -> &'static str {
        match self {
            Self::Eval | Self::Vm | Self::VmInternal => {
                unreachable!("eval/vm mode does not build a cargo profile")
            }
            Self::Run => "debug",
            Self::Release | Self::ReleaseInternal => "release",
        }
    }
}

impl BenchVm {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reg" => Ok(Self::Reg),
            _ => Err(format!("invalid benchmark VM `{value}`; expected reg.")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Reg => "reg",
        }
    }
}

#[derive(Debug)]
struct BenchOptions<'a> {
    json: bool,
    mode: BenchMode,
    vm: BenchVm,
    iterations: usize,
    warmup: usize,
    path: &'a str,
    program_args: Vec<&'a str>,
}

#[derive(Debug)]
struct BenchResult {
    name: String,
    mode: BenchMode,
    vm: BenchVm,
    iterations: usize,
    warmup: usize,
    min: Duration,
    mean: Duration,
    max: Duration,
}

fn parse_bench_args(args: &[String]) -> Result<BenchOptions<'_>, String> {
    let mut json = false;
    let mut mode = BenchMode::Release;
    let mut vm = BenchVm::Reg;
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
        } else if arg == "--mode" {
            index += 1;
            let value = required_flag_value(args, index, "--mode")?;
            match value {
                "reg-vm" => {
                    mode = BenchMode::Vm;
                    vm = BenchVm::Reg;
                }
                "reg-vm-internal" => {
                    mode = BenchMode::VmInternal;
                    vm = BenchVm::Reg;
                }
                _ => mode = BenchMode::parse(value)?,
            }
        } else if arg == "--vm" {
            index += 1;
            vm = BenchVm::parse(required_flag_value(args, index, "--vm")?)?;
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
        mode,
        vm,
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
    match options.mode {
        BenchMode::Eval => run_eval_bench(options),
        BenchMode::Vm => run_reg_vm_bench(options),
        BenchMode::VmInternal => run_reg_vm_internal_bench(options),
        BenchMode::Run | BenchMode::Release | BenchMode::ReleaseInternal => {
            run_generated_bench(options)
        }
    }
}

fn run_eval_bench(options: &BenchOptions<'_>) -> Result<BenchResult, String> {
    if is_package_directory(options.path) {
        return Err("rss bench --mode eval only supports single-file inputs.".to_string());
    }
    let source = std::fs::read_to_string(options.path)
        .map_err(|error| format!("failed to read {}: {error}", options.path))?;
    for _ in 0..options.warmup {
        eval_source_once(options.path, &source, &options.program_args)?;
    }

    let mut measurements = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let start = Instant::now();
        eval_source_once(options.path, &source, &options.program_args)?;
        measurements.push(start.elapsed());
    }
    Ok(summarize_measurements(
        benchmark_name(options.path),
        options.mode,
        options.vm,
        options.iterations,
        options.warmup,
        &measurements,
    ))
}

fn eval_source_once(path: &str, source: &str, args: &[&str]) -> Result<(), String> {
    reg_vm_eval_source_main_with_args(path, source, args.iter().copied()).map_err(|error| {
        match error {
            EvalError::Diagnostics(diagnostics) => format_diagnostics_human(&diagnostics),
            EvalError::Runtime(error) => error,
        }
    })?;
    Ok(())
}

fn run_reg_vm_bench(options: &BenchOptions<'_>) -> Result<BenchResult, String> {
    if is_package_directory(options.path) {
        return Err("rss bench --mode vm only supports single-file inputs.".to_string());
    }
    let source = std::fs::read_to_string(options.path)
        .map_err(|error| format!("failed to read {}: {error}", options.path))?;
    for _ in 0..options.warmup {
        reg_vm_source_once(options.path, &source, &options.program_args)?;
    }

    let mut measurements = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let start = Instant::now();
        reg_vm_source_once(options.path, &source, &options.program_args)?;
        measurements.push(start.elapsed());
    }
    Ok(summarize_measurements(
        benchmark_name(options.path),
        options.mode,
        options.vm,
        options.iterations,
        options.warmup,
        &measurements,
    ))
}

fn reg_vm_source_once(path: &str, source: &str, args: &[&str]) -> Result<(), String> {
    reg_vm_eval_source_main_with_args(path, source, args.iter().copied()).map_err(|error| {
        match error {
            EvalError::Diagnostics(diagnostics) => format_diagnostics_human(&diagnostics),
            EvalError::Runtime(error) => error,
        }
    })?;
    Ok(())
}

fn run_reg_vm_internal_bench(options: &BenchOptions<'_>) -> Result<BenchResult, String> {
    if is_package_directory(options.path) {
        return Err("rss bench --mode vm-internal only supports single-file inputs.".to_string());
    }
    let source = std::fs::read_to_string(options.path)
        .map_err(|error| format!("failed to read {}: {error}", options.path))?;
    let executable = reg_vm_compile_source(options.path, &source).map_err(|error| match error {
        EvalError::Diagnostics(diagnostics) => format_diagnostics_human(&diagnostics),
        EvalError::Runtime(error) => error,
    })?;
    for _ in 0..options.warmup {
        executable
            .eval_main_with_args(options.program_args.iter().copied())
            .map_err(|error| match error {
                EvalError::Diagnostics(diagnostics) => format_diagnostics_human(&diagnostics),
                EvalError::Runtime(error) => error,
            })?;
    }

    let mut measurements = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let start = Instant::now();
        executable
            .eval_main_with_args(options.program_args.iter().copied())
            .map_err(|error| match error {
                EvalError::Diagnostics(diagnostics) => format_diagnostics_human(&diagnostics),
                EvalError::Runtime(error) => error,
            })?;
        measurements.push(start.elapsed());
    }
    Ok(summarize_measurements(
        benchmark_name(options.path),
        options.mode,
        options.vm,
        options.iterations,
        options.warmup,
        &measurements,
    ))
}

fn run_generated_bench(options: &BenchOptions<'_>) -> Result<BenchResult, String> {
    let runtime_path = default_runtime_path()?;
    let mut package = lower_cli_input_to_rust_package(options.path, &runtime_path, false)
        .map_err(|code| format!("failed to lower benchmark input; exit code {code:?}"))?;
    if package.main_rs.is_none() {
        return Err("rss bench requires an executable `main`.".to_string());
    }
    let package_dir = bench_temp_dir(&package.package_name);
    if options.mode == BenchMode::ReleaseInternal {
        let main_rs = package
            .main_rs
            .as_ref()
            .ok_or_else(|| "rss bench requires an executable `main`.".to_string())?;
        package.main_rs = Some(internal_benchmark_main(main_rs)?);
    }
    write_generated_rust_package(&package_dir, &package)?;
    build_benchmark_binary(&package_dir, options.mode)?;
    let binary = benchmark_binary_path(&package_dir, &package.package_name, options.mode);

    if options.mode == BenchMode::ReleaseInternal {
        let result = run_internal_benchmark_binary(&binary, options)?;
        cleanup_temp_dir(&package_dir);
        return Ok(result);
    }

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
        options.mode,
        options.vm,
        options.iterations,
        options.warmup,
        &measurements,
    ))
}

fn build_benchmark_binary(package_dir: &Path, mode: BenchMode) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", package_dir.join("target"));
    if matches!(mode, BenchMode::Release | BenchMode::ReleaseInternal) {
        command.arg("--release");
    }
    let output = command
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

fn run_internal_benchmark_binary(
    binary: &Path,
    options: &BenchOptions<'_>,
) -> Result<BenchResult, String> {
    let output = Command::new(binary)
        .args(&options.program_args)
        .env("RSSCRIPT_BENCH_ITERATIONS", options.iterations.to_string())
        .env("RSSCRIPT_BENCH_WARMUP", options.warmup.to_string())
        .env("RSSCRIPT_BENCH_SILENCE_LOG", "1")
        .output()
        .map_err(|error| format!("failed to run {}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "benchmark iteration failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let marker = "RSSCRIPT_BENCH_JSON:";
    let json = stderr
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .ok_or_else(|| format!("internal benchmark did not emit `{marker}` marker."))?;
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("internal benchmark emitted invalid JSON: {error}"))?;
    let nanos = |field: &str| -> Result<Duration, String> {
        let value = value
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("internal benchmark JSON is missing `{field}`."))?;
        Ok(Duration::from_nanos(value))
    };
    Ok(BenchResult {
        name: benchmark_name(options.path),
        mode: options.mode,
        vm: options.vm,
        iterations: options.iterations,
        warmup: options.warmup,
        min: nanos("min_nanos")?,
        mean: nanos("mean_nanos")?,
        max: nanos("max_nanos")?,
    })
}

fn internal_benchmark_main(main_rs: &str) -> Result<String, String> {
    let calls = main_rs
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("//")
                && *line != "fn main() {"
                && *line != "}"
                && !line.starts_with("rsscript_runtime::install_runtime_diagnostic_panic_hook()")
        })
        .map(|line| format!("        {line}\n"))
        .collect::<String>();
    if calls.trim().is_empty() {
        return Err("generated main did not contain a runnable call.".to_string());
    }
    Ok(format!(
        concat!(
            "// Generated by RSScript. Edit the .rss source instead.\n",
            "// Internal benchmark harness for RSScript.\n\n",
            "fn main() {{\n",
            "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
            "    let iterations = std::env::var(\"RSSCRIPT_BENCH_ITERATIONS\")\n",
            "        .ok()\n",
            "        .and_then(|value| value.parse::<usize>().ok())\n",
            "        .unwrap_or(1);\n",
            "    let warmup = std::env::var(\"RSSCRIPT_BENCH_WARMUP\")\n",
            "        .ok()\n",
            "        .and_then(|value| value.parse::<usize>().ok())\n",
            "        .unwrap_or(0);\n",
            "    for _ in 0..warmup {{\n",
            "{}",
            "    }}\n",
            "    let mut measurements = Vec::with_capacity(iterations);\n",
            "    for _ in 0..iterations {{\n",
            "        let start = std::time::Instant::now();\n",
            "{}",
            "        measurements.push(start.elapsed());\n",
            "    }}\n",
            "    let min_nanos = measurements.iter().map(std::time::Duration::as_nanos).min().unwrap_or(0);\n",
            "    let max_nanos = measurements.iter().map(std::time::Duration::as_nanos).max().unwrap_or(0);\n",
            "    let total_nanos: u128 = measurements.iter().map(std::time::Duration::as_nanos).sum();\n",
            "    let mean_nanos = if measurements.is_empty() {{ 0 }} else {{ total_nanos / measurements.len() as u128 }};\n",
            "    eprintln!(\n",
            "        \"RSSCRIPT_BENCH_JSON:{{{{\\\"min_nanos\\\":{{}},\\\"mean_nanos\\\":{{}},\\\"max_nanos\\\":{{}}}}}}\",\n",
            "        min_nanos,\n",
            "        mean_nanos,\n",
            "        max_nanos\n",
            "    );\n",
            "}}\n"
        ),
        calls, calls
    ))
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

fn benchmark_binary_path(package_dir: &Path, package_name: &str, mode: BenchMode) -> PathBuf {
    let binary_name = if cfg!(windows) {
        format!("{package_name}.exe")
    } else {
        package_name.to_string()
    };
    package_dir
        .join("target")
        .join(mode.cargo_profile_dir())
        .join(binary_name)
}

fn summarize_measurements(
    name: String,
    mode: BenchMode,
    vm: BenchVm,
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
        mode,
        vm,
        iterations,
        warmup,
        min,
        mean,
        max,
    }
}

fn bench_result_human(result: &BenchResult) -> String {
    format!(
        "bench {} mode={} vm={} iterations={} warmup={} min_ms={:.3} mean_ms={:.3} max_ms={:.3}",
        result.name,
        result.mode.as_str(),
        result.vm.as_str(),
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
        "mode": result.mode.as_str(),
        "vm": result.vm.as_str(),
        "iterations": result.iterations,
        "warmup": result.warmup,
        "min_ms": millis(result.min),
        "mean_ms": millis(result.mean),
        "max_ms": millis(result.max),
    })
    .to_string()
}

fn benchmark_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("rsscript-bench")
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
            "--mode",
            "vm-internal",
            "--vm",
            "reg",
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
        assert_eq!(options.mode, super::BenchMode::VmInternal);
        assert_eq!(options.vm, super::BenchVm::Reg);
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
        assert_eq!(
            super::parse_bench_args(&args(&["--mode", "fast", "bench.rss"]))
                .expect_err("unknown mode should fail"),
            "invalid benchmark mode `fast`; expected eval, vm, vm-internal, run, release, or release-internal."
        );
        assert_eq!(
            super::parse_bench_args(&args(&["--vm", "fast", "bench.rss"]))
                .expect_err("unknown vm should fail"),
            "invalid benchmark VM `fast`; expected reg."
        );
    }

    #[test]
    fn parse_bench_args_defaults_to_reg_vm() {
        let values = args(&["--mode", "vm-internal", "bench.rss"]);
        let options = super::parse_bench_args(&values).expect("bench args should parse");

        assert_eq!(options.mode, super::BenchMode::VmInternal);
        assert_eq!(options.vm, super::BenchVm::Reg);
    }
}
