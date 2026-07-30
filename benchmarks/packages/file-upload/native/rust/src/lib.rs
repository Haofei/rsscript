use std::env;
use std::sync::{Arc, Once, OnceLock};
use std::time::Instant;

use rsscript_runtime::host::{NativeAsyncPending, spawn_tokio_native};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::info;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39190";
const DEFAULT_PAYLOAD_BYTES: usize = 64 * 1024;
static TRACE_INIT: Once = Once::new();
static PAYLOAD_CACHE: OnceLock<Arc<Vec<u8>>> = OnceLock::new();

#[allow(clippy::ptr_arg)] // Signature is part of the generated RSS native ABI.
pub fn upload_start(path: &String, body: &String) -> NativeAsyncPending<Result<(), String>> {
    init_tracing();
    let started = Instant::now();
    let endpoint =
        env::var("RSS_FILE_UPLOAD_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let path = path.clone();
    let body = shared_body(body);
    trace_upload_phase(
        "rss_async",
        "native_start",
        &path,
        started.elapsed().as_micros(),
    );

    spawn_tokio_native(async move { upload_one(&endpoint, &path, &body).await })
}

async fn upload_one(endpoint: &str, path: &str, body: &[u8]) -> Result<(), String> {
    let total_started = Instant::now();
    let connect_started = Instant::now();
    let mut stream = TcpStream::connect(endpoint)
        .await
        .map_err(|err| format!("connect {endpoint}: {err}"))?;
    let connect_us = connect_started.elapsed().as_micros();

    let write_started = Instant::now();
    let header = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|err| format!("write header {path}: {err}"))?;
    stream
        .write_all(body)
        .await
        .map_err(|err| format!("write body {path}: {err}"))?;
    stream
        .shutdown()
        .await
        .map_err(|err| format!("shutdown {path}: {err}"))?;
    let write_us = write_started.elapsed().as_micros();

    let read_started = Instant::now();
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("read response {path}: {err}"))?;
    let read_us = read_started.elapsed().as_micros();
    let response = String::from_utf8_lossy(&response);
    let status = response.lines().next().unwrap_or("<empty>");
    if status.contains(" 201 ") {
        trace_upload(
            "rss_async",
            path,
            connect_us,
            write_us,
            read_us,
            total_started.elapsed().as_micros(),
        );
        Ok(())
    } else {
        Err(format!("upload {path} failed: {status}"))
    }
}

fn shared_body(seed: &str) -> Arc<Vec<u8>> {
    PAYLOAD_CACHE
        .get_or_init(|| Arc::new(expanded_body(seed)))
        .clone()
}

fn expanded_body(seed: &str) -> Vec<u8> {
    let target = env::var("RSS_FILE_UPLOAD_PAYLOAD_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAYLOAD_BYTES);
    if seed.len() >= target {
        return seed.as_bytes()[..target].to_vec();
    }

    let mut body = Vec::with_capacity(target);
    while body.len() < target {
        body.extend_from_slice(seed.as_bytes());
        body.push(b'\n');
    }
    body.truncate(target);
    body
}

fn trace_upload(
    mode: &str,
    path: &str,
    connect_us: u128,
    write_us: u128,
    read_us: u128,
    total_us: u128,
) {
    if env::var_os("RSS_FILE_UPLOAD_TRACE").is_none() {
        return;
    }
    trace_upload_phase(mode, "connect", path, connect_us);
    trace_upload_phase(mode, "write_request", path, write_us);
    trace_upload_phase(mode, "read_response", path, read_us);
    trace_upload_phase(mode, "total", path, total_us);
}

fn trace_upload_phase(mode: &str, phase: &str, path: &str, elapsed_us: u128) {
    if env::var_os("RSS_FILE_UPLOAD_TRACE").is_none() {
        return;
    }
    info!(mode, phase, path, elapsed_us, "upload_phase");
}

fn init_tracing() {
    if env::var_os("RSS_FILE_UPLOAD_TRACE").is_none() {
        return;
    }
    TRACE_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .without_time()
            .with_target(false)
            .with_level(false)
            .try_init();
    });
}
