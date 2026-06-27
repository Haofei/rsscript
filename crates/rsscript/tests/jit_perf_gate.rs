//! Fast JIT performance smoke gate.
//!
//! This intentionally lives in the existing Rust test harness instead of adding
//! another `rss` CLI command. Docker/CI run this test directly for the
//! day-to-day native-JIT perf gate.
#![cfg(feature = "native-jit")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Clone, Debug)]
struct CaseSpec {
    case: String,
    path: PathBuf,
    size: String,
}

#[derive(Clone, Debug)]
struct GateRow {
    case: String,
    baseline_ms: f64,
    current_ms: f64,
    delta_pct: f64,
    attempts: usize,
    bails: Option<i64>,
    compiled_code_bytes: Option<i64>,
    native_call_edges: Option<i64>,
    native_call_depth_max: Option<i64>,
    verdict: &'static str,
}

const DEFAULT_BASELINE: &str = "benchmarks/vm-jit/baseline/baseline-20260626-six-framework.json";
const DEFAULT_THRESHOLD_PCT: f64 = 75.0;
const DEFAULT_ITERATIONS: usize = 3;
const DEFAULT_WARMUP: usize = 1;

const DEFAULT_CASES: &[&str] = &[
    "native_scalar_loop.rss",
    "profile_closure_pic.rss",
    "profile_branch_cold_blocks.rss",
    "profile_branch_side_exits.rss",
    "native_call_chain.rss",
    "native_call_nested_chain.rss",
    "native_call_bool_param.rss",
    "native_call_float_param.rss",
    "native_call_flat_int_param.rss",
    "native_call_flat_float_param.rss",
    "native_call_flat_int_mut_param.rss",
    "native_call_handle_param.rss",
    "native_call_mut_handle_param.rss",
    "osr_closure_loop.rss",
    "native_bytes_slice_len_loop.rss",
];

const TELEMETRY_ONLY_CASES: &[&str] = &[
    "profile_branch_cold_blocks.rss",
    "profile_branch_side_exits.rss",
    "native_call_nested_chain.rss",
    "native_call_bool_param.rss",
    "native_call_float_param.rss",
    "native_call_flat_int_param.rss",
    "native_call_flat_float_param.rss",
    "native_call_flat_int_mut_param.rss",
    "native_call_handle_param.rss",
    "native_call_mut_handle_param.rss",
];

#[test]
fn jit_perf_gate_against_baseline() {
    let repo = repo_root();
    let baseline_path =
        env_path("RSS_JIT_PERF_BASELINE").unwrap_or_else(|| repo.join(DEFAULT_BASELINE));
    let cases_file = repo.join("benchmarks/vm-jit/cases.tsv");
    let cases = load_cases(&cases_file);
    let baseline = load_baseline(&baseline_path);
    let selected = selected_cases(&cases);
    let threshold_pct = env_f64("RSS_JIT_PERF_THRESHOLD_PCT").unwrap_or(DEFAULT_THRESHOLD_PCT);
    let iterations = env_usize("RSS_JIT_PERF_ITERATIONS").unwrap_or(DEFAULT_ITERATIONS);
    let warmup = env_usize("RSS_JIT_PERF_WARMUP").unwrap_or(DEFAULT_WARMUP);
    let timing_retries = env_usize("RSS_JIT_PERF_TIMING_RETRIES").unwrap_or(1);
    let allow_bails = env_bool("RSS_JIT_PERF_ALLOW_BAILS");
    let skip_telemetry = env_bool("RSS_JIT_PERF_SKIP_TELEMETRY");

    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for case in selected {
        let key = (case.case.clone(), case.size.clone());
        let telemetry_only = TELEMETRY_ONLY_CASES.contains(&case.case.as_str());
        let mut ref_ms = match baseline
            .get(&key)
            .and_then(|row| mode_center(row, "native"))
        {
            Some(value) => Some(value),
            None if telemetry_only => None,
            None => {
                failures.push(format!("{}: missing native baseline timing", case.case));
                continue;
            }
        };

        let mut result = match run_bench(&repo, &case, iterations, warmup) {
            Ok(result) => result,
            Err(error) => {
                failures.push(format!("{}: {error}", case.case));
                continue;
            }
        };
        let mut cur_ms = bench_center_ms(&result).unwrap_or(0.0);
        let ref_ms = ref_ms.get_or_insert(cur_ms);
        let ref_ms = *ref_ms;
        let mut delta_pct = percent_delta(ref_ms, cur_ms);
        let mut attempts = 1;
        for _ in 0..timing_retries {
            if delta_pct <= threshold_pct {
                break;
            }
            let retry = match run_bench(&repo, &case, iterations, warmup) {
                Ok(result) => result,
                Err(error) => {
                    failures.push(format!("{} retry: {error}", case.case));
                    break;
                }
            };
            let retry_ms = bench_center_ms(&retry).unwrap_or(cur_ms);
            let retry_delta = percent_delta(ref_ms, retry_ms);
            attempts += 1;
            if retry_delta < delta_pct {
                result = retry;
                cur_ms = retry_ms;
                delta_pct = retry_delta;
            }
        }

        let jit = result.get("jit").unwrap_or(&Value::Null);
        let bails = jit_counter(jit, "bails");
        let compiled_code_bytes = jit_counter(jit, "compiled_code_bytes");
        let native_call_edges = jit_counter(jit, "native_call_edges");
        let native_call_depth_max = jit_counter(jit, "native_call_depth_max");
        let mut verdict = "OK";
        if delta_pct > threshold_pct {
            verdict = "REGRESSION";
            failures.push(format!(
                "{}: {:.1}% slower than baseline ({:.3}ms vs {:.3}ms)",
                case.case, delta_pct, cur_ms, ref_ms
            ));
        }
        if !allow_bails && bails.is_some_and(|value| value != 0) {
            verdict = "BAILED";
            failures.push(format!("{}: native bails={}", case.case, bails.unwrap()));
        }
        if !skip_telemetry {
            for (counter, minimum) in expected_min_counters(&case.case) {
                let value = jit_counter(jit, counter);
                if value.is_none_or(|actual| actual < *minimum) {
                    verdict = "MISSING_TELEMETRY";
                    failures.push(format!(
                        "{}: expected {counter}>={minimum}, got {value:?}",
                        case.case
                    ));
                }
            }
        }

        rows.push(GateRow {
            case: case.case,
            baseline_ms: ref_ms,
            current_ms: cur_ms,
            delta_pct,
            attempts,
            bails,
            compiled_code_bytes,
            native_call_edges,
            native_call_depth_max,
            verdict,
        });
    }

    print_gate_table(&rows);
    assert!(
        failures.is_empty(),
        "JIT perf gate failed:\n{}",
        failures.join("\n")
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should exist")
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_bool(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn selected_cases(cases: &HashMap<String, CaseSpec>) -> Vec<CaseSpec> {
    let names = std::env::var("RSS_JIT_PERF_CASES").ok();
    let requested: Vec<&str> = names
        .as_deref()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_else(|| DEFAULT_CASES.to_vec());
    requested
        .into_iter()
        .map(|name| {
            let basename = Path::new(name)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(name);
            cases
                .get(basename)
                .unwrap_or_else(|| panic!("unknown benchmark case `{basename}`"))
                .clone()
        })
        .collect()
}

fn load_cases(path: &Path) -> HashMap<String, CaseSpec> {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    contents
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let mut parts = trimmed.split_whitespace();
            let _category = parts.next()?;
            let rel_path = parts.next()?;
            let size = parts.next()?.to_string();
            let case = Path::new(rel_path).file_name()?.to_str()?.to_string();
            Some((
                case.clone(),
                CaseSpec {
                    case,
                    path: PathBuf::from(rel_path),
                    size,
                },
            ))
        })
        .collect()
}

fn load_baseline(path: &Path) -> HashMap<(String, String), Value> {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let doc: Value = serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("invalid baseline JSON {}: {error}", path.display()));
    let mut rows = HashMap::new();
    let Some(cases) = doc.get("cases").and_then(Value::as_array) else {
        return rows;
    };
    for row in cases {
        let Some(case) = row.get("case").and_then(Value::as_str) else {
            continue;
        };
        let size = row
            .get("size")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| row.get("size").map(Value::to_string).unwrap_or_default());
        rows.insert((case.to_string(), size), row.clone());
    }
    rows
}

fn run_bench(
    repo: &Path,
    case: &CaseSpec,
    iterations: usize,
    warmup: usize,
) -> Result<Value, String> {
    let _env = EnvGuard::set_many(case_env(&case.case));
    let path = repo.join(&case.path);
    let path_label = path.display().to_string();
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let executable = rsscript::reg_vm_compile_source(&path_label, &source)
        .map_err(|error| format!("compile failed for {}: {error:?}", case.case))?;
    let args = [case.size.as_str()];
    for _ in 0..warmup {
        executable
            .eval_main_with_args_native(args.iter().copied())
            .map_err(|error| format!("warmup failed for {}: {error:?}", case.case))?;
    }
    let mut measurements = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        executable
            .eval_main_with_args_native(args.iter().copied())
            .map_err(|error| format!("run failed for {}: {error:?}", case.case))?;
        measurements.push(start.elapsed());
    }
    measurements.sort_unstable();
    let median = median_duration(&measurements);
    let total_nanos: u128 = measurements.iter().map(Duration::as_nanos).sum();
    let mean = Duration::from_nanos((total_nanos / measurements.len() as u128) as u64);

    let (_output, warm_stats) = executable
        .eval_main_with_args_native_with_stats(args.iter().copied())
        .map_err(|error| format!("warm stats failed for {}: {error:?}", case.case))?;
    let mut jit = warm_stats.to_json();
    let stats_executable = rsscript::reg_vm_compile_source(&path_label, &source)
        .map_err(|error| format!("stats compile failed for {}: {error:?}", case.case))?;
    let (_output, cold_stats) = stats_executable
        .eval_main_with_args_native_with_stats(args.iter().copied())
        .map_err(|error| format!("cold stats failed for {}: {error:?}", case.case))?;
    jit["cold_start"] = cold_stats.to_json();

    Ok(serde_json::json!({
        "name": case.case.trim_end_matches(".rss"),
        "mode": "jit-native",
        "vm": "reg",
        "iterations": iterations,
        "warmup": warmup,
        "min_ms": millis(measurements[0]),
        "mean_ms": millis(mean),
        "median_ms": millis(median),
        "max_ms": millis(*measurements.last().unwrap()),
        "samples_ms": measurements.iter().copied().map(millis).collect::<Vec<_>>(),
        "jit": jit,
    }))
}

fn case_env(case: &str) -> &'static [(&'static str, &'static str)] {
    match case {
        "profile_branch_cold_blocks.rss" => &[("RSS_JIT_TIER_THRESHOLD", "200")],
        "profile_branch_side_exits.rss" => &[("RSS_JIT_TIER_THRESHOLD", "66")],
        _ => &[],
    }
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set_many(values: &'static [(&'static str, &'static str)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        for (key, value) in values {
            // These tests run through Make with `--test-threads=1`, and the JIT
            // tier reads process environment knobs. Keep the mutation scoped.
            unsafe {
                std::env::set_var(key, value);
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn median_duration(sorted: &[Duration]) -> Duration {
    let len = sorted.len();
    if len % 2 == 1 {
        sorted[len / 2]
    } else {
        let lo = sorted[len / 2 - 1].as_nanos();
        let hi = sorted[len / 2].as_nanos();
        Duration::from_nanos(((lo + hi) / 2) as u64)
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn mode_center(row: &Value, mode_key: &str) -> Option<f64> {
    row.get(mode_key)
        .and_then(|nested| {
            nested
                .get("median")
                .or_else(|| nested.get("mean"))
                .and_then(Value::as_f64)
        })
        .or_else(|| row.get(format!("{mode_key}_ms")).and_then(Value::as_f64))
}

fn bench_center_ms(row: &Value) -> Option<f64> {
    row.get("median_ms")
        .or_else(|| row.get("mean_ms"))
        .and_then(Value::as_f64)
        .or_else(|| {
            let mut samples: Vec<f64> = row
                .get("samples_ms")?
                .as_array()?
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            samples.sort_by(f64::total_cmp);
            match samples.len() {
                0 => None,
                len if len % 2 == 1 => Some(samples[len / 2]),
                len => Some((samples[len / 2 - 1] + samples[len / 2]) / 2.0),
            }
        })
}

fn percent_delta(reference: f64, current: f64) -> f64 {
    if reference == 0.0 {
        if current > 0.0 { f64::INFINITY } else { 0.0 }
    } else {
        (current - reference) / reference * 100.0
    }
}

fn jit_counter(jit: &Value, key: &str) -> Option<i64> {
    let mut values = Vec::new();
    if let Some(value) = jit.get(key).and_then(Value::as_i64) {
        values.push(value);
    }
    if let Some(value) = jit
        .get("cold_start")
        .and_then(|cold| cold.get(key))
        .and_then(Value::as_i64)
    {
        values.push(value);
    }
    values.into_iter().max()
}

fn expected_min_counters(case: &str) -> &'static [(&'static str, i64)] {
    match case {
        "native_scalar_loop.rss" => &[("compiled_code_bytes", 1)],
        "native_call_chain.rss" => &[
            ("native_call_edges", 1),
            ("native_call_depth_max", 1),
            ("compiled_code_bytes", 1),
        ],
        "native_call_nested_chain.rss" => &[
            ("native_call_edges", 2),
            ("native_call_depth_max", 2),
            ("compiled_code_bytes", 1),
        ],
        "native_call_bool_param.rss"
        | "native_call_float_param.rss"
        | "native_call_flat_int_param.rss"
        | "native_call_flat_float_param.rss"
        | "native_call_flat_int_mut_param.rss"
        | "native_call_handle_param.rss"
        | "native_call_mut_handle_param.rss" => &[
            ("native_call_edges", 1),
            ("native_call_depth_max", 1),
            ("compiled_code_bytes", 1),
        ],
        "profile_closure_pic.rss" => &[
            ("profile_closure_pic_sites", 1),
            ("profile_closure_pic_arms", 3),
            ("profile_branch_sites", 1),
            ("profile_branch_samples", 1),
            ("compiled_code_bytes", 1),
        ],
        "profile_branch_cold_blocks.rss" => &[
            ("profile_branch_sites", 1),
            ("profile_branch_samples", 1),
            ("profile_branch_cold_blocks", 1),
            ("compiled_code_bytes", 1),
        ],
        "profile_branch_side_exits.rss" => &[
            ("profile_branch_sites", 1),
            ("profile_branch_samples", 1),
            ("profile_branch_cold_blocks", 1),
            ("profile_branch_side_exits", 1),
            ("compiled_code_bytes", 1),
        ],
        "osr_closure_loop.rss" => &[
            ("profile_closure_guard_sites", 1),
            ("profile_branch_sites", 1),
            ("profile_branch_samples", 1),
            ("compiled_code_bytes", 1),
        ],
        _ => &[],
    }
}

fn print_gate_table(rows: &[GateRow]) {
    println!(
        "{:<34} {:>9} {:>9} {:>8} {:>3} {:>7} {:>7} {:>7} {:>7} verdict",
        "case", "base_ms", "cur_ms", "delta", "try", "bails", "edges", "depth", "code_b"
    );
    println!("{}", "-".repeat(116));
    for row in rows {
        println!(
            "{:<34} {:>9.3} {:>9.3} {:>+7.1}% {:>3} {:>7} {:>7} {:>7} {:>7} {}",
            row.case,
            row.baseline_ms,
            row.current_ms,
            row.delta_pct,
            row.attempts,
            fmt_counter(row.bails),
            fmt_counter(row.native_call_edges),
            fmt_counter(row.native_call_depth_max),
            fmt_counter(row.compiled_code_bytes),
            row.verdict
        );
    }
}

fn fmt_counter(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}
