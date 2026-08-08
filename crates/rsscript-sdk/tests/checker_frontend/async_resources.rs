//! Core structured-concurrency and resource-suspension invariants.

use super::*;

#[test]
fn resource_cannot_remain_live_across_await() {
    let source = r#"
resource File
struct IOError
fn File.open(path: read Path) -> Result<File, IOError>
async fn pause() -> Result<Unit, IOError> { return Ok(Unit) }

async fn bad(path: read Path) -> Result<Unit, IOError> {
    with File.open(path: read path)? as file {
        await pause()?
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("resource-await.rss", source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0031"
            && diagnostic
                .summary
                .contains("resource `file` cannot live across `await`")
    }));
}

#[test]
fn task_group_requires_every_named_child_to_be_consumed_once() {
    let unawaited = r#"
async fn child() -> Int { return 1 }
async fn main() -> Unit {
    task_group { async let result = child() }
    return Unit
}
"#;
    let diagnostics = analyze_source("unawaited-child.rss", unawaited);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "unawaited async let"
    }));

    let duplicate = r#"
async fn child() -> Int { return 1 }
async fn main() -> Unit {
    task_group {
        async let result = child()
        await result
        await result
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("duplicate-child-await.rss", duplicate);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "async let awaited more than once"
    }));
}

#[test]
fn task_handle_cannot_escape_its_group() {
    let source = r#"
async fn child() -> Int { return 1 }
async fn main() -> Int {
    task_group {
        async let result = child()
        await result
    }
    return await result
}
"#;
    let diagnostics = analyze_source("escaped-task-handle.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0030")
    );
}

#[test]
fn cancellation_token_requires_a_lexical_task_group_owner() {
    let source = r#"
async fn main() -> Unit {
    let token = Task.cancellation_token()
    return Unit
}
"#;
    let interface = r#"
resource CancellationToken
pub fn Task.cancellation_token() -> fresh CancellationToken
"#;
    let diagnostics = analyze_source_with_interfaces(
        "unowned-cancellation.rss",
        source,
        &[("task.rssi", interface)],
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0412")
    );
}

#[test]
fn retained_local_cannot_escape_through_an_async_call() {
    let source = r#"
struct Payload { value: Int }
async fn main() -> Unit {
    local payload = Payload(value: 1)
    await send(value: read payload)
    return Unit
}
"#;
    let interface = "pub async fn send(value: read Payload) -> Unit retains(value)\n";
    let diagnostics =
        analyze_source_with_interfaces("retained-async.rss", source, &[("host.rssi", interface)]);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "RS0501" || diagnostic.code == "RS0031" })
    );
}
