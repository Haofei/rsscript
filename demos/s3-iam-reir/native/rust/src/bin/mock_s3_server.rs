use std::env;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DEFAULT_ADDR: &str = "127.0.0.1:39090";

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = env::var("RSS_S3_DEMO_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let delay_ms = env::var("RSS_S3_DEMO_SERVER_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(150);
    let listener = TcpListener::bind(&addr).await?;
    let state = Arc::new(ServerState {
        delay: Duration::from_millis(delay_ms),
        ..ServerState::default()
    });
    println!("mock s3 server listening on {addr} delay_ms={delay_ms}");

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, state).await {
                eprintln!("mock s3 request failed: {err}");
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
    let current = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    let request = read_http_request(&mut stream).await?;
    tokio::time::sleep(state.delay).await;

    let total = state.total.fetch_add(1, Ordering::SeqCst) + 1;
    println!(
        "stored object #{total} path={} bytes={} in_flight={current}",
        request.path,
        request.body_len
    );

    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
    stream.write_all(response.as_bytes()).await?;
    state.in_flight.fetch_sub(1, Ordering::SeqCst);
    Ok(())
}

struct HttpRequest {
    path: String,
    body_len: usize,
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> Result<HttpRequest, Box<dyn Error + Send + Sync>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

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
    let mut lines = headers.lines();
    let request_line = lines.next().ok_or("missing request line")?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or("missing request path")?
        .to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);

    Ok(HttpRequest {
        path,
        body_len: content_length,
    })
}

fn request_complete(buffer: &[u8]) -> bool {
    let Some(header_end) = find_header_end(buffer) else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    buffer.len() >= header_end + 4 + content_length
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
