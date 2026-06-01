use std::env;
use std::error::Error;
use std::sync::Once;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::info;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39190";
const DEFAULT_REQUESTS: usize = 24;
const DEFAULT_PAYLOAD_BYTES: usize = 64 * 1024;
const DEFAULT_CONCURRENCY: usize = 8;
static TRACE_INIT: Once = Once::new();

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    init_tracing();
    let endpoint =
        env::var("RSS_FILE_UPLOAD_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let requests = env_usize("RSS_FILE_UPLOAD_REQUESTS", DEFAULT_REQUESTS);
    let payload_bytes = env_usize("RSS_FILE_UPLOAD_PAYLOAD_BYTES", DEFAULT_PAYLOAD_BYTES);
    let concurrency = env_usize("RSS_FILE_UPLOAD_CONCURRENCY", DEFAULT_CONCURRENCY).max(1);
    let payload = Arc::new(payload(payload_bytes));
    let started = Instant::now();

    let mut batch_start = 0usize;
    while batch_start < requests {
        let batch_end = (batch_start + concurrency).min(requests);
        let mut tasks = Vec::with_capacity(batch_end - batch_start);
        for index in batch_start..batch_end {
            let endpoint = endpoint.clone();
            let payload = payload.clone();
            tasks.push(tokio::spawn(async move {
                upload_one(&endpoint, index, &payload).await
            }));
        }
        for task in tasks {
            task.await??;
        }
        batch_start = batch_end;
    }

    let elapsed = started.elapsed();
    let rps = requests as f64 / elapsed.as_secs_f64();
    println!(
        "mode=rust_async requests={requests} payload_bytes={payload_bytes} concurrency={concurrency} elapsed_ms={} rps={rps:.2}",
        elapsed.as_millis()
    );
    Ok(())
}

async fn upload_one(
    endpoint: &str,
    index: usize,
    payload: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let total_started = Instant::now();
    let connect_started = Instant::now();
    let mut stream = TcpStream::connect(endpoint).await?;
    let connect_us = connect_started.elapsed().as_micros();

    let path = format!("/upload/rust-{index}.bin");
    let write_started = Instant::now();
    let header = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(payload).await?;
    stream.shutdown().await?;
    let write_us = write_started.elapsed().as_micros();

    let read_started = Instant::now();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let read_us = read_started.elapsed().as_micros();
    let response = String::from_utf8_lossy(&response);
    let status = response.lines().next().unwrap_or("<empty>");
    if status.contains(" 201 ") {
        trace_upload(
            "rust_async",
            &path,
            connect_us,
            write_us,
            read_us,
            total_started.elapsed().as_micros(),
        );
        Ok(())
    } else {
        Err(format!("upload {path} failed: {status}").into())
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn payload(bytes: usize) -> Vec<u8> {
    (0..bytes).map(|index| b'a' + (index % 26) as u8).collect()
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
    info!(mode, phase = "connect", path, elapsed_us = connect_us, "upload_phase");
    info!(mode, phase = "write_request", path, elapsed_us = write_us, "upload_phase");
    info!(mode, phase = "read_response", path, elapsed_us = read_us, "upload_phase");
    info!(mode, phase = "total", path, elapsed_us = total_us, "upload_phase");
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
