use std::env;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DEFAULT_ADDR: &str = "127.0.0.1:39190";

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = env::var("RSS_FILE_UPLOAD_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let delay_ms = env::var("RSS_FILE_UPLOAD_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(50);
    let listener = TcpListener::bind(&addr).await?;
    let state = Arc::new(ServerState {
        delay: Duration::from_millis(delay_ms),
        ..ServerState::default()
    });
    println!("file upload server listening on {addr} delay_ms={delay_ms}");

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state).await {
                eprintln!("upload request failed: {error}");
            }
        });
    }
}

#[derive(Default)]
struct ServerState {
    total: AtomicUsize,
    in_flight: AtomicUsize,
    delay: Duration,
}

async fn handle_connection(
    mut stream: TcpStream,
    state: Arc<ServerState>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let guard = InFlightGuard::new(state.clone());
    let request = read_http_request(&mut stream).await?;
    tokio::time::sleep(state.delay).await;

    let total = state.total.fetch_add(1, Ordering::SeqCst) + 1;
    println!(
        "upload #{total} method={} path={} bytes={} in_flight={}",
        request.method, request.path, request.body_len, guard.current
    );

    stream
        .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await?;
    Ok(())
}

struct InFlightGuard {
    state: Arc<ServerState>,
    current: usize,
}

impl InFlightGuard {
    fn new(state: Arc<ServerState>) -> Self {
        let current = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        Self { state, current }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct UploadRequest {
    method: String,
    path: String,
    body_len: usize,
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> Result<UploadRequest, Box<dyn Error + Send + Sync>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if request_complete(&buffer) {
            break;
        }
    }

    let header_end = find_header_end(&buffer).ok_or("missing HTTP header terminator")?;
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let request_line = headers.lines().next().ok_or("missing request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().ok_or("missing method")?.to_string();
    let path = request_parts.next().ok_or("missing path")?.to_string();
    let content_length = content_length(&headers);
    let received_body = buffer.len().saturating_sub(header_end + 4);
    if received_body != content_length {
        return Err(format!("body length mismatch: declared={content_length} received={received_body}").into());
    }

    Ok(UploadRequest {
        method,
        path,
        body_len: received_body,
    })
}

fn request_complete(buffer: &[u8]) -> bool {
    let Some(header_end) = find_header_end(buffer) else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    buffer.len() >= header_end + 4 + content_length(&headers)
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
