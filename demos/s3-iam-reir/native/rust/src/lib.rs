use std::env;

use rsscript_runtime::{spawn_tokio_native, NativeAsyncPending};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39090";

pub fn put_object_start(
    bucket: &String,
    key: &String,
    body: &String,
) -> NativeAsyncPending<Result<(), String>> {
    let endpoint = env::var("RSS_S3_DEMO_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let bucket = bucket.clone();
    let key = key.clone();
    let body = body.clone();

    spawn_tokio_native(async move { put_object(&endpoint, &bucket, &key, &body).await })
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
    let path = format!("/{}/{}", clean_path_segment(bucket), clean_key(key));
    let request = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
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
