//! Spec §3 — features: / capability gates (native, unsafe, async)
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn bundled_core_interfaces_are_available_to_checker() {
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/test/assert.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/log/log.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/cache/cache.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/collections/buffer.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/collections/list.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/os/os.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/process/process.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/counter/counter.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/config/rules.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/interpreter/interpreter.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "stdlib/weak/weak.rssi")
    );

    let source = r#"
fn check_label(actual: read String, expected: read String) -> Unit {
    Assert.equal(left: read actual, right: read expected)
    Log.write(message: read actual)
}
"#;

    assert_eq!(
        analyze_source_with_core("assert-use.rss", source),
        Vec::new()
    );
}

#[test]
fn bundled_core_interfaces_report_call_contract_errors() {
    let source = r#"
fn check_label(actual: read String, expected: read String) -> Unit {
    Assert.equal(value: read actual, right: read expected)
}
"#;
    let codes = analyze_source_with_core("assert-use.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0203".to_string()));
    assert!(codes.contains(&"RS0204".to_string()));
}

#[test]
fn bundled_interpreter_function_object_new_does_not_retain_closure() {
    let source = r#"
features: local

fn build() -> Unit {
    local env = Environment.root()
    let function = FunctionObject.new(closure: read env)
    return Unit
}
"#;

    assert_eq!(
        analyze_source_with_core("interpreter-weak.rss", source),
        Vec::new()
    );
}

#[test]
fn checker_can_disable_bundled_core_interfaces() {
    let source = r#"
fn log(value: read String) -> Unit {
    Log.write(message: read value)
    return Unit
}
"#;
    assert!(
        !analyze_source("with-core.rss", source)
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0206"),
        "default analysis should include bundled core interfaces"
    );

    let diagnostics = analyze_source_without_core("without-core.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0206"
                && diagnostic.summary.contains("Log.write")),
        "{diagnostics:?}"
    );
}

#[test]
fn calling_unsafe_function_requires_features_unsafe() {
    let interface = r#"
features: unsafe
pub fn Crypto.raw_copy(dst: mut Buffer, src: read Buffer) -> Unit
    effects(unsafe)
"#;
    let source = r#"
fn looks_safe(dst: mut Buffer, src: read Buffer) -> Unit {
    Crypto.raw_copy(dst: mut dst, src: read src)
    return Unit
}
"#;
    let codes = analyze_source_with_interfaces("caller.rss", source, &[("crypto.rssi", interface)])
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0101".to_string()),
        "calling an unsafe function from a file without `features: unsafe` should be rejected, got {codes:?}"
    );
}

#[test]
fn calling_unsafe_function_is_allowed_under_features_unsafe() {
    let interface = r#"
features: unsafe
pub fn Crypto.raw_copy(dst: mut Buffer, src: read Buffer) -> Unit
    effects(unsafe)
"#;
    let source = r#"
features: unsafe

fn wrapper(dst: mut Buffer, src: read Buffer) -> Unit {
    Crypto.raw_copy(dst: mut dst, src: read src)
    return Unit
}
"#;
    assert_eq!(
        analyze_source_with_interfaces("caller.rss", source, &[("crypto.rssi", interface)]),
        Vec::new()
    );
}

#[test]
fn checker_accepts_executable_async_function_body() {
    let source = r#"
features: async

async fn tick() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("async-body.rss", source);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn checker_accepts_await_inside_async_function() {
    let source = r#"
features: async

async fn TestTimer.sleep(ms: Int) -> Unit

async fn receive() -> Unit {
    await TestTimer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-body.rss", source);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn checker_rejects_await_outside_async_function() {
    let source = r#"
features: async

async fn TestTimer.sleep(ms: Int) -> Unit

fn receive() -> Unit {
    await TestTimer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-outside-async.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0029" && diagnostic.label == "await outside async fn"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_await_of_non_async_expression() {
    let source = r#"
features: async

fn sync_sleep(ms: Int) -> Unit

async fn receive() -> Unit {
    await sync_sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-non-async.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0030" && diagnostic.label == "await non-async expression"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_await_inside_non_async_closure() {
    let source = r#"
features: async

async fn Timer.sleep(ms: Int) -> Unit
fn run(callback: noescape Fn()) -> Unit

async fn receive() -> Unit {
    run(callback: || {
        await Timer.sleep(ms: 1)
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("await-closure.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0029" && diagnostic.label == "await outside async fn"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_resource_live_across_await() {
    let source = r#"
features: async, local

resource File
struct IOError

fn File.open(path: read Path) -> Result<File, IOError>
async fn Timer.sleep(ms: Int) -> Result<Unit, IOError>

async fn bad(path: read Path) -> Result<Unit, IOError> {
    with File.open(path: read path)? as file {
        await Timer.sleep(ms: 1)?
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("await-resource.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("resource `file` cannot live across `await`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_dead_local_before_await() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Timer.sleep(ms: Int) -> Unit

async fn ok() -> Unit {
    local image = Image(size: 1)
    await Timer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-dead-local.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0031"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_local_used_after_await() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Timer.sleep(ms: Int) -> Unit
fn Image.inspect(image: read Image) -> Unit

async fn bad() -> Unit {
    local image = Image(size: 1)
    await Timer.sleep(ms: 1)
    Image.inspect(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-live-local.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("local value `image` cannot live across `await`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_local_passed_into_awaited_call() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Image.upload(image: read Image) -> Unit

async fn bad() -> Unit {
    local image = Image(size: 1)
    await Image.upload(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-local-arg.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("local value `image` cannot live across `await`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_local_taken_into_awaited_call() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Image.upload(image: take Image) -> Unit

async fn ok() -> Unit {
    local image = Image(size: 1)
    await Image.upload(image: take image)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-take-local-arg.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0031"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_spawn_as_unsupported_until_async_lowering_exists() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported spawn expression")
    );
}

#[test]
fn checker_rejects_async_call_without_await() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("async-call-direct.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0022" && diagnostic.label == "async call must be awaited"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_async_interface_calls_on_async_feature() {
    let interface = r#"
async fn Http.get(url: read Url) -> Result<fresh Bytes, NetworkError>
"#;
    let source = r#"
fn receive(url: read Url) -> Unit {
    let bytes = Http.get(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source_with_interfaces(
        "async-interface-call.rss",
        source,
        &[("net.rssi", interface)],
    );

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("async")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0022"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_await_on_async_feature() {
    let source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = await fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-feature.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("await")),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_spawn_on_async_feature() {
    let source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-feature.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("spawn")),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_spawn_capturing_local_value() {
    let source = r#"
features: async, local

struct Image

fn work(image: read Image) -> Unit

fn schedule(path: read Path) -> Unit {
    local image = Image()
    let task = spawn work(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-local.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0501"
                && diagnostic.label == "local captured by spawn"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_native_bodies_as_unsupported_until_native_binding_exists() {
    let source = r#"
features: native

native fn Host.emit(message: read String) -> Unit
    effects(native)
{
    return Unit
}
"#;
    let diagnostics = analyze_source("native-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported native function body")
    );
}

#[test]
fn checker_rejects_unknown_file_features() {
    let source = r#"
features: local, locall

fn main() -> Unit {
    return Unit
}
"#;
    let program = parse_source("features.rss", source);

    assert_eq!(program.features.len(), 1);
    assert_eq!(program.unknown_features.len(), 1);
    assert_eq!(program.unknown_features[0].name, "locall");
    let diagnostics = analyze_source("features.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0016")
    );
}

#[test]
fn checker_rejects_duplicate_file_features() {
    let source = r#"
features: local, local

fn main() -> Unit {
    return Unit
}
"#;
    let program = parse_source("features.rss", source);

    assert_eq!(program.features.len(), 2);
    assert_eq!(program.duplicate_features.len(), 1);
    assert_eq!(program.duplicate_features[0].name, "local");
    let diagnostics = analyze_source("features.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0017")
    );
}

#[test]
fn checker_keeps_file_features_scoped_across_source_sets() {
    let diagnostics = analyze_sources_with_interfaces(
        &[
            (
                "capability.rss",
                r#"
features: local

fn helper() -> Unit {
    local value = String.new()
    return Unit
}
"#,
            ),
            (
                "plain.rss",
                r#"
fn bad() -> Unit {
    local value = String.new()
    return Unit
}
"#,
            ),
        ],
        &[],
    );

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0101"
            && diagnostic.span.file == "plain.rss"
            && diagnostic.summary.contains("features: local")
    }));
    assert!(!diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0101" && diagnostic.span.file == "capability.rss"
    }));
}

#[test]
fn task_group_with_async_let_passes_checker() {
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
        let p = await profile?
    }
    return Ok("done")
}
"#;
    let diagnostics = analyze_source("task-group-async-let.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "task_group with async let should pass: {errors:?}"
    );
}

#[test]
fn async_let_outside_task_group_is_rejected() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    async let result = fetch(id: read id)
    return await result
}
"#;
    let diagnostics = analyze_source("async-let-outside.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.causes.iter().any(|c| c.contains("async let"))),
        "async let outside task_group should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_rejects_unawaited_async_let() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    task_group {
        async let result = fetch(id: read id)
    }
    return 0
}
"#;
    let diagnostics = analyze_source("task-group-unawaited.rss", source);
    assert!(
        diagnostics.iter().any(|d| {
            d.code == "RS0015"
                && d.label == "unawaited async let"
                && d.causes
                    .iter()
                    .any(|cause| cause.contains("must be consumed by `await`"))
        }),
        "unawaited async let should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_rejects_forward_await_of_async_let_handle() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

fn run(id: read Int) -> Int {
    task_group {
        await result
        async let result = fetch(id: read id)
    }
    return 0
}
"#;
    let diagnostics = analyze_source("task-group-forward-await.rss", source);
    assert!(
        diagnostics.iter().any(|d| {
            d.code == "RS0015"
                && d.label == "async let await before declaration"
                && d.causes
                    .iter()
                    .any(|cause| cause.contains("after the matching `async let`"))
        }),
        "forward await of task_group handle should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_rejects_duplicate_await_of_async_let_handle() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

fn run(id: read Int) -> Int {
    task_group {
        async let result = fetch(id: read id)
        await result
        await result
    }
    return 0
}
"#;
    let diagnostics = analyze_source("task-group-duplicate-await.rss", source);
    assert!(
        diagnostics.iter().any(|d| {
            d.code == "RS0015"
                && d.label == "async let awaited more than once"
                && d.causes
                    .iter()
                    .any(|cause| cause.contains("can be consumed by `await` only once"))
        }),
        "duplicate await of task_group handle should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_allows_discarded_background_async_let() {
    let source = r#"
features: async

fn run() -> Result<Unit, TimerError> {
    task_group {
        async let _ = Timer.sleep(ms: 1)
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("task-group-background.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "discarded async let should be a scoped background task: {errors:?}"
    );
}

#[test]
fn task_group_async_let_handle_not_visible_after_group() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    task_group {
        async let result = fetch(id: read id)
        await result
    }
    return await result
}
"#;
    let diagnostics = analyze_source("task-group-handle-escape.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0030"),
        "awaiting a consumed task_group handle outside the group should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_rejects_nested_async_let() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    task_group {
        if true {
            async let result = fetch(id: read id)
            await result
        }
    }
    return 0
}
"#;
    let diagnostics = analyze_source("task-group-nested-async-let.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.label == "nested async let"),
        "nested async let should be rejected until lowering supports it: {diagnostics:?}"
    );
}

#[test]
fn task_group_rejects_nested_await_of_async_let_handle() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    task_group {
        async let result = fetch(id: read id)
        if true {
            await result
        }
    }
    return 0
}
"#;
    let diagnostics = analyze_source("task-group-nested-await.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.label == "nested async let await"),
        "nested await of task_group handle should be rejected until lowering supports it: {diagnostics:?}"
    );
}

#[test]
fn task_group_requires_async_feature() {
    let source = r#"
fn run() -> Int {
    task_group {
        let x = 1
    }
    return x
}
"#;
    let diagnostics = analyze_source("task-group-no-feature.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0101"),
        "task_group without features: async should be rejected: {diagnostics:?}"
    );
}

#[test]
fn async_fn_with_task_group_passes_checker() {
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
    let diagnostics = analyze_source("async-fn-task-group.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "async fn with task_group should pass: {errors:?}"
    );
}

#[test]
fn select_with_await_arms_passes_checker() {
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
    let diagnostics = analyze_source("select-pass.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "select should pass: {errors:?}");
}

#[test]
fn select_arm_requires_await_operation() {
    let source = r#"
features: async

fn run() -> Result<Unit, TimerError> {
    select {
        _ = Timer.sleep(ms: 1) => {
            Log.write(message: read "bad")
        }
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("select-no-await.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.label == "malformed select arm"),
        "select arm without await should be rejected: {diagnostics:?}"
    );
}

#[test]
fn async_fn_with_select_passes_checker() {
    let source = r#"
features: async

async fn run() -> Result<Unit, TimerError> {
    select {
        _ = await Timer.sleep(ms: 1)? => {
            Log.write(message: read "bad")
        }
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("select-in-async-fn.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "select inside async fn should pass: {errors:?}"
    );
}

#[test]
fn async_fn_allows_await_inside_select_arm_body() {
    let source = r#"
features: async

async fn run() -> Result<Unit, TimerError> {
    select {
        _ = await Timer.sleep(ms: 1)? => {
            await Timer.sleep(ms: 1)?
        }
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("select-body-await.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "await nested in select arm body should pass through an async boundary: {errors:?}"
    );
}

#[test]
fn stream_next_from_channel_receiver_passes_checker() {
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
    let diagnostics = analyze_source("stream-next.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "stream next should pass: {errors:?}");
}

#[test]
fn stream_sources_and_collect_list_pass_checker() {
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
        Log.write(message: read "row")
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("stream-sources.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "stream source APIs should pass: {errors:?}"
    );
}

#[test]
fn await_for_stream_passes_checker() {
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
    let diagnostics = analyze_source("await-for-stream.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "await for stream should pass: {errors:?}"
    );
}

#[test]
fn async_fn_with_await_for_stream_passes_checker() {
    let source = r#"
features: async, local

async fn run(stream: read Stream<Int>) -> Result<Unit, ChannelError> {
    await for item in stream {
        Log.write(message: read "stream item")
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("async-await-for-stream.rss", source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "await for stream inside async fn should pass: {errors:?}"
    );
}
