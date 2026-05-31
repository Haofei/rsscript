mod common;

use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rsscript::{
    lower_sources_to_rust_package_with_options, package_lowering_input,
    write_generated_rust_package,
};

const REQUESTS: usize = 24;
const PAYLOAD_BYTES: usize = 64 * 1024;
const CONCURRENCY: usize = 8;
const SERVER_DELAY_MS: u64 = 50;

#[test]
#[ignore = "release/demo e2e; run from rss/test-runner/manifests/demo-e2e.rsstest.toml"]
fn file_upload_benchmark_reports_async_and_sync_requests_per_second() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let benchmark_dir = repo.join("benchmarks/file-upload");
    let native_dir = benchmark_dir.join("native/rust");
    let native_manifest = native_dir.join("Cargo.toml");
    let temp_dir = common::unique_temp_dir("rsscript-file-upload-benchmark");
    fs::create_dir_all(&temp_dir).expect("e2e temp dir should be created");

    cargo_build(&native_manifest, "mock_upload_server");
    cargo_build(&native_manifest, "rust_async_upload_client");
    cargo_build(&native_manifest, "sync_upload_client");

    let addr = available_local_addr();
    let server_log = temp_dir.join("file-upload-server.log");
    let _server = UploadServer::start(&native_dir, addr, &server_log);

    let generated_dir = temp_dir.join("generated-rss-file-upload-benchmark");
    generate_rss_package(&benchmark_dir, repo, &generated_dir);
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

    run_client(&rss_async_bin, addr, &[]);
    run_client(
        &rust_async_bin,
        addr,
        &[("RSS_FILE_UPLOAD_CONCURRENCY", CONCURRENCY.to_string())],
    );

    let rss_async_result = run_client(&rss_async_bin, addr, &[]);
    let rust_async_result = run_client(
        &rust_async_bin,
        addr,
        &[("RSS_FILE_UPLOAD_CONCURRENCY", CONCURRENCY.to_string())],
    );
    let sync_result = run_client(&sync_bin, addr, &[]);

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
        "file upload benchmark: rss_async_rps={:.2} rust_async_rps={:.2} sync_rps={:.2} rss_async_ms={} rust_async_ms={} sync_ms={} rss_async_max_in_flight={} rust_async_max_in_flight={} sync_max_in_flight={} rust_to_rss_rps_ratio={:.3} likely_bottleneck={}",
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

    let _ = fs::remove_dir_all(&temp_dir);
}

#[derive(Debug)]
struct ClientResult {
    elapsed_ms: u64,
    rps: f64,
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

fn generate_rss_package(benchmark_dir: &Path, repo: &Path, out_dir: &Path) {
    let input = package_lowering_input(benchmark_dir).expect("benchmark package should lower");
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

fn run_client(binary: &Path, addr: SocketAddr, extra_env: &[(&str, String)]) -> ClientResult {
    let started = Instant::now();
    let output = Command::new(binary)
        .env("RSS_FILE_UPLOAD_ENDPOINT", addr.to_string())
        .env("RSS_FILE_UPLOAD_REQUESTS", REQUESTS.to_string())
        .env("RSS_FILE_UPLOAD_PAYLOAD_BYTES", PAYLOAD_BYTES.to_string())
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
    let measured_rps = REQUESTS as f64 / elapsed.as_secs_f64();
    ClientResult {
        elapsed_ms: measured_ms,
        rps: measured_rps,
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
    fn start(native_dir: &Path, addr: SocketAddr, log_path: &Path) -> Self {
        let server_bin = binary_path(&native_dir.join("target/debug"), "mock_upload_server");
        let log = File::create(log_path).expect("server log should be created");
        let child = Command::new(&server_bin)
            .env("RSS_FILE_UPLOAD_ADDR", addr.to_string())
            .env("RSS_FILE_UPLOAD_DELAY_MS", SERVER_DELAY_MS.to_string())
            .stdout(Stdio::from(log))
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start {}: {error}", server_bin.display()));
        wait_for_server(addr);
        Self { child }
    }
}

impl Drop for UploadServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
