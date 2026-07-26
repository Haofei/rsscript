//! Spec §5A/§3 — async/await, streams, task groups, select lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_wraps_async_control_flow_with_await_in_pending_boundary() {
    let source = r#"
features: async

async fn run(flag: Bool) -> Result<Unit, TimerError> {
    if flag {
        await Timer.sleep(ms: 1)?
    } else {
        await Timer.sleep(ms: 2)?
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("async-control.rss", source).expect("source should lower");

    assert!(
        rust.contains("impl rsscript_runtime::Pending<Result<(), rsscript_runtime::TimerError>>")
    );
    assert!(rust.contains("rsscript_runtime::pending_defer(move ||"));
    assert!(rust.contains("__rsscript_async_executor.run_pending"));
}

#[test]
fn rust_lowering_maps_async_file_io_to_tokio_pending_hooks() {
    let source = r#"
features: async

async fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "/tmp/rsscript-async-file.txt")
    await File.write_string_async(path: read path, text: read "hello")?
    let text = await File.read_all_string_async(path: read path)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("async-file.rss", source).expect("async file IO should lower");

    assert!(
        rust.contains("let path = rsscript_runtime::path_from_string(&(\"/tmp/rsscript-async-file.txt\".to_string())); rsscript_runtime::pending_try(rsscript_runtime::file_write_string_async(&(path), &(\"hello\".to_string())), move |_rsscript_unit| { rsscript_runtime::pending_try(rsscript_runtime::file_read_all_string_async(&(path)), move |text| { rsscript_runtime::pending_ready(Ok(())) }) }) })"),
        "async file write should lower to runtime pending hook, got:\n{rust}"
    );
    assert!(
        rust.contains("let path = rsscript_runtime::path_from_string(&(\"/tmp/rsscript-async-file.txt\".to_string())); rsscript_runtime::pending_try(rsscript_runtime::file_write_string_async(&(path), &(\"hello\".to_string())), move |_rsscript_unit| { rsscript_runtime::pending_try(rsscript_runtime::file_read_all_string_async(&(path)), move |text| { rsscript_runtime::pending_ready(Ok(())) }) }) })"),
        "async file read should lower to runtime pending hook, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::pending_try(")
            && rust.contains("rsscript_runtime::run_pending("),
        "async file IO should compose through RSScript pending chain, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_async_timer_await_to_runtime_pending() {
    let source = r#"
features: async

async fn main() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("async-timer.rss", source).expect("source should lower");

    assert!(rust.contains("pub fn main() -> Result<(), rsscript_runtime::TimerError>"));
    assert!(!rust.contains("async fn"));
    // The async entrypoint drives a pending chain via `run_pending`; the body
    // composes the sleep with `pending_try` and has no inline executor.
    assert!(rust.contains("rsscript_runtime::run_pending("));
    assert!(rust.contains(
        "rsscript_runtime::pending_try(rsscript_runtime::timer_sleep_native_start(1i64)"
    ));
    assert!(!rust.contains("__rsscript_async_executor"));
}

#[test]
fn rust_lowering_omits_async_executor_without_await() {
    let source = r#"
features: async

async fn tick() -> Unit {
    return Unit
}
"#;
    let rust = lower_source_to_rust("async-no-await.rss", source).expect("source should lower");

    assert!(rust.contains("fn tick()"));
    assert!(!rust.contains("__rsscript_async_executor"));
}

#[test]
fn rust_lowering_wraps_async_native_bindings_in_runtime_pending() {
    let source = r#"
features: async, native

struct HostError

async native fn Host.wait(ms: Int) -> Result<Unit, HostError>
    effects(native)

async fn main() -> Result<Unit, HostError> {
    await Host.wait(ms: 1)?
    return Ok(Unit)
}
"#;
    let package = lower_sources_to_rust_package_with_options(
        &[("async-native.rss".to_string(), source.to_string())],
        "Async Native Example.rss",
        "/workspace/rsscript/runtime",
        &[],
        &[NativeRustDependency {
            crate_name: "timer_native".to_string(),
            path: "/workspace/timer-native".to_string(),
            cargo_features: Vec::new(),
            default_features: true,
            bindings: BTreeMap::from([(
                "Host.wait".to_string(),
                "timer_native::sleep_start".to_string(),
            )]),
        }],
    )
    .expect("source should lower with async native binding");

    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::pending_try(timer_native::sleep_start(1i64)")
    );
    assert!(package.lib_rs.contains("rsscript_runtime::run_pending("));
}

#[test]
fn rust_lowering_maps_async_process_calls_to_runtime_pending_hooks() {
    let source = r#"
features: async

async fn run_command() -> Result<String, String> {
    let args = List<String>.new()
    List.push(list: mut args, value: read "hello")
    let stdout = await Process.run_stdout_async(command: read "printf", args: read args)?
    return Ok(stdout)
}
"#;
    let rust = lower_source_to_rust("async-process.rss", source).expect("source should lower");

    assert!(rust.contains("-> impl rsscript_runtime::Pending<Result<String, String>>"));
    assert!(
        rust.contains("rsscript_runtime::pending_try(rsscript_runtime::process_run_stdout_async"),
        "async process call should lower to runtime pending hook, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_async_process_request_calls_to_runtime_pending_hooks() {
    let source = r#"
features: async

async fn run_command(token: read CancellationToken) -> Result<ProcessOutput, String> {
    let request = ProcessRequest(
        command: "printf",
        args: ["hello"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: true,
        output_cap_bytes: 4096,
    )
    let output = await Process.run_request_cancellable_async(request: read request, token: read token)?
    return Ok(output)
}
"#;
    let rust =
        lower_source_to_rust("async-process-request.rss", source).expect("source should lower");

    assert!(rust.contains(
        "-> impl rsscript_runtime::Pending<Result<rsscript_runtime::ProcessOutput, String>>"
    ));
    assert!(
        rust.contains(
            "rsscript_runtime::pending_try(rsscript_runtime::process_run_request_cancellable_async"
        ),
        "async process request call should lower to runtime pending hook, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_process_stream_to_runtime_stream() {
    let source = r#"
features: async

async fn run_command() -> Result<Unit, String> {
    let request = ProcessRequest(
        command: "printf",
        args: ["hello"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: true,
        output_cap_bytes: 4096,
    )
    let events: Stream<ProcessEvent> = Process.stream(request: read request)?
    match await Stream.next(stream: read events) {
        Ok(first) => {
            match first {
                Some(event) => {
                    Log.write(message: read event.kind)
                }
                None => {
                    Log.write(message: read "done")
                }
            }
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("process-stream.rss", source).expect("source should lower");

    assert!(rust.contains("let request = rsscript_runtime::ProcessRequest { command: \"printf\".to_string(), args: vec![\"hello\".to_string()], cwd: None, stdin: None, env: rsscript_runtime::list_new(), timeout_ms: 1000i64, merge_stderr: true, output_cap_bytes: 4096i64 }; rsscript_runtime::pending_try(rsscript_runtime::pending_ready((|| -> Result<rsscript_runtime::RssStream<rsscript_runtime::ProcessEvent>, String> { Ok(rsscript_runtime::process_stream(&(request))?) })()), move |events| { rsscript_runtime::pending_try(rsscript_runtime::pending_defer(move || {"));
    assert!(rust.contains(
        "match __rsscript_async_executor.run_pending(rsscript_runtime::stream_next(&(events))) {"
    ));
    assert!(rust.contains("rsscript_runtime::ProcessEvent"));
}

#[test]
fn rust_lowering_maps_async_tcp_calls_to_runtime_pending_hooks() {
    let source = r#"
features: async

async fn ping() -> Result<Bytes, TcpError> {
    let stream = await Tcp.connect(host: read "127.0.0.1", port: 8080)?
    let payload = Bytes.from_string(value: read "ping")
    await TcpStream.write_all(stream: read stream, data: read payload)?
    let response = await TcpStream.read(stream: read stream, max_bytes: 4)?
    await TcpStream.shutdown(stream: read stream)?
    return Ok(response)
}
"#;
    let rust = lower_source_to_rust("async-tcp.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "fn ping() -> impl rsscript_runtime::Pending<Result<Vec<u8>, rsscript_runtime::TcpError>>"
        ),
        "async TCP function should lower to pending, got:\n{rust}"
    );
    assert!(rust.contains("rsscript_runtime::tcp_connect"));
    assert!(rust.contains("rsscript_runtime::tcp_stream_write_all"));
    assert!(rust.contains("rsscript_runtime::tcp_stream_read"));
    assert!(rust.contains("rsscript_runtime::tcp_stream_shutdown"));
}

#[test]
fn rust_lowering_maps_async_websocket_calls_to_runtime_pending_hooks() {
    let source = r#"
features: async

async fn exchange(url: read Url) -> Result<Option<String>, WebSocketError> {
    let socket = await WebSocket.connect(url: read url)?
    await WebSocket.send_text(socket: read socket, text: read "ping")?
    let response = await WebSocket.recv_text(socket: read socket)?
    await WebSocket.close(socket: read socket)?
    return Ok(response)
}
"#;
    let rust = lower_source_to_rust("async-websocket.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "fn exchange(url: &String) -> impl rsscript_runtime::Pending<Result<Option<String>, rsscript_runtime::WebSocketError>>"
        ),
        "async WebSocket function should lower to pending, got:\n{rust}"
    );
    assert!(rust.contains("rsscript_runtime::websocket_connect"));
    assert!(rust.contains("rsscript_runtime::websocket_send_text"));
    assert!(rust.contains("rsscript_runtime::websocket_recv_text"));
    assert!(rust.contains("rsscript_runtime::websocket_close"));
}

#[test]
fn rust_lowering_maps_async_http_client_to_tokio_pending_hooks() {
    let source = r#"
features: async

async fn fetch_status(url: read Url) -> Result<Int, HttpError> {
    let response = await Http.get_async(url: read url)?
    return Ok(HttpResponse.status(response: read response))
}
"#;
    let rust = lower_source_to_rust("async-http.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "-> impl rsscript_runtime::Pending<Result<i64, rsscript_runtime::HttpError>>"
        ),
        "async fn should lower to a pending chain, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::pending_try(rsscript_runtime::http_get_async(url)"),
        "async HTTP client call should use the Tokio-backed pending hook, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::http_response_status(&(response))"),
        "response helpers should still map through runtime hooks, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_agent_async_io_helpers() {
    let source = r#"
features: async, native, local

async fn call_agent(url: read Url) -> Result<String, HttpError> {
    let fields = List<String>.new()
    List.push(list: mut fields, value: read Json.string_field(name: read "input", value: read "hello"))
    List.push(list: mut fields, value: read Json.int_field(name: read "max_steps", value: 2))
    List.push(list: mut fields, value: read Json.bool_field(name: read "stream", value: false))
    let body = Json.object(fields: read fields)
    local request = HttpRequest.json(url: read url, body: read body)
    local authorized = HttpRequest.with_header(request: take request, name: read "Authorization", value: read "Bearer test_key")
    local timed = HttpRequest.with_timeout(request: take authorized, timeout_ms: 5000)
    local retried = HttpRequest.with_retry(request: take timed, attempts: 3, backoff_ms: 100)
    let response = await Http.send_async(request: take retried)?
    Log.trace(event: read "agent.response", message: read HttpResponse.text(response: read response))
    let lines = HttpResponse.lines(response: read response)
    return Ok(HttpResponse.text(response: read response))
}

async fn probe(url: read Url) -> Result<Int, HttpError> {
    let response = await Http.get_timeout_async(url: read url, timeout_ms: 1000)?
    let retried = await Http.get_retry_async(url: read url, timeout_ms: 1000, attempts: 2, backoff_ms: 10)?
    return Ok(HttpResponse.status(response: read response) + HttpResponse.status(response: read retried))
}
"#;
    let rust = lower_source_to_rust("agent-async-io.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::json_string_field"));
    assert!(rust.contains("rsscript_runtime::json_int_field"));
    assert!(rust.contains("rsscript_runtime::json_bool_field"));
    assert!(rust.contains("let body = rsscript_runtime::json_object(&(fields)); { // rss:span kind=statement file=agent-async-io.rss line=10 column=5 length=5"));
    assert!(rust.contains("rsscript_runtime::http_request_json"));
    assert!(rust.contains("rsscript_runtime::http_request_with_header"));
    assert!(rust.contains("rsscript_runtime::http_request_with_timeout"));
    assert!(rust.contains("rsscript_runtime::http_request_with_retry"));
    assert!(rust.contains("rsscript_runtime::http_send_async"));
    assert!(rust.contains("rsscript_runtime::http_get_timeout_async"));
    assert!(rust.contains("rsscript_runtime::http_get_retry_async"));
    assert!(rust.contains("rsscript_runtime::http_response_lines"));
    assert!(rust.contains("rsscript_runtime::log_trace"));
}

#[test]
fn rust_lowering_rejects_unbound_async_native_calls_before_generation() {
    let source = r#"
features: async, native

struct HostError

async native fn Host.wait(ms: Int) -> Result<Unit, HostError>
    effects(native)

async fn main() -> Result<Unit, HostError> {
    await Host.wait(ms: 1)?
    return Ok(Unit)
}
"#;
    let diagnostics = lower_source_to_rust("unbound-async-native.rss", source)
        .expect_err("unbound native async call should fail before Rust generation");

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "unbound native declaration call"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn review_map_marks_async_signatures_review_required() {
    let source = r#"
features: async

async fn ping(url: read Url) -> Result<Unit, NetworkError>
"#;

    let map = review_map_sources(vec![("async-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "ping")
        .expect("expected ping region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "async function boundary"),
        "{region:?}"
    );
    assert_eq!(map.summary.foldable.functions, 0);
    assert_eq!(map.summary.review_required.functions, 1);
}

#[test]
fn review_map_marks_spawn_retention_boundary() {
    let source = r#"
features: async

async fn fetch_user(client: read HttpClient, id: read UserId) -> Result<fresh User, HttpError>

fn schedule(client: read HttpClient, id: read UserId) -> Unit {
    let task = spawn fetch_user(client: read client, id: read id)
    return Unit
}
"#;

    let map = review_map_sources(vec![("spawn-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "schedule")
        .expect("expected schedule region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "spawn task boundary"),
        "{region:?}"
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "spawn retains-until-task-complete `client`, `id`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_await_suspension_boundary() {
    let source = r#"
features: async

async fn fetch_user(client: read HttpClient, id: read UserId) -> Result<fresh User, HttpError>

fn receive(client: read HttpClient, id: read UserId) -> Unit {
    let user = await fetch_user(client: read client, id: read id)
    return Unit
}
"#;

    let map = review_map_sources(vec![("await-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "receive")
        .expect("expected receive region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "await suspension boundary"),
        "{region:?}"
    );
}

#[test]
fn parser_preserves_async_function_kind() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>
"#;
    let program = parse_source("net.rssi", source);

    assert!(
        matches!(&program.items[0], Item::Function(function) if function.name == "fetch" && function.is_async)
    );
    assert!(analyze_source("net.rssi", source).is_empty());
}

#[test]
fn parser_preserves_spawn_expression() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let program = parse_source("spawn.rss", source);
    let schedule = program
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(function) if function.name == "schedule" => Some(function),
            _ => None,
        })
        .expect("expected schedule function");
    let stmt = schedule
        .body
        .statements
        .first()
        .expect("expected task binding");

    assert!(matches!(
        stmt,
        rsscript::syntax::ast::Stmt::Let(let_stmt)
            if matches!(let_stmt.value.as_ref(), Some(Expr::Spawn { .. }))
    ));
}

#[test]
fn parser_preserves_await_expression() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = await fetch(url: read url)
    return Unit
}
"#;
    let program = parse_source("await.rss", source);
    let receive = program
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(function) if function.name == "receive" => Some(function),
            _ => None,
        })
        .expect("expected receive function");
    let stmt = receive
        .body
        .statements
        .first()
        .expect("expected bytes binding");

    assert!(matches!(
        stmt,
        rsscript::syntax::ast::Stmt::Let(let_stmt)
            if matches!(let_stmt.value.as_ref(), Some(Expr::Await { .. }))
    ));
}

#[test]
fn task_group_lowers_to_pending_pattern() {
    let source = r#"
features: async

struct NetworkError { message: String }

async fn fetch_user(id: read Int) -> Result<String, NetworkError> {
    return Ok("user")
}

async fn fetch_profile(id: read Int) -> Result<String, NetworkError> {
    return Ok("profile")
}

fn load(id: read Int) -> Result<String, NetworkError> {
    task_group {
        async let user = fetch_user(id: read id)
        async let profile = fetch_profile(id: read id)

        let u = await user?
        await profile?
    }
    return Ok("done")
}
"#;
    let lowered = lower_source_to_rust("task-group.rss", source).expect("task_group should lower");
    // Each async-let constructs a pending and a result slot; the group drives
    // them together with one concurrent poll loop; awaits read the cached result.
    assert!(
        lowered.contains("let mut __rsscript_pending_user = fetch_user(id);"),
        "async let should construct (not run) the pending, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_result_user") && lowered.contains("__rsscript_result_profile"),
        "async let should produce a result slot, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_async_executor.poll_once_keyed(__rsscript_key_user")
            && lowered.contains("__rsscript_async_executor.yield_once()"),
        "task_group should cooperatively drive siblings from keyed polls, got:\n{lowered}"
    );
    assert!(
        lowered
            .contains("let u = __rsscript_result_user.take().expect(\"awaited task completed\")?;"),
        "await should read the cached result, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_result_profile.take().expect(\"awaited task completed\")?;"),
        "expression-statement await should read its cached result, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::TaskGroupScope::new()"),
        "task_group should create the cancellation guard, got:\n{lowered}"
    );
}

#[test]
fn task_group_async_let_materializes_borrowed_temporaries() {
    let source = r#"
features: async

struct NetworkError { message: String }

async fn upload(key: read String) -> Result<Unit, NetworkError> {
    return Ok(Unit)
}

fn run() -> Result<Unit, NetworkError> {
    task_group {
        async let summary = upload(key: read "reports/summary.json")
        await summary?
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("task-group-temp.rss", source)
        .expect("task_group with borrowed temporary should lower");

    assert!(
        lowered
            .contains("let __rsscript_async_arg_summary_0 = \"reports/summary.json\".to_string();"),
        "async let should materialize borrowed temporaries before constructing a pending, got:\n{lowered}"
    );
    assert!(
        lowered.contains(
            "let mut __rsscript_pending_summary = upload(&(__rsscript_async_arg_summary_0));"
        ),
        "async let should borrow the materialized local, got:\n{lowered}"
    );
    assert!(
        !lowered.contains("upload(&(\"reports/summary.json\".to_string()))"),
        "async let should not store a pending that borrows a temporary, got:\n{lowered}"
    );
}

#[test]
fn async_fn_with_task_group_lowers_to_pending_boundary() {
    let source = r#"
features: async

async fn worker() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}

async fn run() -> Result<Unit, TimerError> {
    task_group {
        async let child = worker()
        await child?
    }
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("async-fn-task-group.rss", source).expect("source should lower");

    assert!(
        lowered.contains(
            "fn run() -> impl rsscript_runtime::Pending<Result<(), rsscript_runtime::TimerError>>"
        ),
        "async orchestrator should still return a Pending, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::pending_defer(move ||")
            && lowered.contains("rsscript_runtime::pending_try("),
        "task_group should become an explicit pending boundary in the async chain, got:\n{lowered}"
    );
    assert!(
        lowered.contains("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();")
            && lowered.contains("rsscript_runtime::TaskGroupScope::new()")
            && lowered.contains("let mut __rsscript_pending_child = worker();"),
        "task_group boundary should keep the internal cooperative executor, got:\n{lowered}"
    );
    assert!(
        lowered.contains("Ok::<(), rsscript_runtime::TimerError>(())"),
        "task_group pending boundary should expose a Result<Unit, E> for `?`, got:\n{lowered}"
    );
}

#[test]
fn async_loop_preserves_statements_before_await_pending() {
    let source = r#"
features: async

async fn run(url: read Url) -> Result<Unit, HttpError> {
    let mut step = 0
    while step < 2 {
        let body = Json.encode(value: read {
            "step": step,
        })
        let response = await Http.post_json_bearer_retry_async(
            url: read url,
            body: read body,
            token: read "test_key",
            timeout_ms: 1000,
            attempts: 1,
            backoff_ms: 0,
        )?
        Log.write(message: read HttpResponse.text(response: read response))
        step = step + 1
    }
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("async-loop-prefix.rss", source).expect("source should lower");

    assert!(
        lowered.contains("let body = rsscript_runtime::json_object"),
        "async loop should emit statements before the await inside the poll loop, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_loop_6_5_pending = Some(rsscript_runtime::http_post_json_bearer_retry_async(url, &(body), &(\"test_key\".to_string()), 1000i64, 1i64, 0i64));"),
        "await pending should be constructed after the per-iteration body binding, got:\n{lowered}"
    );
    assert!(
        !lowered.contains("cannot find value") && !lowered.contains("&request_body"),
        "async loop lowering should not reference a dropped pre-await binding, got:\n{lowered}"
    );
}

#[test]
fn task_group_await_only_polls_already_declared_siblings() {
    // An await emitted before a later `async let` must not poll that later
    // task's pending — doing so forward-references a binding that does not yet
    // exist and leaks `cannot find value __rsscript_pending_b` into rustc.
    let source = r#"
features: async

fn run() -> Result<Unit, TimerError> {
    task_group {
        async let a = Timer.sleep(ms: 1)
        await a?
        async let b = Timer.sleep(ms: 1)
        await b?
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("task-group-order.rss", source)
        .expect("interleaved task_group should lower");

    let decl_b = lowered
        .find("let mut __rsscript_pending_b")
        .expect("b's pending should be declared, got:\n{lowered}");
    if let Some(poll_b) = lowered.find("poll_once_keyed(__rsscript_key_b") {
        assert!(
            poll_b > decl_b,
            "await on `a` must not poll `b` before it is declared, got:\n{lowered}"
        );
    }
    // After taking `a`'s result, the await on `b` must no longer poll `a`
    // (its result slot was reset by `take`).
    let take_a = lowered
        .find("__rsscript_result_a.take()")
        .expect("a should be taken, got:\n{lowered}");
    let poll_a_after = lowered[take_a..].find("poll_once_keyed(__rsscript_key_a");
    assert!(
        poll_a_after.is_none(),
        "an awaited task must not be re-polled after its result is taken, got:\n{lowered}"
    );
}

#[test]
fn task_group_discarded_async_let_lowers_to_scoped_background_drain() {
    let source = r#"
features: async

fn run() -> Result<Unit, TimerError> {
    task_group {
        async let _ = Timer.sleep(ms: 1)
    }
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("task-group-background.rss", source).expect("background should lower");
    assert!(
        lowered.contains("let mut __rsscript_pending_discard_0 ="),
        "discarded async let should get an internal pending slot, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_result_discard_0")
            && lowered.contains("__rsscript_all_done")
            && lowered.contains("poll_once_keyed(__rsscript_key_discard_0"),
        "task_group should drain discarded background tasks before scope exit, got:\n{lowered}"
    );
}

#[test]
fn select_lowers_to_first_ready_poll_loop() {
    let source = r#"
features: async

fn run() -> Result<Unit, TimerError> {
    select {
        _ = await Timer.sleep(ms: 1)? => {
            Log.write(message: read "fast")
        }
        _ = await Timer.sleep(ms: 100)? => {
            Log.write(message: read "slow")
        }
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("select.rss", source).expect("select should lower");

    assert!(
        lowered.contains("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();"),
        "select in a sync function should create a cooperative executor, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::timer_sleep_native_start(1i64)")
            && lowered.contains("rsscript_runtime::timer_sleep_native_start(100i64)"),
        "select arms should construct pending operations without running them inline, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_async_executor.poll_once(&mut __rsscript_select_")
            && lowered.contains("break 0;")
            && lowered.contains("break 1;"),
        "select should poll arms and break with the first ready arm, got:\n{lowered}"
    );
    assert!(
        lowered.contains("match __rsscript_select_")
            && lowered.contains(".take().expect(\"select arm completed\")?;"),
        "select should execute the winning arm and apply `?` at the arm boundary, got:\n{lowered}"
    );
}

#[test]
fn async_fn_with_select_lowers_to_pending_boundary() {
    let source = r#"
features: async

async fn run() -> Result<Unit, TimerError> {
    select {
        _ = await Timer.sleep(ms: 1)? => {
            Log.write(message: read "fast")
        }
        _ = await Timer.sleep(ms: 100)? => {
            Log.write(message: read "slow")
        }
    }
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("async-select.rss", source).expect("async select should lower");
    assert!(
        lowered.contains(
            "fn run() -> impl rsscript_runtime::Pending<Result<(), rsscript_runtime::TimerError>>"
        ),
        "async select function should return a Pending, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::pending_defer(move ||")
            && lowered.contains("rsscript_runtime::pending_try(")
            && lowered
                .contains("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();"),
        "select should lower through an explicit async boundary, got:\n{lowered}"
    );
    assert!(
        lowered.contains("__rsscript_async_executor.poll_once")
            && lowered.contains("Ok::<(), rsscript_runtime::TimerError>(())"),
        "select boundary should poll arms and expose Result<Unit, E>, got:\n{lowered}"
    );
}

#[test]
fn stream_from_receiver_lowers_to_runtime_pending_next() {
    let source = r#"
features: async, local

fn run() -> Result<Unit, ChannelError> {
    let mut channel: Channel<Int> = Channel.bounded(capacity: 1)?
    local receiver = Channel.receiver(channel: mut channel)?
    let stream: Stream<Int> = Receiver.into_stream(receiver: take receiver)
    select {
        item = await Stream.next(stream: read stream)? => {
            Log.write(message: read "stream item")
        }
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("stream-next.rss", source).expect("stream should lower");
    assert!(
        lowered.contains("let stream: rsscript_runtime::RssStream<i64> ="),
        "typed Stream<Int> let should emit a concrete Rust annotation, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::receiver_into_stream(receiver)"),
        "Receiver.into_stream should lower to the runtime hook, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::stream_next(&(stream))"),
        "Stream.next should lower to a pending runtime hook, got:\n{lowered}"
    );
}

#[test]
fn stream_sources_and_collect_list_lower_to_runtime_pipeline_hooks() {
    let source = r#"
features: async, local

fn collect_numbers() -> Result<fresh List<Int>, ChannelError> {
    local items = List<Int>.new()
    List.push(list: mut items, value: read 1)
    List.push(list: mut items, value: read 2)
    let stream: Stream<Int> = Stream.from_list(items: take items)
    return Stream.collect_list(stream: read stream)?
}

fn read_chunks(path: read Path) -> Result<Unit, ChannelError> {
    let chunks: Stream<Bytes> = File.bytes_stream(path: read path, chunk_size: 4096)?
    await for chunk in chunks {
        Log.write(message: read "chunk")
    }
    return Ok(Unit)
}

fn read_rows(path: read Path) -> Result<Unit, ChannelError> {
    let rows: Stream<Row> = Csv.rows(path: read path, buffer_size: 8192)?
    await for row in rows {
        let name = Row.field_string(row: read row, index: 0)
        Log.write(message: read "row")
    }
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("stream-sources.rss", source).expect("stream sources should lower");

    assert!(lowered.contains("rsscript_runtime::stream_from_list(items)"));
    assert!(lowered.contains("return Ok(rsscript_runtime::stream_collect_list(&(stream))?);"));
    assert!(lowered.contains("rsscript_runtime::file_bytes_stream(path, 4096i64)?"));
    assert!(lowered.contains("let chunks: rsscript_runtime::RssStream<Vec<u8>> ="));
    assert!(lowered.contains("rsscript_runtime::csv_rows(path, 8192i64)?"));
    assert!(lowered.contains("let rows: rsscript_runtime::RssStream<rsscript_runtime::Row> ="));
    assert!(lowered.matches("rsscript_runtime::stream_next(&").count() >= 2);
}

#[test]
fn await_for_stream_lowers_to_stream_next_loop() {
    let source = r#"
features: async, local

fn run() -> Result<Unit, ChannelError> {
    let mut channel: Channel<Int> = Channel.bounded(capacity: 1)?
    local receiver = Channel.receiver(channel: mut channel)?
    let stream: Stream<Int> = Receiver.into_stream(receiver: take receiver)
    await for item in stream {
        Log.write(message: read "stream item")
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("await-for-stream.rss", source)
        .expect("await for stream should lower");
    assert!(
        lowered.contains("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();"),
        "await for in a sync function should create an executor, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::stream_next(&stream)")
            && lowered.contains(".run_pending(&mut __rsscript_await_for_"),
        "await for should poll Stream.next through the executor, got:\n{lowered}"
    );
    assert!(
        lowered.contains("Some(item) =>") && lowered.contains("None => break"),
        "await for should match Some(item)/None, got:\n{lowered}"
    );
}

#[test]
fn async_fn_with_await_for_stream_lowers_to_pending_boundary() {
    let source = r#"
features: async, local

async fn run(stream: read Stream<Int>) -> Result<Unit, ChannelError> {
    await for item in stream {
        Log.write(message: read "stream item")
    }
    return Ok(Unit)
}
"#;
    let lowered = lower_source_to_rust("async-await-for-stream.rss", source)
        .expect("async await for stream should lower");
    assert!(
        lowered.contains("fn run(stream: &rsscript_runtime::RssStream<i64>) -> impl rsscript_runtime::Pending<Result<(), rsscript_runtime::ChannelError>>"),
        "async await-for function should return a Pending, got:\n{lowered}"
    );
    assert!(
        lowered.contains("rsscript_runtime::pending_defer(move ||")
            && lowered.contains("rsscript_runtime::pending_try(")
            && lowered.contains("rsscript_runtime::stream_next(&stream)")
            && lowered.contains("run_pending(&mut __rsscript_await_for_"),
        "await for should lower through an explicit async boundary, got:\n{lowered}"
    );
}
