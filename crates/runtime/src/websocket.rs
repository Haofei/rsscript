use std::sync::Arc;

use crate::{
    NativeAsyncPending, ResourceBudget, RssCancellationToken, RssDeadline, cancellation_never,
    cancellation_token_cancelled, deadline_after_ms, deadline_remaining_duration,
    spawn_tokio_native,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type WebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WebSocketWriter = futures_util::stream::SplitSink<WebSocketStream, Message>;
type WebSocketReader = futures_util::stream::SplitStream<WebSocketStream>;
const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketError {
    message: String,
}

impl WebSocketError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn websocket_error_message(error: &WebSocketError) -> String {
    error.message.clone()
}

pub struct RssWebSocket {
    reader: Arc<tokio::sync::Mutex<WebSocketReader>>,
    writer: Arc<tokio::sync::Mutex<WebSocketWriter>>,
}

pub fn websocket_connect(url: &str) -> NativeAsyncPending<Result<RssWebSocket, WebSocketError>> {
    websocket_connect_with_resources(
        url,
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_connect_with_resources(
    url: &str,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<RssWebSocket, WebSocketError>> {
    let url = url.to_string();
    spawn_tokio_native(async move {
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES));
        let (stream, _) = websocket_with_controls(
            async {
                tokio_tungstenite::connect_async_with_config(&url, Some(config), false)
                    .await
                    // Connection errors can echo the request URI, including
                    // credentials. Keep the URL out of diagnostics.
                    .map_err(|_| WebSocketError::new("WebSocket connection failed"))
            },
            &cancellation,
            &deadline,
            "connect",
        )
        .await?;
        let (writer, reader) = stream.split();
        Ok(RssWebSocket {
            reader: Arc::new(tokio::sync::Mutex::new(reader)),
            writer: Arc::new(tokio::sync::Mutex::new(writer)),
        })
    })
}

pub fn websocket_send_text(
    socket: &RssWebSocket,
    text: &str,
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    websocket_send_text_with_resources(
        socket,
        text,
        ResourceBudget::new(MAX_WEBSOCKET_MESSAGE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_send_text_with_resources(
    socket: &RssWebSocket,
    text: &str,
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let writer = Arc::clone(&socket.writer);
    let text = text.to_string();
    spawn_tokio_native(async move {
        budget.try_consume(text.len()).map_err(|error| {
            WebSocketError::new(format!(
                "WebSocket send text byte budget exhausted: {error}"
            ))
        })?;
        websocket_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer
                    .send(Message::Text(text.into()))
                    .await
                    .map_err(|error| {
                        WebSocketError::new(format!("WebSocket send text failed: {error}"))
                    })
            },
            &cancellation,
            &deadline,
            "send text",
        )
        .await
    })
}

pub fn websocket_send_bytes(
    socket: &RssWebSocket,
    bytes: &[u8],
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    websocket_send_bytes_with_resources(
        socket,
        bytes,
        ResourceBudget::new(MAX_WEBSOCKET_MESSAGE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_send_bytes_with_resources(
    socket: &RssWebSocket,
    bytes: &[u8],
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let writer = Arc::clone(&socket.writer);
    let bytes = bytes.to_vec();
    spawn_tokio_native(async move {
        budget.try_consume(bytes.len()).map_err(|error| {
            WebSocketError::new(format!(
                "WebSocket send bytes byte budget exhausted: {error}"
            ))
        })?;
        websocket_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer
                    .send(Message::Binary(bytes.into()))
                    .await
                    .map_err(|error| {
                        WebSocketError::new(format!("WebSocket send bytes failed: {error}"))
                    })
            },
            &cancellation,
            &deadline,
            "send bytes",
        )
        .await
    })
}

pub fn websocket_recv_text(
    socket: &RssWebSocket,
) -> NativeAsyncPending<Result<Option<String>, WebSocketError>> {
    websocket_recv_text_with_resources(
        socket,
        ResourceBudget::new(MAX_WEBSOCKET_MESSAGE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_recv_text_with_resources(
    socket: &RssWebSocket,
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<Option<String>, WebSocketError>> {
    let reader = Arc::clone(&socket.reader);
    spawn_tokio_native(async move {
        websocket_with_controls(
            websocket_recv_text_inner(reader, &budget),
            &cancellation,
            &deadline,
            "receive text",
        )
        .await
    })
}

async fn websocket_recv_text_inner(
    reader: Arc<tokio::sync::Mutex<WebSocketReader>>,
    budget: &ResourceBudget,
) -> Result<Option<String>, WebSocketError> {
    loop {
        let next = {
            let mut reader = reader.lock().await;
            reader.next().await
        };
        match next {
            Some(Ok(Message::Text(text))) => {
                budget.try_consume(text.len()).map_err(|error| {
                    WebSocketError::new(format!(
                        "WebSocket receive text byte budget exhausted: {error}"
                    ))
                })?;
                return Ok(Some(text.to_string()));
            }
            Some(Ok(Message::Close(_))) | None => return Ok(None),
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Binary(_))) => {
                return Err(WebSocketError::new(
                    "WebSocket received binary frame while waiting for text",
                ));
            }
            Some(Ok(Message::Frame(_))) => {}
            Some(Err(error)) => {
                return Err(WebSocketError::new(format!(
                    "WebSocket receive text failed: {error}"
                )));
            }
        }
    }
}

pub fn websocket_recv_bytes(
    socket: &RssWebSocket,
) -> NativeAsyncPending<Result<Option<Vec<u8>>, WebSocketError>> {
    websocket_recv_bytes_with_resources(
        socket,
        ResourceBudget::new(MAX_WEBSOCKET_MESSAGE_BYTES as u64),
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_recv_bytes_with_resources(
    socket: &RssWebSocket,
    budget: ResourceBudget,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<Option<Vec<u8>>, WebSocketError>> {
    let reader = Arc::clone(&socket.reader);
    spawn_tokio_native(async move {
        websocket_with_controls(
            websocket_recv_bytes_inner(reader, &budget),
            &cancellation,
            &deadline,
            "receive bytes",
        )
        .await
    })
}

async fn websocket_recv_bytes_inner(
    reader: Arc<tokio::sync::Mutex<WebSocketReader>>,
    budget: &ResourceBudget,
) -> Result<Option<Vec<u8>>, WebSocketError> {
    loop {
        let next = {
            let mut reader = reader.lock().await;
            reader.next().await
        };
        match next {
            Some(Ok(Message::Binary(bytes))) => {
                budget.try_consume(bytes.len()).map_err(|error| {
                    WebSocketError::new(format!(
                        "WebSocket receive bytes byte budget exhausted: {error}"
                    ))
                })?;
                return Ok(Some(bytes.to_vec()));
            }
            Some(Ok(Message::Close(_))) | None => return Ok(None),
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Text(_))) => {
                return Err(WebSocketError::new(
                    "WebSocket received text frame while waiting for bytes",
                ));
            }
            Some(Ok(Message::Frame(_))) => {}
            Some(Err(error)) => {
                return Err(WebSocketError::new(format!(
                    "WebSocket receive bytes failed: {error}"
                )));
            }
        }
    }
}

async fn websocket_with_controls<T>(
    future: impl std::future::Future<Output = Result<T, WebSocketError>>,
    cancellation: &RssCancellationToken,
    deadline: &RssDeadline,
    operation: &str,
) -> Result<T, WebSocketError> {
    let remaining = deadline_remaining_duration(deadline);
    if remaining.is_zero() {
        return Err(WebSocketError::new(format!(
            "WebSocket {operation} deadline expired"
        )));
    }
    tokio::select! {
        biased;
        _ = cancellation_token_cancelled(cancellation) => {
            Err(WebSocketError::new(format!("WebSocket {operation} was cancelled")))
        }
        result = tokio::time::timeout(remaining, future) => {
            result.map_err(|_| {
                WebSocketError::new(format!("WebSocket {operation} deadline expired"))
            })?
        }
    }
}

pub fn websocket_close(socket: &RssWebSocket) -> NativeAsyncPending<Result<(), WebSocketError>> {
    websocket_close_with_resources(
        socket,
        cancellation_never(),
        deadline_after_ms(DEFAULT_WEBSOCKET_OPERATION_TIMEOUT_MS),
    )
}

pub fn websocket_close_with_resources(
    socket: &RssWebSocket,
    cancellation: RssCancellationToken,
    deadline: RssDeadline,
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let writer = Arc::clone(&socket.writer);
    spawn_tokio_native(async move {
        websocket_with_controls(
            async {
                let mut writer = writer.lock().await;
                writer.close().await.map_err(|error| {
                    WebSocketError::new(format!("WebSocket close failed: {error}"))
                })
            },
            &cancellation,
            &deadline,
            "close",
        )
        .await
    })
}

#[cfg(test)]
mod tests {
    use crate::{Executor, tokio_native_runtime};
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn websocket_round_trips_text_on_native_runtime() {
        let listener = tokio_native_runtime()
            .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
            .expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = tokio_native_runtime().spawn(async move {
            let (stream, _) = listener.accept().await.expect("client should connect");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket should accept");
            match websocket.next().await {
                Some(Ok(Message::Text(text))) => {
                    assert_eq!(text.as_str(), "ping");
                    websocket
                        .send(Message::Text("pong".into()))
                        .await
                        .expect("server should send");
                }
                other => panic!("unexpected websocket message: {other:?}"),
            }
        });

        let mut executor = Executor::new();
        let socket = executor
            .run_pending(super::websocket_connect(&format!("ws://127.0.0.1:{port}")))
            .expect("websocket connect should succeed");
        executor
            .run_pending(super::websocket_send_text(&socket, "ping"))
            .expect("websocket send should succeed");
        let response = executor
            .run_pending(super::websocket_recv_text(&socket))
            .expect("websocket recv should succeed");
        assert_eq!(response, Some("pong".to_string()));
        tokio_native_runtime()
            .block_on(server)
            .expect("server task should finish");
    }

    #[test]
    fn pending_websocket_receive_does_not_block_sends() {
        let listener = tokio_native_runtime()
            .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
            .expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = tokio_native_runtime().spawn(async move {
            let (stream, _) = listener.accept().await.expect("client should connect");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket should accept");
            let incoming = tokio::time::timeout(Duration::from_secs(1), websocket.next())
                .await
                .expect("client send should not be blocked by its pending receive");
            match incoming {
                Some(Ok(Message::Text(text))) => assert_eq!(text.as_str(), "ping"),
                other => panic!("unexpected websocket message: {other:?}"),
            }
            websocket
                .send(Message::Text("pong".into()))
                .await
                .expect("server should send");
        });

        let mut executor = Executor::new();
        let socket = executor
            .run_pending(super::websocket_connect(&format!("ws://127.0.0.1:{port}")))
            .expect("websocket connect should succeed");
        let receive = super::websocket_recv_text(&socket);
        std::thread::sleep(Duration::from_millis(20));
        executor
            .run_pending(super::websocket_send_text(&socket, "ping"))
            .expect("send should progress while receive is pending");
        assert_eq!(
            executor
                .run_pending(receive)
                .expect("pending receive should complete"),
            Some("pong".to_string())
        );
        tokio_native_runtime()
            .block_on(server)
            .expect("server task should finish");
    }

    #[test]
    fn websocket_receive_consumes_shared_budget() {
        let listener = tokio_native_runtime()
            .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
            .expect("test listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should have addr")
            .port();
        let server = tokio_native_runtime().spawn(async move {
            let (stream, _) = listener.accept().await.expect("client should connect");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket should accept");
            websocket
                .send(Message::Binary(vec![1, 2, 3, 4].into()))
                .await
                .expect("server should send");
        });

        let mut executor = Executor::new();
        let socket = executor
            .run_pending(super::websocket_connect(&format!("ws://127.0.0.1:{port}")))
            .expect("websocket connect should succeed");
        let error = executor
            .run_pending(super::websocket_recv_bytes_with_resources(
                &socket,
                crate::ResourceBudget::new(2),
                crate::cancellation_never(),
                crate::deadline_after_ms(1_000),
            ))
            .expect_err("message should exceed shared budget");
        assert!(error.message.contains("byte budget exhausted"));
        tokio_native_runtime()
            .block_on(server)
            .expect("server task should finish");
    }

    #[test]
    fn websocket_connect_errors_do_not_expose_url_secrets() {
        let url = "not-websocket://user:password@example.invalid/socket?token=secret";
        let error = match Executor::new().run_pending(super::websocket_connect(url)) {
            Ok(_) => panic!("invalid WebSocket scheme should fail"),
            Err(error) => error,
        };
        let message = super::websocket_error_message(&error);

        assert_eq!(message, "WebSocket connection failed");
        assert!(!message.contains("password"));
        assert!(!message.contains("secret"));
        assert!(!message.contains(url));
    }
}
