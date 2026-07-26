use std::sync::Arc;

use crate::{NativeAsyncPending, spawn_tokio_native};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type WebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WebSocketWriter = futures_util::stream::SplitSink<WebSocketStream, Message>;
type WebSocketReader = futures_util::stream::SplitStream<WebSocketStream>;

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
    let url = url.to_string();
    spawn_tokio_native(async move {
        let (stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|error| {
                WebSocketError::new(format!("WebSocket connect to `{url}` failed: {error}"))
            })?;
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
    let writer = Arc::clone(&socket.writer);
    let text = text.to_string();
    spawn_tokio_native(async move {
        let mut writer = writer.lock().await;
        writer
            .send(Message::Text(text.into()))
            .await
            .map_err(|error| WebSocketError::new(format!("WebSocket send text failed: {error}")))?;
        Ok(())
    })
}

pub fn websocket_send_bytes(
    socket: &RssWebSocket,
    bytes: &[u8],
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let writer = Arc::clone(&socket.writer);
    let bytes = bytes.to_vec();
    spawn_tokio_native(async move {
        let mut writer = writer.lock().await;
        writer
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|error| {
                WebSocketError::new(format!("WebSocket send bytes failed: {error}"))
            })?;
        Ok(())
    })
}

pub fn websocket_recv_text(
    socket: &RssWebSocket,
) -> NativeAsyncPending<Result<Option<String>, WebSocketError>> {
    let reader = Arc::clone(&socket.reader);
    spawn_tokio_native(async move {
        loop {
            let next = {
                let mut reader = reader.lock().await;
                reader.next().await
            };
            match next {
                Some(Ok(Message::Text(text))) => return Ok(Some(text.to_string())),
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
    })
}

pub fn websocket_recv_bytes(
    socket: &RssWebSocket,
) -> NativeAsyncPending<Result<Option<Vec<u8>>, WebSocketError>> {
    let reader = Arc::clone(&socket.reader);
    spawn_tokio_native(async move {
        loop {
            let next = {
                let mut reader = reader.lock().await;
                reader.next().await
            };
            match next {
                Some(Ok(Message::Binary(bytes))) => return Ok(Some(bytes.to_vec())),
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
    })
}

pub fn websocket_close(socket: &RssWebSocket) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let writer = Arc::clone(&socket.writer);
    spawn_tokio_native(async move {
        let mut writer = writer.lock().await;
        writer
            .close()
            .await
            .map_err(|error| WebSocketError::new(format!("WebSocket close failed: {error}")))?;
        Ok(())
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
}
