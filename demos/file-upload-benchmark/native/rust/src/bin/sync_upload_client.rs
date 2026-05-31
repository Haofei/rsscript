use std::env;
use std::error::Error;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Instant;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:39190";
const DEFAULT_REQUESTS: usize = 24;
const DEFAULT_PAYLOAD_BYTES: usize = 64 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let endpoint = env::var("RSS_FILE_UPLOAD_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let requests = env_usize("RSS_FILE_UPLOAD_REQUESTS", DEFAULT_REQUESTS);
    let payload_bytes = env_usize("RSS_FILE_UPLOAD_PAYLOAD_BYTES", DEFAULT_PAYLOAD_BYTES);
    let payload = payload(payload_bytes);
    let started = Instant::now();

    for index in 0..requests {
        upload_one(&endpoint, index, &payload)?;
    }

    let elapsed = started.elapsed();
    let rps = requests as f64 / elapsed.as_secs_f64();
    println!(
        "mode=sync requests={requests} payload_bytes={payload_bytes} concurrency=1 elapsed_ms={} rps={rps:.2}",
        elapsed.as_millis()
    );
    Ok(())
}

fn upload_one(endpoint: &str, index: usize, payload: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut stream = TcpStream::connect(endpoint)?;
    let path = format!("/upload/sync-{index}.bin");
    let header = format!(
        "PUT {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(payload)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
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
