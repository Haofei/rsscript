use std::env;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream as StdTcpStream};

use rsscript_runtime::{spawn_tokio_native, NativeAsyncPending};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39090";
const DEFAULT_PAYLOAD_BYTES: usize = 256 * 1024;

pub fn put_object_start(
    bucket: &String,
    key: &String,
    body: &String,
) -> NativeAsyncPending<Result<(), String>> {
    let endpoint = env::var("RSS_S3_DEMO_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let bucket = bucket.clone();
    let key = key.clone();
    let body = expanded_body(body);

    spawn_tokio_native(async move { put_object(&endpoint, &bucket, &key, &body).await })
}

pub fn default_endpoint() -> String {
    env::var("RSS_S3_DEMO_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

pub fn benchmark_payload(seed: &str) -> String {
    expanded_body(seed)
}

pub fn put_object_blocking(
    endpoint: &str,
    bucket: &str,
    key: &str,
    body: &str,
) -> Result<(), String> {
    let mut stream =
        StdTcpStream::connect(endpoint).map_err(|err| format!("connect {endpoint}: {err}"))?;
    let path = object_path(bucket, key);
    let request = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("write PUT {path}: {err}"))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|err| format!("shutdown PUT {path}: {err}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|err| format!("read PUT {path} response: {err}"))?;
    let status = response.lines().next().unwrap_or("<empty response>");
    if status.contains(" 200 ") {
        Ok(())
    } else {
        Err(format!("mock S3 rejected PUT {path}: {status}"))
    }
}

async fn put_object(
    endpoint: &str,
    bucket: &str,
    key: &str,
    body: &str,
) -> Result<(), String> {
    let mut stream = TcpStream::connect(endpoint)
        .await
        .map_err(|err| format!("connect {endpoint}: {err}"))?;
    let path = object_path(bucket, key);
    let request = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("write PUT {path}: {err}"))?;
    stream
        .shutdown()
        .await
        .map_err(|err| format!("shutdown PUT {path}: {err}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("read PUT {path} response: {err}"))?;
    let response = String::from_utf8_lossy(&response);
    let status = response.lines().next().unwrap_or("<empty response>");
    if status.contains(" 200 ") {
        Ok(())
    } else {
        Err(format!("mock S3 rejected PUT {path}: {status}"))
    }
}

fn expanded_body(seed: &str) -> String {
    let target = env::var("RSS_S3_DEMO_PAYLOAD_BYTES")
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

fn object_path(bucket: &str, key: &str) -> String {
    format!("/{}/{}", clean_path_segment(bucket), clean_key(key))
}

fn clean_path_segment(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect()
}

fn clean_key(value: &str) -> String {
    value
        .trim_start_matches('/')
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
        .collect()
}
