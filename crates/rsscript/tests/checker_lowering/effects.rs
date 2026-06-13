//! Spec §10 — data-effect / borrow lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_maps_take_consume_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn consume_list(list: take List<Int>) -> Unit {
    List.consume(list: take list)
}

fn consume_buffer(buffer: take Buffer) -> Unit {
    Buffer.consume(buffer: take buffer)
}
"#;
    let rust = lower_source_to_rust("consume.rss", source).expect("source should lower");

    assert!(rust.contains("fn consume_list(list: Vec<i64>)"));
    assert!(rust.contains("rsscript_runtime::list_consume(list);"));
    assert!(rust.contains("fn consume_buffer(buffer: Vec<u8>)"));
    assert!(rust.contains("rsscript_runtime::buffer_consume(buffer);"));
}

#[test]
fn rust_lowering_maps_file_read_into_and_buffer_reuse_to_runtime_hooks() {
    let source = r#"
features: local

fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    local buffer = Buffer.new(size: 8192)
    with File.open_read(path: read input)? as reader {
        with File.open_write(path: read output)? as writer {
            while File.read_into(file: mut reader, buffer: mut buffer)? {
                File.write_buffer(file: mut writer, buffer: read buffer)?
                Buffer.clear(buffer: mut buffer)
            }
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file-buffer.rss", source).expect("source should lower");

    assert!(rust.contains("let mut buffer = rsscript_runtime::buffer_new(8192i64);"));
    assert!(rust.contains("while rsscript_runtime::file_read_into(&mut reader, &mut buffer)? {"));
    assert!(rust.contains("rsscript_runtime::file_write_buffer(&mut writer, &(buffer))?;"));
    assert!(rust.contains("rsscript_runtime::buffer_clear(&mut buffer);"));
}

#[test]
fn rust_lowering_allows_read_effect_on_receiver_call_result() {
    let source = r#"
fn accept(value: read String) -> Unit {
    Log.write(message: read value)
}

fn main() -> Unit {
    let value: JsonValue = {"ok": true}
    accept(value: read value.to_string())
    return Unit
}
"#;
    let rust = lower_source_to_rust("read-receiver-call-result.rss", source)
        .expect("read receiver-call result should lower");

    assert!(rust.contains("accept(&rsscript_runtime::json_to_string(&value));"));
}

#[test]
fn rust_lowering_materializes_read_string_when_owned_string_is_expected() {
    let source = r#"
struct Names {
    value: String
}

fn identity(value: read String) -> String {
    return value
}

fn wrap(value: read String) -> fresh Names {
    return Names(value: value)
}
"#;
    let rust = lower_source_to_rust("read-string-owned.rss", source)
        .expect("read String should materialize when owned String is expected");

    assert!(rust.contains("return value.clone();"));
    assert!(rust.contains("value: value.clone()"));
}

#[test]
fn rust_lowering_maps_string_and_bytes_views_to_borrowed_slices() {
    let source = r#"
features: local

fn parse_header(line: read String, bytes: read Bytes, buffer: read Buffer) -> Result<String, Unit> {
    let head = String.view(value: read line, start: 0, len: 12)
    let key = StringView.before(value: read head, delimiter: read ":")
    let bytes_head = Bytes.view(value: read bytes, start: 0, len: 4)
    let buffer_head = Buffer.view(buffer: read buffer, start: 0, len: 4)
    let copied = BytesView.to_bytes(value: read bytes_head)
    let copied_buffer = BufferView.to_bytes(value: read buffer_head)
    let _ = BytesView.len(value: read bytes_head)
    let _ = BufferView.len(value: read buffer_head)
    match key {
        Some(value) => return Ok(StringView.to_string(value: read value))
        None => return Ok(StringView.to_string(value: read head))
    }
}

fn write_views(file: mut File, bytes: read Bytes, buffer: read Buffer) -> Result<Unit, FileError> {
    let bytes_head = Bytes.view(value: read bytes, start: 0, len: 4)
    let buffer_head = Buffer.view(buffer: read buffer, start: 0, len: 4)
    File.write_bytes_view(file: mut file, data: read bytes_head)?
    File.write_buffer_view(file: mut file, buffer: read buffer_head)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("view-lower.rss", source).expect("source should lower");

    assert!(rust.contains("let head = rsscript_runtime::string_view(line, 0i64, 12i64);"));
    assert!(rust.contains(
        "let key = rsscript_runtime::string_view_before(&(head), &(\":\".to_string()));"
    ));
    assert!(rust.contains("let bytes_head = rsscript_runtime::bytes_view(bytes, 0i64, 4i64);"));
    assert!(rust.contains("let buffer_head = rsscript_runtime::buffer_view(buffer, 0i64, 4i64);"));
    assert!(rust.contains("let copied = rsscript_runtime::bytes_view_to_bytes(&(bytes_head));"));
    assert!(
        rust.contains(
            "let copied_buffer = rsscript_runtime::buffer_view_to_bytes(&(buffer_head));"
        )
    );
    assert!(rust.contains("rsscript_runtime::file_write(file, &(bytes_head))?;"));
    assert!(rust.contains("rsscript_runtime::file_write(file, &(buffer_head))?;"));
    assert!(rust.contains("return Ok(rsscript_runtime::string_view_to_string(&(value)));"));
    assert!(!rust.contains("dyn"));
}

#[test]
fn rust_lowering_compares_read_string_params_to_literals() {
    let source = r#"
fn is_unknown(value: read String) -> Bool {
    return value == "unknown"
}
"#;
    let rust = lower_source_to_rust("string-compare.rss", source).expect("source should lower");

    assert!(rust.contains("return value.as_str() == \"unknown\";"));
}

#[test]
fn rust_lowering_emits_machine_readable_source_map() {
    let source = r#"
features: local

struct Session {
    id: Int
}

fn save(session: read Session) -> Unit {
    return Unit
}

pub fn make_session(id: Int) -> Unit {
    local session = Session(id: id)
    save(session: read session)
    let managed = manage session
    return Unit
}
"#;
    let lowered =
        lower_source_to_rust_with_map("session.rss", source).expect("source should lower");
    let kinds: Vec<&str> = lowered
        .source_map
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect();

    assert!(kinds.contains(&"function"));
    assert!(kinds.contains(&"statement"));
    assert!(kinds.contains(&"call"));
    assert!(kinds.contains(&"named_arg"));
    assert!(kinds.contains(&"manage"));
    assert!(kinds.contains(&"ident"));
    assert!(kinds.contains(&"field"));
    assert!(lowered.source_map.iter().any(|entry| {
        entry.source.file == "session.rss"
            && entry.generated.file == "src/lib.rs"
            && entry.generated.line > 0
            && entry.generated.column > 0
    }));
}

#[test]
fn rust_lowering_resolves_receiver_calls_through_capability_object_protocol() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_dynamic(writer: take BufferWriter, message: read String) -> Unit {
    local cap: Capability<Writer> = Capability<Writer>.from(value: take writer)
    mut cap.write(message: read message)
}
"#;
    let rust =
        lower_source_to_rust("capability-receiver-lower.rss", source).expect("source should lower");

    assert!(rust.contains("<CapabilityWriter as Writer>::write(&mut cap, message);"));
}

#[test]
fn rust_lowering_allows_multiline_receiver_chain_after_closure() {
    let source = r#"
fn parse(value: read Option<Int>) -> Int {
    return value.map(|item| {
        return item + 1
    }).unwrap_or(0)
}
"#;
    let rust = lower_source_to_rust("multiline-receiver-chain-after-closure.rss", source)
        .expect("multiline receiver chain after closure should lower");

    assert!(rust.contains("rsscript_runtime::option_unwrap_or(&rsscript_runtime::option_map("));
    assert!(rust.contains("|item: i64| {"));
}

#[test]
fn rust_lowering_allows_receiver_calls_on_field_receivers() {
    let source = r#"
struct User {
    name: String
}

fn main() -> Int {
    let user = User(name: "rss")
    return user.name.len()
}
"#;
    let rust = lower_source_to_rust("field-receiver-call-lower.rss", source)
        .expect("field receiver calls should lower");

    assert!(rust.contains("return rsscript_runtime::string_len(&user.name);"));
}

#[test]
fn rust_lowering_allows_receiver_calls_on_sum_variants() {
    let source = r#"
sum Runtime {
    CoreRuntime
}

fn Runtime.name(runtime: read Runtime) -> String {
    match runtime {
        CoreRuntime => {
            return "core"
        }
    }
}

fn main() -> String {
    let runtime = CoreRuntime
    return runtime.name()
}
"#;
    let rust = lower_source_to_rust("sum-variant-receiver-call-lower.rss", source)
        .expect("sum variant receiver calls should lower");

    assert!(rust.contains("let runtime = Runtime::CoreRuntime;"));
    assert!(rust.contains("return Runtime_name(&runtime);"));
}

#[test]
fn rust_lowering_materializes_constructor_read_fields_as_owned_values() {
    let source = r#"
features: local

struct Inner {
    name: String
}

struct Outer {
    inner: Inner
    label: String
}

fn make_outer(name: read String, label: read String) -> fresh Outer {
    local inner = Inner(name: read name)
    return Outer(inner: take inner, label: read label)
}
"#;
    let rust = lower_source_to_rust("owned-constructor.rss", source).expect("source should lower");

    assert!(rust.contains("#[derive(Debug, Clone)]"));
    assert!(rust.contains("Inner { name: name.clone() }"));
    assert!(rust.contains("Outer { inner: inner, label: label.clone() }"));
    assert!(!rust.contains("name: &name"));
    assert!(!rust.contains("inner: &inner"));
    assert!(!rust.contains("inner: inner.clone()"));
}

#[test]
fn rust_lowering_maps_read_and_mut_effects_to_rust_borrows() {
    let source = r#"
features: local

struct Meter {
    value: Int
}

fn read_value(counter: read Meter) -> Int {
    return counter.value
}

fn touch(counter: mut Meter) -> Unit {
    return Unit
}

pub fn run() -> Int {
    local counter = Meter(value: 1)
    touch(counter: mut counter)
    return read_value(counter: read counter)
}
"#;
    let rust = lower_source_to_rust("effects.rss", source).expect("source should lower");

    assert!(rust.contains("fn read_value(counter: &Meter) -> i64"));
    assert!(rust.contains("fn touch(counter: &mut Meter)"));
    assert!(rust.contains("touch(&mut counter);"));
    assert!(rust.contains("return read_value(&(counter));"));
}

#[test]
fn checker_reports_malformed_effect_declarations_as_unsupported() {
    let source = r#"
fn empty_effect_slot(value: read String) -> Unit
    effects(no_panic,, native)
{
    return Unit
}

fn empty_retains(value: read String) -> Unit
    effects(retains())
{
    return Unit
}

fn unknown_effect_call(value: read String) -> Unit
    effects(custom(value))
{
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-effects.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed effect declaration"
        })
        .count();

    assert_eq!(malformed_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_rejects_unknown_effect_names() {
    let source = r#"
fn typo(value: read String) -> Unit
    effects(retian)
{
    return Unit
}
"#;
    let diagnostics = analyze_source("unknown-effect.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0004"
            && diagnostic.severity.is_error()
            && diagnostic.summary.contains("retian")
    }));
}

#[test]
fn checker_rejects_user_authored_suspends_effect() {
    let source = r#"
fn wait() -> Unit
    effects(suspends)
{
    return Unit
}
"#;
    let diagnostics = analyze_source("suspends-effect.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0012"
                && diagnostic.label == "removed runtime effect"
                && diagnostic
                    .fixes
                    .iter()
                    .any(|fix| fix.title.contains("compiler-normalized review metadata"))
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_self_parameter_outside_method_receiver_position() {
    let source = r#"
fn bad(self: read String) -> Unit {
    return Unit
}

fn Logger.write(message: read String, self: mut Logger) -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("self-param.rss", source);
    let invalid_self_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0028")
        .count();
    assert_eq!(invalid_self_count, 2, "{diagnostics:#?}");
}

#[test]
fn checker_requires_protocol_call_receiver_to_satisfy_protocol() {
    let source = r#"
protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn write_concrete(
    writer: mut BufferWriter,
    message: read String,
) -> Unit {
    Writer.write(self: mut writer, message: read message)
}

fn write_generic<W>(
    writer: mut W,
    message: read String,
) -> Unit {
    Writer.write(self: mut writer, message: read message)
}
"#;
    let diagnostics = analyze_source("protocol-call-satisfaction.rss", source);
    let protocol_errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0032")
        .collect::<Vec<_>>();
    assert_eq!(protocol_errors.len(), 2, "{diagnostics:#?}");
    assert!(protocol_errors.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("receiver type `BufferWriter` does not satisfy protocol `Writer`")
    }));
    assert!(protocol_errors.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("receiver type `W` does not satisfy protocol `Writer`")
    }));
}
