mod common;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rsscript::{
    lower_sources_to_rust_package_with_options, package_lowering_input,
    write_generated_rust_package,
};

const DEFAULT_REQUESTS: usize = 256;
const DEFAULT_PAYLOAD_BYTES: usize = 256 * 1024;
const DEFAULT_CONCURRENCY: usize = 16;
const DEFAULT_SERVER_DELAY_MS: u64 = 10;

#[test]
#[ignore = "release/demo e2e; run from rss/test-runner/manifests/demo-e2e.rsstest.toml"]
fn file_upload_benchmark_reports_async_and_sync_requests_per_second() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let benchmark_dir = repo.join("benchmarks/file-upload");
    let native_dir = benchmark_dir.join("native/rust");
    let native_manifest = native_dir.join("Cargo.toml");
    let config = BenchmarkConfig::from_env();
    let temp_dir = common::unique_temp_dir("rsscript-file-upload-benchmark");
    fs::create_dir_all(&temp_dir).expect("e2e temp dir should be created");

    cargo_build(&native_manifest, "mock_upload_server");
    cargo_build(&native_manifest, "rust_async_upload_client");
    cargo_build(&native_manifest, "sync_upload_client");

    let addr = available_local_addr();
    let server_log = temp_dir.join("file-upload-server.log");
    let _server = UploadServer::start(&native_dir, addr, &server_log, config.server_delay_ms);

    let generated_source_dir = temp_dir.join("rss-file-upload-benchmark-source");
    generate_rss_benchmark_source(&benchmark_dir, &native_dir, &generated_source_dir, &config);
    let generated_dir = temp_dir.join("generated-rss-file-upload-benchmark");
    generate_rss_package(&generated_source_dir, repo, &generated_dir);
    cargo_build(
        &generated_dir.join("Cargo.toml"),
        "rss-file-upload-benchmark",
    );

    let target_dir = native_dir.join("target/debug");
    let rss_async_bin = binary_path(
        &generated_dir.join("target/debug"),
        "rss-file-upload-benchmark",
    );
    let rust_async_bin = binary_path(&target_dir, "rust_async_upload_client");
    let sync_bin = binary_path(&target_dir, "sync_upload_client");

    run_client(&rss_async_bin, addr, &config, &[]);
    run_client(
        &rust_async_bin,
        addr,
        &config,
        &[(
            "RSS_FILE_UPLOAD_CONCURRENCY",
            config.concurrency.to_string(),
        )],
    );

    let rss_async_result = run_client(&rss_async_bin, addr, &config, &[]);
    let rust_async_result = run_client(
        &rust_async_bin,
        addr,
        &config,
        &[(
            "RSS_FILE_UPLOAD_CONCURRENCY",
            config.concurrency.to_string(),
        )],
    );
    let sync_result = run_client(&sync_bin, addr, &config, &[]);

    let log = fs::read_to_string(&server_log).expect("server log should be readable");
    let rss_async_max_in_flight = max_in_flight_for(&log, "/upload/rss-");
    let rust_async_max_in_flight = max_in_flight_for(&log, "/upload/rust-");
    let sync_max_in_flight = max_in_flight_for(&log, "/upload/sync-");

    assert!(
        rss_async_result.rps > sync_result.rps,
        "RSS async RPS should exceed sync RPS; rss_async={rss_async_result:?}, sync={sync_result:?}\n{log}"
    );
    assert!(
        rust_async_result.rps > sync_result.rps,
        "Rust async RPS should exceed sync RPS; rust_async={rust_async_result:?}, sync={sync_result:?}\n{log}"
    );
    assert!(
        rss_async_max_in_flight > 1,
        "RSS async client should overlap upload requests; log:\n{log}"
    );
    assert!(
        rust_async_max_in_flight > 1,
        "Rust async client should overlap upload requests; log:\n{log}"
    );
    assert_eq!(
        sync_max_in_flight, 1,
        "sync client should upload sequentially; log:\n{log}"
    );

    let rust_to_rss_ratio = rust_async_result.rps / rss_async_result.rps;
    let likely_bottleneck = if (rust_to_rss_ratio - 1.0).abs() <= 0.10 {
        "server_or_io"
    } else if rust_to_rss_ratio > 1.0 {
        "rss_runtime_or_lowering"
    } else {
        "rust_client_or_noise"
    };
    println!(
        "file upload benchmark: requests={} payload_bytes={} concurrency={} server_delay_ms={} rss_async_rps={:.2} rust_async_rps={:.2} sync_rps={:.2} rss_async_ms={} rust_async_ms={} sync_ms={} rss_async_max_in_flight={} rust_async_max_in_flight={} sync_max_in_flight={} rust_to_rss_rps_ratio={:.3} likely_bottleneck={}",
        config.requests,
        config.payload_bytes,
        config.concurrency,
        config.server_delay_ms,
        rss_async_result.rps,
        rust_async_result.rps,
        sync_result.rps,
        rss_async_result.elapsed_ms,
        rust_async_result.elapsed_ms,
        sync_result.elapsed_ms,
        rss_async_max_in_flight,
        rust_async_max_in_flight,
        sync_max_in_flight,
        rust_to_rss_ratio,
        likely_bottleneck,
    );
    print_trace_comparison(&rss_async_result.trace, &rust_async_result.trace);
    print_runtime_trace("rss_async", &rss_async_result.runtime_trace);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[derive(Debug, Clone)]
struct BenchmarkConfig {
    requests: usize,
    payload_bytes: usize,
    concurrency: usize,
    server_delay_ms: u64,
}

impl BenchmarkConfig {
    fn from_env() -> Self {
        Self {
            requests: env_usize("RSS_FILE_UPLOAD_REQUESTS", DEFAULT_REQUESTS),
            payload_bytes: env_usize("RSS_FILE_UPLOAD_PAYLOAD_BYTES", DEFAULT_PAYLOAD_BYTES),
            concurrency: env_usize("RSS_FILE_UPLOAD_CONCURRENCY", DEFAULT_CONCURRENCY).max(1),
            server_delay_ms: env_u64("RSS_FILE_UPLOAD_DELAY_MS", DEFAULT_SERVER_DELAY_MS),
        }
    }
}

#[derive(Debug)]
struct ClientResult {
    elapsed_ms: u64,
    rps: f64,
    trace: TraceStats,
    runtime_trace: RuntimeTraceStats,
}

#[derive(Debug, Default, Clone)]
struct TraceStats {
    phases: BTreeMap<String, PhaseStats>,
}

#[derive(Debug, Default, Clone)]
struct PhaseStats {
    count: usize,
    total_us: u128,
    max_us: u128,
}

#[derive(Debug, Default, Clone)]
struct RuntimeTraceStats {
    phases: BTreeMap<String, RuntimePhaseStats>,
}

#[derive(Debug, Default, Clone)]
struct RuntimePhaseStats {
    count: usize,
    total_us: u128,
    max_us: u128,
    polls: u128,
    yields: u128,
    tasks: u128,
}

impl RuntimePhaseStats {
    fn record(&mut self, elapsed_us: u128, polls: u128, yields: u128, tasks: u128) {
        self.count += 1;
        self.total_us += elapsed_us;
        self.max_us = self.max_us.max(elapsed_us);
        self.polls += polls;
        self.yields += yields;
        self.tasks += tasks;
    }

    fn avg_us(&self) -> u128 {
        if self.count == 0 {
            0
        } else {
            self.total_us / self.count as u128
        }
    }
}

impl PhaseStats {
    fn record(&mut self, elapsed_us: u128) {
        self.count += 1;
        self.total_us += elapsed_us;
        self.max_us = self.max_us.max(elapsed_us);
    }

    fn avg_us(&self) -> u128 {
        if self.count == 0 {
            0
        } else {
            self.total_us / self.count as u128
        }
    }
}

fn cargo_build(manifest: &Path, bin: &str) {
    let output = Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--bin")
        .arg(bin)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("cargo build should run");
    assert!(
        output.status.success(),
        "cargo build failed for {bin}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_rss_benchmark_source(
    benchmark_dir: &Path,
    native_dir: &Path,
    out_dir: &Path,
    config: &BenchmarkConfig,
) {
    fs::create_dir_all(out_dir.join("src")).expect("benchmark source src dir should be created");
    fs::create_dir_all(out_dir.join("interface"))
        .expect("benchmark source interface dir should be created");
    fs::create_dir_all(out_dir.join("native")).expect("benchmark native dir should be created");
    fs::copy(
        benchmark_dir.join("interface/upload.rssi"),
        out_dir.join("interface/upload.rssi"),
    )
    .expect("benchmark interface should copy");
    fs::copy(
        benchmark_dir.join("native/bindings.rssbind.toml"),
        out_dir.join("native/bindings.rssbind.toml"),
    )
    .expect("benchmark native bindings should copy");
    fs::write(
        out_dir.join("rsspkg.toml"),
        format!(
            r#"[package]
name = "rss-file-upload-benchmark"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]

[interfaces]
paths = ["interface"]

[native.rust]
enabled = true
path = "{}"
crate = "rss_file_upload_benchmark_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
native_links = "forbid"
ffi = "forbid"

[[review.capability_bindings]]
symbol = "Upload.put"
category = "network.client"
provider = "local"
service = "mock-upload"
action = "upload:Put"
resource = "http://127.0.0.1/upload/*"
"#,
            native_dir.display()
        ),
    )
    .expect("benchmark manifest should be written");
    fs::write(out_dir.join("src/main.rss"), rss_benchmark_source(config))
        .expect("generated RSS benchmark source should be written");
}

fn rss_benchmark_source(config: &BenchmarkConfig) -> String {
    let mut source = String::from("features: async\n\n");
    let mut request = 0usize;
    let mut batch = 0usize;
    while request < config.requests {
        let end = (request + config.concurrency).min(config.requests);
        source.push_str(&format!(
            "async fn upload_batch_{batch}() -> Result<Unit, String> {{\n"
        ));
        source.push_str("    task_group {\n");
        for index in request..end {
            source.push_str(&format!(
                "        async let upload_{index} = Upload.put(path: read \"/upload/rss-{index}.bin\", body: read \"rss payload seed\")\n"
            ));
        }
        source.push('\n');
        for index in request..end {
            source.push_str(&format!("        await upload_{index}?\n"));
        }
        source.push_str("    }\n");
        source.push_str("    return Ok(Unit)\n");
        source.push_str("}\n\n");
        request = end;
        batch += 1;
    }

    source.push_str("async fn main() -> Result<Unit, String> {\n");
    for index in 0..batch {
        source.push_str(&format!("    await upload_batch_{index}()?\n"));
    }
    source.push_str("    return Ok(Unit)\n");
    source.push_str("}\n");
    source
}

fn generate_rss_package(package_dir: &Path, repo: &Path, out_dir: &Path) {
    let input = package_lowering_input(package_dir).expect("benchmark package should lower");
    let runtime_path = repo.join("runtime").display().to_string();
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        &runtime_path,
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("benchmark package Rust lowering should succeed");
    write_generated_rust_package(out_dir, &package)
        .expect("generated RSS benchmark package should be written");
}

fn run_client(
    binary: &Path,
    addr: SocketAddr,
    config: &BenchmarkConfig,
    extra_env: &[(&str, String)],
) -> ClientResult {
    let started = Instant::now();
    let output = Command::new(binary)
        .env("RSS_FILE_UPLOAD_ENDPOINT", addr.to_string())
        .env("RSS_FILE_UPLOAD_REQUESTS", config.requests.to_string())
        .env(
            "RSS_FILE_UPLOAD_PAYLOAD_BYTES",
            config.payload_bytes.to_string(),
        )
        .envs(extra_env.iter().map(|(key, value)| (*key, value)))
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.display()));
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        binary.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let measured_ms = elapsed.as_millis().max(1) as u64;
    let measured_rps = config.requests as f64 / elapsed.as_secs_f64();
    ClientResult {
        elapsed_ms: measured_ms,
        rps: measured_rps,
        trace: parse_trace_stats(&String::from_utf8_lossy(&output.stdout)),
        runtime_trace: parse_runtime_trace_stats(&String::from_utf8_lossy(&output.stdout)),
    }
}

fn parse_trace_stats(stdout: &str) -> TraceStats {
    let mut stats = TraceStats::default();
    for line in stdout.lines().filter(|line| line.contains("upload_phase")) {
        let Some(phase) = trace_field(line, "phase") else {
            continue;
        };
        let Some(elapsed_us) =
            trace_field(line, "elapsed_us").and_then(|value| value.parse::<u128>().ok())
        else {
            continue;
        };
        stats.phases.entry(phase).or_default().record(elapsed_us);
    }
    stats
}

fn parse_runtime_trace_stats(stdout: &str) -> RuntimeTraceStats {
    let mut stats = RuntimeTraceStats::default();
    for line in stdout
        .lines()
        .filter(|line| line.contains("async_runtime_phase"))
    {
        let Some(phase) = trace_field(line, "phase") else {
            continue;
        };
        let elapsed_us = trace_field(line, "elapsed_us")
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or(0);
        let polls = trace_field(line, "polls")
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or(0);
        let yields = trace_field(line, "yields")
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or(0);
        let tasks = trace_field(line, "tasks")
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or(0);
        stats
            .phases
            .entry(phase)
            .or_default()
            .record(elapsed_us, polls, yields, tasks);
    }
    stats
}

fn trace_field(line: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    line.split_whitespace().find_map(|token| {
        token
            .strip_prefix(&prefix)
            .map(|value| value.trim_matches(|ch| ch == '"' || ch == ',').to_string())
    })
}

fn print_trace_comparison(rss: &TraceStats, rust: &TraceStats) {
    if rss.phases.is_empty() && rust.phases.is_empty() {
        return;
    }

    for phase in ["connect", "write_request", "read_response", "total"] {
        let Some(rss_phase) = rss.phases.get(phase) else {
            continue;
        };
        let Some(rust_phase) = rust.phases.get(phase) else {
            continue;
        };
        let rss_avg = rss_phase.avg_us();
        let rust_avg = rust_phase.avg_us();
        let delta = rss_avg as i128 - rust_avg as i128;
        println!(
            "file upload trace: phase={phase} rss_avg_us={} rust_avg_us={} delta_us={} rss_max_us={} rust_max_us={} samples={}/{}",
            rss_avg,
            rust_avg,
            delta,
            rss_phase.max_us,
            rust_phase.max_us,
            rss_phase.count,
            rust_phase.count,
        );
    }
    for phase in ["native_start"] {
        let Some(rss_phase) = rss.phases.get(phase) else {
            continue;
        };
        println!(
            "file upload trace: phase={phase} rss_avg_us={} rust_avg_us=n/a delta_us=n/a rss_max_us={} rust_max_us=n/a samples={}/0",
            rss_phase.avg_us(),
            rss_phase.max_us,
            rss_phase.count,
        );
    }
}

fn print_runtime_trace(mode: &str, stats: &RuntimeTraceStats) {
    for (phase, phase_stats) in &stats.phases {
        println!(
            "file upload runtime trace: mode={mode} phase={phase} count={} avg_us={} max_us={} polls={} yields={} tasks={}",
            phase_stats.count,
            phase_stats.avg_us(),
            phase_stats.max_us,
            phase_stats.polls,
            phase_stats.yields,
            phase_stats.tasks,
        );
    }
}

fn available_local_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral port should bind");
    let addr = listener.local_addr().expect("local addr should be known");
    drop(listener);
    addr
}

fn wait_for_server(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("file upload server did not start on {addr}");
}

fn binary_path(target_dir: &Path, name: &str) -> PathBuf {
    target_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn max_in_flight_for(log: &str, path_fragment: &str) -> usize {
    log.lines()
        .filter(|line| line.contains(path_fragment))
        .filter_map(|line| {
            line.split("in_flight=")
                .nth(1)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0)
}

struct UploadServer {
    child: Child,
}

impl UploadServer {
    fn start(native_dir: &Path, addr: SocketAddr, log_path: &Path, delay_ms: u64) -> Self {
        let server_bin = binary_path(&native_dir.join("target/debug"), "mock_upload_server");
        let log = File::create(log_path).expect("server log should be created");
        let child = Command::new(&server_bin)
            .env("RSS_FILE_UPLOAD_ADDR", addr.to_string())
            .env("RSS_FILE_UPLOAD_DELAY_MS", delay_ms.to_string())
            .stdout(Stdio::from(log))
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start {}: {error}", server_bin.display()));
        wait_for_server(addr);
        Self { child }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

impl Drop for UploadServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
