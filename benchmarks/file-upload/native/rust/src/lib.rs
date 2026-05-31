use std::env;

use rsscript_runtime::{NativeAsyncPending, spawn_tokio_native};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39190";
const DEFAULT_PAYLOAD_BYTES: usize = 64 * 1024;

pub fn upload_start(path: &String, body: &String) -> NativeAsyncPending<Result<(), String>> {
    let endpoint =
        env::var("RSS_FILE_UPLOAD_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let path = path.clone();
    let body = expanded_body(body);

    spawn_tokio_native(async move { upload_one(&endpoint, &path, &body).await })
}

async fn upload_one(endpoint: &str, path: &str, body: &str) -> Result<(), String> {
    let mut stream = TcpStream::connect(endpoint)
        .await
        .map_err(|err| format!("connect {endpoint}: {err}"))?;
    let header = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|err| format!("write header {path}: {err}"))?;
    stream
        .write_all(body.as_bytes())
        .await
        .map_err(|err| format!("write body {path}: {err}"))?;
    stream
        .shutdown()
        .await
        .map_err(|err| format!("shutdown {path}: {err}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("read response {path}: {err}"))?;
    let response = String::from_utf8_lossy(&response);
    let status = response.lines().next().unwrap_or("<empty>");
    if status.contains(" 201 ") {
        Ok(())
    } else {
        Err(format!("upload {path} failed: {status}"))
    }
}

fn expanded_body(seed: &str) -> String {
    let target = env::var("RSS_FILE_UPLOAD_PAYLOAD_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAYLOAD_BYTES);
    if seed.len() >= target {
        return seed.to_string();
    }

    let mut body = String::with_capacity(target);
    while body.len() < target {
        body.push_str(seed);
        body.push('\n');
    }
    body.truncate(target);
    body
}
