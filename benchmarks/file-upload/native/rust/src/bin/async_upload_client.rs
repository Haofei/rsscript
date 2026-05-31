use std::env;
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39190";
const DEFAULT_REQUESTS: usize = 24;
const DEFAULT_PAYLOAD_BYTES: usize = 64 * 1024;
const DEFAULT_CONCURRENCY: usize = 8;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let endpoint = env::var("RSS_FILE_UPLOAD_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let requests = env_usize("RSS_FILE_UPLOAD_REQUESTS", DEFAULT_REQUESTS);
    let payload_bytes = env_usize("RSS_FILE_UPLOAD_PAYLOAD_BYTES", DEFAULT_PAYLOAD_BYTES);
    let concurrency = env_usize("RSS_FILE_UPLOAD_CONCURRENCY", DEFAULT_CONCURRENCY);
    let payload = Arc::new(payload(payload_bytes));
    let limiter = Arc::new(Semaphore::new(concurrency));
    let started = Instant::now();
    let mut tasks = Vec::with_capacity(requests);

    for index in 0..requests {
        let permit = limiter.clone().acquire_owned().await?;
        let endpoint = endpoint.clone();
        let payload = payload.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = permit;
            upload_one(&endpoint, index, &payload).await
        }));
    }

    for task in tasks {
        task.await??;
    }

    let elapsed = started.elapsed();
    let rps = requests as f64 / elapsed.as_secs_f64();
    println!(
        "mode=rust_async requests={requests} payload_bytes={payload_bytes} concurrency={concurrency} elapsed_ms={} rps={rps:.2}",
        elapsed.as_millis()
    );
    Ok(())
}

async fn upload_one(endpoint: &str, index: usize, payload: &[u8]) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut stream = TcpStream::connect(endpoint).await?;
    let path = format!("/upload/rust-{index}.bin");
    let header = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(payload).await?;
    stream.shutdown().await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8_lossy(&response);
    let status = response.lines().next().unwrap_or("<empty>");
    if status.contains(" 201 ") {
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
