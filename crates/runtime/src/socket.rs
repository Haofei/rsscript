use std::sync::Arc;

use crate::network::{NetworkEndpoint, NetworkOperationContext, authorize_resolved_target};
use crate::{
    NativeAsyncPending, ResourceBudget, RssCancellationToken, RssDeadline, cancellation_never,
    deadline_after_ms, spawn_tokio_native,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub use crate::network::{
    AllowAllNetworkTargetPolicy, DenyPrivateNetworkTargetPolicy, NetworkTargetPolicy,
};

const MAX_TCP_READ_BYTES: i64 = 16 * 1024 * 1024;
const MAX_TCP_WRITE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_TCP_OPERATION_TIMEOUT_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpError {
    message: String,
}

impl TcpError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn tcp_error_message(error: &TcpError) -> String {
    error.message.clone()
}

#[derive(Clone)]
pub struct RssTcpStream {
    reader: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedReadHalf>>,
    writer: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
}

pub fn tcp_connect(host: &str, port: i64) -> NativeAsyncPending<Result<RssTcpStream, TcpError>> {
    tcp_connect_with_resources(
        host,
        port,
        cancellation_never(),
        deadline_after_ms(DEFAULT_TCP_OPERATION_TIMEOUT_MS),
    )
}

pub fn tcp_connect_with_resources(
    host: &str,
    port: i64,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<RssTcpStream, TcpError>> {
    tcp_connect_with_policy_and_resources(
        host,
        port,
        Arc::new(AllowAllNetworkTargetPolicy),
        cancellation,
        deadline,
    )
}

pub fn tcp_connect_with_policy_and_resources(
    host: &str,
    port: i64,
    policy: Arc<dyn NetworkTargetPolicy>,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<RssTcpStream, TcpError>> {
    let host = host.to_string();
    let context = NetworkOperationContext::from_controls(cancellation, deadline);
    spawn_tokio_native(async move {
        let endpoint = NetworkEndpoint::from_host_and_port(&host, port)
            .ok_or_else(|| TcpError::new("TCP port must be between 1 and 65535"))?;
        let addresses = tcp_with_controls(
            tokio::net::lookup_host((endpoint.hostname(), endpoint.port())),
            &context,
            "resolve",
        )
        .await?
        .map_err(|error| TcpError::new(format!("TCP target resolution failed: {error}")))?
        .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(TcpError::new("TCP target did not resolve to an address"));
        }
        authorize_resolved_target(policy.as_ref(), &endpoint, &addresses).map_err(TcpError::new)?;
        let stream = tcp_with_controls(
            tokio::net::TcpStream::connect(addresses.as_slice()),
            &context,
            "connect",
        )
        .await?
        .map_err(|error| TcpError::new(format!("TCP connect failed: {error}")))?;
        let (reader, writer) = stream.into_split();
        Ok(RssTcpStream {
            reader: Arc::new(tokio::sync::Mutex::new(reader)),
            writer: Arc::new(tokio::sync::Mutex::new(writer)),
        })
    })
}

pub fn tcp_stream_read(
    stream: &RssTcpStream,
    max_bytes: i64,
) -> NativeAsyncPending<Result<Vec<u8>, TcpError>> {
    tcp_stream_read_with_resources(
        stream,
        max_bytes,
        ResourceBudget::new(MAX_TCP_READ_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_TCP_OPERATION_TIMEOUT_MS),
    )
}

pub fn tcp_stream_read_with_resources(
    stream: &RssTcpStream,
    max_bytes: i64,
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<Vec<u8>, TcpError>> {
    let reader = Arc::clone(&stream.reader);
    let context = NetworkOperationContext::from_resources(budget, cancellation, deadline);
    spawn_tokio_native(async move {
        tcp_with_controls(
            tcp_stream_read_inner(reader, max_bytes, context.byte_budget()),
            &context,
            "read",
        )
        .await?
    })
}

async fn tcp_stream_read_inner(
    reader: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedReadHalf>>,
    max_bytes: i64,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, TcpError> {
    if max_bytes <= 0 {
        return Err(TcpError::new("TCP read max_bytes must be positive"));
    }
    if max_bytes > MAX_TCP_READ_BYTES {
        return Err(TcpError::new(format!(
            "TCP read max_bytes must not exceed {MAX_TCP_READ_BYTES}"
        )));
    }
    let capacity = usize::try_from(max_bytes)
        .map_err(|_| TcpError::new("TCP read max_bytes does not fit this platform"))?;
    let reservation = budget
        .try_reserve(capacity)
        .map_err(|error| TcpError::new(format!("TCP read byte budget exhausted: {error}")))?;
    let mut buffer = vec![0; capacity];
    let mut reader = reader.lock().await;
    let read = reader
        .read(&mut buffer)
        .await
        .map_err(|error| TcpError::new(format!("TCP read failed: {error}")))?;
    buffer.truncate(read);
    reservation.commit(read);
    Ok(buffer)
}

pub fn tcp_stream_write(
    stream: &RssTcpStream,
    data: &[u8],
) -> NativeAsyncPending<Result<i64, TcpError>> {
    tcp_stream_write_with_resources(
        stream,
        data,
        ResourceBudget::new(MAX_TCP_WRITE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_TCP_OPERATION_TIMEOUT_MS),
    )
}

pub fn tcp_stream_write_with_resources(
    stream: &RssTcpStream,
    data: &[u8],
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<i64, TcpError>> {
    let writer = Arc::clone(&stream.writer);
    let data = data.to_vec();
    let context = NetworkOperationContext::from_resources(budget, cancellation, deadline);
    spawn_tokio_native(async move {
        if data.len() > MAX_TCP_WRITE_BYTES {
            return Err(TcpError::new("TCP write exceeds the runtime ceiling"));
        }
        context
            .byte_budget()
            .try_consume(data.len())
            .map_err(|error| TcpError::new(format!("TCP write byte budget exhausted: {error}")))?;
        let written = tcp_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer.write(&data).await
            },
            &context,
            "write",
        )
        .await?
        .map_err(|error| TcpError::new(format!("TCP write failed: {error}")))?;
        Ok(written as i64)
    })
}

pub fn tcp_stream_write_all(
    stream: &RssTcpStream,
    data: &[u8],
) -> NativeAsyncPending<Result<(), TcpError>> {
    tcp_stream_write_all_with_resources(
        stream,
        data,
        ResourceBudget::new(MAX_TCP_WRITE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_TCP_OPERATION_TIMEOUT_MS),
    )
}

pub fn tcp_stream_write_all_with_resources(
    stream: &RssTcpStream,
    data: &[u8],
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<(), TcpError>> {
    let writer = Arc::clone(&stream.writer);
    let data = data.to_vec();
    let context = NetworkOperationContext::from_resources(budget, cancellation, deadline);
    spawn_tokio_native(async move {
        if data.len() > MAX_TCP_WRITE_BYTES {
            return Err(TcpError::new("TCP write_all exceeds the runtime ceiling"));
        }
        context
            .byte_budget()
            .try_consume(data.len())
            .map_err(|error| {
                TcpError::new(format!("TCP write_all byte budget exhausted: {error}"))
            })?;
        tcp_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer.write_all(&data).await
            },
            &context,
            "write_all",
        )
        .await?
        .map_err(|error| TcpError::new(format!("TCP write_all failed: {error}")))?;
        Ok(())
    })
}

pub fn tcp_stream_shutdown(stream: &RssTcpStream) -> NativeAsyncPending<Result<(), TcpError>> {
    tcp_stream_shutdown_with_resources(
        stream,
        cancellation_never(),
        deadline_after_ms(DEFAULT_TCP_OPERATION_TIMEOUT_MS),
    )
}

pub fn tcp_stream_shutdown_with_resources(
    stream: &RssTcpStream,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<(), TcpError>> {
    let writer = Arc::clone(&stream.writer);
    let context = NetworkOperationContext::from_controls(cancellation, deadline);
    spawn_tokio_native(async move {
        tcp_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer.shutdown().await
            },
            &context,
            "shutdown",
        )
        .await?
        .map_err(|error| TcpError::new(format!("TCP shutdown failed: {error}")))?;
        Ok(())
    })
}

async fn tcp_with_controls<T>(
    future: impl std::future::Future<Output = T>,
    context: &NetworkOperationContext,
    operation: &str,
) -> Result<T, TcpError> {
    context
        .run(future)
        .await
        .map_err(|error| TcpError::new(error.message("TCP", operation)))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::Executor;

    #[test]
    fn tcp_stream_round_trips_bytes_on_native_runtime() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("client should connect");
            let mut buffer = [0; 4];
            socket.read_exact(&mut buffer).expect("server should read");
            assert_eq!(&buffer, b"ping");
            socket.write_all(b"pong").expect("server should write");
        });

        let mut executor = Executor::new();
        let stream = executor
            .run_pending(super::tcp_connect("127.0.0.1", i64::from(port)))
            .expect("TCP connect should succeed");
        executor
            .run_pending(super::tcp_stream_write_all(&stream, b"ping"))
            .expect("TCP write should succeed");
        let response = executor
            .run_pending(super::tcp_stream_read(&stream, 4))
            .expect("TCP read should succeed");
        assert_eq!(response, b"pong");
        server.join().expect("server thread should finish");
    }

    #[test]
    fn strict_network_policy_rejects_loopback_before_connecting() {
        let error = match Executor::new().run_pending(super::tcp_connect_with_policy_and_resources(
            "127.0.0.1",
            9,
            Arc::new(super::DenyPrivateNetworkTargetPolicy),
            crate::cancellation_never(),
            crate::deadline_after_ms(1_000),
        )) {
            Ok(_) => panic!("strict network policy must reject loopback"),
            Err(error) => error,
        };

        assert_eq!(
            super::tcp_error_message(&error),
            "network target resolves to a non-public address"
        );
    }

    #[test]
    fn strict_network_policy_rejects_mixed_public_and_private_answers() {
        let policy = super::DenyPrivateNetworkTargetPolicy;
        let endpoint = crate::network::NetworkEndpoint::from_host_and_port("mixed.example", 443)
            .expect("valid endpoint");
        let error = super::authorize_resolved_target(
            &policy,
            &endpoint,
            &[
                "8.8.8.8:443".parse().expect("public address"),
                "127.0.0.1:443".parse().expect("loopback address"),
            ],
        )
        .expect_err("one denied answer must reject the target");

        assert_eq!(error, "network target resolves to a non-public address");
    }

    #[test]
    fn pending_tcp_read_does_not_block_writes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("client should connect");
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("server read timeout should configure");
            let mut buffer = [0; 4];
            socket
                .read_exact(&mut buffer)
                .expect("server should receive concurrent client write");
            assert_eq!(&buffer, b"ping");
            socket.write_all(b"pong").expect("server should write");
        });

        let mut executor = Executor::new();
        let stream = executor
            .run_pending(super::tcp_connect("127.0.0.1", i64::from(port)))
            .expect("TCP connect should succeed");
        let read = super::tcp_stream_read(&stream, 4);
        std::thread::sleep(Duration::from_millis(20));
        executor
            .run_pending(super::tcp_stream_write_all(&stream, b"ping"))
            .expect("write should progress while read is pending");
        let response = executor
            .run_pending(read)
            .expect("pending read should complete");
        assert_eq!(response, b"pong");
        server.join().expect("server thread should finish");
    }

    #[test]
    fn tcp_read_rejects_allocation_beyond_shared_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("client should connect");
            socket.write_all(b"data").expect("server should write");
        });

        let mut executor = Executor::new();
        let stream = executor
            .run_pending(super::tcp_connect("127.0.0.1", i64::from(port)))
            .expect("TCP connect should succeed");
        let error = executor
            .run_pending(super::tcp_stream_read_with_resources(
                &stream,
                4,
                crate::ResourceBudget::new(2),
                crate::cancellation_never(),
                crate::deadline_after_ms(1_000),
            ))
            .expect_err("read allocation should exceed the budget");
        assert!(error.message.contains("byte budget exhausted"));
        server.join().expect("server thread should finish");
    }

    #[test]
    fn pending_tcp_read_completes_when_cancelled() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().expect("client should connect");
            std::thread::sleep(Duration::from_millis(200));
        });

        let mut executor = Executor::new();
        let stream = executor
            .run_pending(super::tcp_connect("127.0.0.1", i64::from(port)))
            .expect("TCP connect should succeed");
        let mut source = crate::cancellation_source_new();
        let token = crate::cancellation_source_token(&source);
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            crate::cancellation_source_cancel(&mut source);
        });
        let started = std::time::Instant::now();
        let error = executor
            .run_pending(super::tcp_stream_read_with_resources(
                &stream,
                4,
                crate::ResourceBudget::new(4),
                token,
                crate::deadline_after_ms(1_000),
            ))
            .expect_err("cancelled read should complete with an error");
        assert!(error.message.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_millis(150));
        cancel.join().expect("cancellation thread should finish");
        server.join().expect("server thread should finish");
    }
}
