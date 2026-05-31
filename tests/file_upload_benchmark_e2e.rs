mod common;

use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const REQUESTS: usize = 24;
const PAYLOAD_BYTES: usize = 64 * 1024;
const CONCURRENCY: usize = 8;
const SERVER_DELAY_MS: u64 = 50;

#[test]
#[ignore = "release/demo e2e; run from rss/test-runner/manifests/demo-e2e.rsstest.toml"]
fn file_upload_benchmark_reports_async_and_sync_requests_per_second() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let demo_dir = repo.join("demos/file-upload-benchmark");
    let native_dir = demo_dir.join("native/rust");
    let native_manifest = native_dir.join("Cargo.toml");
    let temp_dir = common::unique_temp_dir("rsscript-file-upload-benchmark");
    fs::create_dir_all(&temp_dir).expect("e2e temp dir should be created");

    cargo_build(&native_manifest, "mock_upload_server");
    cargo_build(&native_manifest, "async_upload_client");
    cargo_build(&native_manifest, "sync_upload_client");

    let addr = available_local_addr();
    let server_log = temp_dir.join("file-upload-server.log");
    let _server = UploadServer::start(&native_dir, addr, &server_log);

    let target_dir = native_dir.join("target/debug");
    let async_bin = binary_path(&target_dir, "async_upload_client");
    let sync_bin = binary_path(&target_dir, "sync_upload_client");

    let async_result = run_client(
        &async_bin,
        addr,
        &[("RSS_FILE_UPLOAD_CONCURRENCY", CONCURRENCY.to_string())],
    );
    let sync_result = run_client(&sync_bin, addr, &[]);

    let log = fs::read_to_string(&server_log).expect("server log should be readable");
    let async_max_in_flight = max_in_flight_for(&log, "/upload/async-");
    let sync_max_in_flight = max_in_flight_for(&log, "/upload/sync-");

    assert!(
        async_result.rps > sync_result.rps,
        "async RPS should exceed sync RPS; async={async_result:?}, sync={sync_result:?}\n{log}"
    );
    assert!(
        async_max_in_flight > 1,
        "async client should overlap upload requests; log:\n{log}"
    );
    assert_eq!(
        sync_max_in_flight, 1,
        "sync client should upload sequentially; log:\n{log}"
    );

    println!(
        "file upload benchmark: async_rps={:.2} sync_rps={:.2} async_ms={} sync_ms={} async_max_in_flight={} sync_max_in_flight={}",
        async_result.rps,
        sync_result.rps,
        async_result.elapsed_ms,
        sync_result.elapsed_ms,
        async_max_in_flight,
        sync_max_in_flight
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

fn run_client(binary: &Path, addr: SocketAddr, extra_env: &[(&str, String)]) -> ClientResult {
    let output = Command::new(binary)
        .env("RSS_FILE_UPLOAD_ENDPOINT", addr.to_string())
        .env("RSS_FILE_UPLOAD_REQUESTS", REQUESTS.to_string())
        .env("RSS_FILE_UPLOAD_PAYLOAD_BYTES", PAYLOAD_BYTES.to_string())
        .envs(extra_env.iter().map(|(key, value)| (*key, value)))
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.display()));
    assert!(
        output.status.success(),
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        binary.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    ClientResult {
        elapsed_ms: parse_metric(&stdout, "elapsed_ms")
            .unwrap_or_else(|| panic!("elapsed_ms missing in output:\n{stdout}"))
            as u64,
        rps: parse_metric(&stdout, "rps")
            .unwrap_or_else(|| panic!("rps missing in output:\n{stdout}")),
    }
}

fn parse_metric(output: &str, key: &str) -> Option<f64> {
    output.split_whitespace().find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.parse::<f64>().ok()).flatten()
    })
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
