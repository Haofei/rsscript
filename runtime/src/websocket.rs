use std::sync::Arc;

use crate::{NativeAsyncPending, spawn_tokio_native};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

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
    stream: Arc<
        tokio::sync::Mutex<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    >,
}

pub fn websocket_connect(url: &str) -> NativeAsyncPending<Result<RssWebSocket, WebSocketError>> {
    let url = url.to_string();
    spawn_tokio_native(async move {
        let (stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|error| {
                WebSocketError::new(format!("WebSocket connect to `{url}` failed: {error}"))
            })?;
        Ok(RssWebSocket {
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
        })
    })
}

pub fn websocket_send_text(
    socket: &RssWebSocket,
    text: &str,
) -> NativeAsyncPending<Result<(), WebSocketError>> {
    let stream = Arc::clone(&socket.stream);
    let text = text.to_string();
    spawn_tokio_native(async move {
        let mut stream = stream.lock().await;
        stream
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
    let stream = Arc::clone(&socket.stream);
    let bytes = bytes.to_vec();
    spawn_tokio_native(async move {
        let mut stream = stream.lock().await;
        stream
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
    let stream = Arc::clone(&socket.stream);
    spawn_tokio_native(async move {
        loop {
            let next = {
                let mut stream = stream.lock().await;
                stream.next().await
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
    let stream = Arc::clone(&socket.stream);
    spawn_tokio_native(async move {
        loop {
            let next = {
                let mut stream = stream.lock().await;
                stream.next().await
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
    let stream = Arc::clone(&socket.stream);
    spawn_tokio_native(async move {
        let mut stream = stream.lock().await;
        stream
            .close(None)
            .await
            .map_err(|error| WebSocketError::new(format!("WebSocket close failed: {error}")))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use crate::{Executor, tokio_native_runtime};
    use futures_util::{SinkExt, StreamExt};
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
}
