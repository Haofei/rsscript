//! Spec §3/§10.6 — eval≡lowered parity: network IO (HTTP/TCP/WebSocket)
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_sync_http_error_intrinsics() {
    let source = r#"

fn main(args: read List<String>) -> Unit {
    let url = Url.from_string(value: read "https://example.test/api")
    match Http.get(url: read url) {
        Ok(response) => {
            Output.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "https://example.test/api") {
                Output.write(message: read "get-error")
            }
        }
    }
    match Http.post_json(url: read url, body: read "{\"ok\":true}") {
        Ok(response) => {
            Output.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "POST JSON https://example.test/api") {
                Output.write(message: read "post-json-error")
            }
        }
    }
    match Http.post_form(url: read url, body: read "a=1") {
        Ok(response) => {
            Output.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "POST form https://example.test/api") {
                Output.write(message: read "post-form-error")
            }
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-http-sync.rss",
        "rsscript_parity_http_sync",
        source,
    );
}

#[test]
fn parity_async_http_error_intrinsics() {
    let source = r#"

fn log_http_error(error: read HttpError, label: read String) -> Unit {
    let message = HttpError.message(error: read error)
    if String.contains(value: read message, needle: read "") {
        Output.write(message: read label)
    }
    return Unit
}

async fn main(args: read List<String>) -> Unit {
    match await Http.get_async(url: read Url.from_string(value: read "https://example.test/api")) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "get-async-error")
        }
    }
    match await Http.get_timeout_async(url: read Url.from_string(value: read "https://example.test/api"), timeout_ms: 1000) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "get-timeout-error")
        }
    }
    match await Http.get_retry_async(url: read Url.from_string(value: read "https://example.test/api"), timeout_ms: 1000, attempts: 2, backoff_ms: 1) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "get-retry-error")
        }
    }
    match await Http.post_json_async(url: read Url.from_string(value: read "https://example.test/api"), body: read "{\"ok\":true}") {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "post-json-async-error")
        }
    }
    match await Http.post_json_timeout_async(url: read Url.from_string(value: read "https://example.test/api"), body: read "{\"ok\":true}", timeout_ms: 1000) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "post-json-timeout-error")
        }
    }
    match await Http.post_json_retry_async(url: read Url.from_string(value: read "https://example.test/api"), body: read "{\"ok\":true}", timeout_ms: 1000, attempts: 2, backoff_ms: 1) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "post-json-retry-error")
        }
    }
    match await Http.post_json_bearer_retry_async(url: read Url.from_string(value: read "https://example.test/api"), body: read "{\"ok\":true}", token: read "token", timeout_ms: 1000, attempts: 2, backoff_ms: 1) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "post-json-bearer-error")
        }
    }
    match await Http.post_form_async(url: read Url.from_string(value: read "https://example.test/api"), body: read "a=1") {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "post-form-async-error")
        }
    }
    local request = HttpRequest.json(url: read Url.from_string(value: read "https://example.test/api"), body: read "{\"ok\":true}")
    match await Http.send_async(request: take request) {
        Ok(_) => {}
        Err(error) => {
            log_http_error(error: read error, label: read "send-async-error")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-http-async.rss",
        "rsscript_parity_http_async",
        source,
    );
}

#[test]
fn parity_http_response_intrinsics() {
    common::run_with_large_stack(|| {
        let (interpreter_port, interpreter_server) = common::spawn_http_response_server();
        let (backend_port, backend_server) = common::spawn_http_response_server();
        let interpreter_url = format!("http://127.0.0.1:{interpreter_port}/health");
        let backend_url = format!("http://127.0.0.1:{backend_port}/health");
        let source = r#"

fn url_arg(args: read List<String>) -> Url {
    return Url.from_string(value: read Arguments.get_or_default(args: read args, index: 0, default: read "http://127.0.0.1:1/health"))
}

async fn main(args: read List<String>) -> Result<Unit, HttpError> {
    let response = await Http.get_async(url: read url_arg(args: read args))?
    Output.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
    Output.write(message: read HttpResponse.text(response: read response))
    Output.write(message: read String.from_int(value: Bytes.len(value: read HttpResponse.bytes(response: read response))))
    let lines = HttpResponse.lines(response: read response)
    Output.write(message: read String.from_int(value: List.len<String>(list: read lines)))
    if HttpResponse.is_success(response: read response) {
        Output.write(message: read "success")
    }
    return Ok(Unit)
}
"#;
        common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
            "parity-http-response.rss",
            "rsscript_parity_http_response",
            source,
            &[interpreter_url.as_str()],
            &[backend_url.as_str()],
        );
        interpreter_server
            .join()
            .expect("interpreter http server should finish");
        backend_server
            .join()
            .expect("backend http server should finish");
    });
}

#[test]
fn parity_websocket_intrinsics() {
    common::run_with_large_stack(|| {
        let (interpreter_port, interpreter_server) = common::spawn_websocket_echo_server();
        let (backend_port, backend_server) = common::spawn_websocket_echo_server();
        let interpreter_url = format!("ws://127.0.0.1:{interpreter_port}/socket");
        let backend_url = format!("ws://127.0.0.1:{backend_port}/socket");
        let source = r#"

fn url_arg(args: read List<String>) -> Url {
    return Url.from_string(value: read Arguments.get_or_default(args: read args, index: 0, default: read "ws://127.0.0.1:1/socket"))
}

async fn main(args: read List<String>) -> Result<Unit, WebSocketError> {
    let socket = await WebSocket.connect(url: read url_arg(args: read args))?
    await WebSocket.send_text(socket: read socket, text: read "ping")?
    let text = await WebSocket.recv_text(socket: read socket)?
    Output.write(message: read Option.unwrap_or<String>(value: read text, default: read "text-none"))
    await WebSocket.send_bytes(socket: read socket, bytes: read String.to_bytes(value: read "bin"))?
    let bytes = await WebSocket.recv_bytes(socket: read socket)?
    let bytes = Option.unwrap_or<Bytes>(value: read bytes, default: read String.to_bytes(value: read ""))
    Output.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    await WebSocket.close(socket: read socket)?
    Output.write(message: read "closed")
    return Ok(Unit)
}
"#;
        common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
            "parity-websocket.rss",
            "rsscript_parity_websocket",
            source,
            &[interpreter_url.as_str()],
            &[backend_url.as_str()],
        );
        interpreter_server
            .join()
            .expect("interpreter websocket server should finish");
        backend_server
            .join()
            .expect("backend websocket server should finish");
    });
}

#[test]
fn parity_async_socket_error_intrinsics() {
    let source = r#"

async fn main(args: read List<String>) -> Unit {
    match await Tcp.connect(host: read "127.0.0.1", port: 9) {
        Ok(_) => {}
        Err(error) => {
            let message = TcpError.message(error: read error)
            if String.contains(value: read message, needle: read "") {
                Output.write(message: read "tcp-error")
            }
        }
    }
    let url = Url.from_string(value: read "ws://127.0.0.1:9/socket")
    match await WebSocket.connect(url: read url) {
        Ok(_) => {}
        Err(error) => {
            let message = WebSocketError.message(error: read error)
            if String.contains(value: read message, needle: read "") {
                Output.write(message: read "websocket-error")
            }
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-async-socket.rss",
        "rsscript_parity_async_socket",
        source,
    );
}

#[test]
fn parity_tcp_stream_intrinsics() {
    let (interpreter_port, interpreter_server) = common::spawn_tcp_echo_server();
    let (backend_port, backend_server) = common::spawn_tcp_echo_server();
    let source = r#"

fn port_arg(args: read List<String>) -> Int {
    match String.parse_int(value: read Arguments.get_or_default(args: read args, index: 0, default: read "0")) {
        Some(port) => {
            return port
        }
        None => {
            return 0
        }
    }
}

async fn main(args: read List<String>) -> Result<Unit, TcpError> {
    let port = port_arg(args: read args)
    let stream = await Tcp.connect(host: read "127.0.0.1", port: port)?
    let written = await TcpStream.write(stream: read stream, data: read String.to_bytes(value: read ""))?
    Output.write(message: read String.from_int(value: written))
    await TcpStream.write_all(stream: read stream, data: read String.to_bytes(value: read "ping"))?
    let response = await TcpStream.read(stream: read stream, max_bytes: 4)?
    Output.write(message: read String.from_int(value: Bytes.len(value: read response)))
    await TcpStream.shutdown(stream: read stream)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-tcp-stream.rss",
        "rsscript_parity_tcp_stream",
        source,
        &[interpreter_port.as_str()],
        &[backend_port.as_str()],
    );
    interpreter_server
        .join()
        .expect("interpreter tcp server should finish");
    backend_server
        .join()
        .expect("backend tcp server should finish");
}

#[test]
fn parity_http_request_builder_intrinsics() {
    let source = r#"

fn main(args: read List<String>) -> Unit {
    let url = Url.from_string(value: read "https://example.test/api")
    local base = HttpRequest.json(url: read url, body: read "{\"ok\":true}")
    local timed = HttpRequest.with_timeout(request: take base, timeout_ms: 250)
    local retry = HttpRequest.with_retry(request: take timed, attempts: 3, backoff_ms: 50)
    local with_header = HttpRequest.with_header(request: take retry, name: read "X-Test", value: read "rss")
    HttpRequest.with_header(request: take with_header, name: read "X-Trace", value: read "1")
    Output.write(message: read "http-request-built")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-http-request.rss",
        "rsscript_parity_http_request",
        source,
    );
}
