mod common;

use std::fs;
use std::process::Command;

use rsscript::{
    EvalError, eval_source_main, eval_source_main_with_args, lower_source_to_rust_package,
    write_generated_rust_package,
};

#[test]
fn eval_runs_pure_arithmetic_main() {
    let source = r#"
fn main() -> Int {
    let x = 2
    let y = 3
    return x + y * 4
}
"#;

    let output = eval_source_main("eval-arithmetic.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "14");
}

#[test]
fn eval_runs_user_function_and_assignment() {
    let source = r#"
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main() -> Int {
    let mut total = add(a: 1, b: 2)
    total = total + 4
    return total
}
"#;

    let output = eval_source_main("eval-function.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "7");
}

#[test]
fn eval_runs_nested_pattern_match() {
    let source = r#"
fn main() -> String {
    let value = Some(Some("rss"))
    match value {
        Some(Some(text)) => {
            return read text
        }
        Some(None) => {
            return "inner none"
        }
        None => {
            return "none"
        }
    }
}
"#;

    let output = eval_source_main("eval-nested-match.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "rss");
}

#[test]
fn eval_reports_unsupported_runtime_intrinsic() {
    let source = r#"
fn main() -> Int {
    return Random.int(min: 0, max: 10)
}
"#;

    let error = eval_source_main("eval-unsupported.rss", source)
        .expect_err("unsupported intrinsic should fail");

    assert!(matches!(error, EvalError::Runtime(message) if message.contains("Random.int")));
}

#[test]
fn parity_async_await_runs_synchronously() {
    let source = r#"
features: async, native

async fn add_after_sleep(value: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(value + 1)
}

async fn main() -> Result<Unit, TimerError> {
    let value = await add_after_sleep(value: 4)?
    Log.write(message: read String.from_int(value: value))

    let deadline = Deadline.after_ms(ms: 1)
    await Timer.sleep_until(deadline: read deadline)?

    let source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    await Timer.sleep_cancellable(ms: 1, token: read token)?
    Log.write(message: read "async-done")
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend(
        "parity-async-await.rss",
        "rsscript_parity_async_await",
        source,
    );
}

#[test]
fn parity_async_file_intrinsics() {
    let source = r#"
features: async, native, local

async fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "async-file.txt")
    await File.write_string_async(path: read path, text: read "hello async")?
    let text = await File.read_all_string_async(path: read path)?
    Log.write(message: read text)

    await File.write_async(path: read path, data: read Bytes.from_string(value: read "bytes"))?
    let bytes = await File.read_all_async(path: read path)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend(
        "parity-async-file.rss",
        "rsscript_parity_async_file",
        source,
    );
}

#[test]
fn parity_async_process_intrinsics() {
    let source = r#"
features: async, native, local

async fn main() -> Result<Unit, String> {
    let output = await Process.run_async(command: read "printf", args: read ["ok"])?
    Log.write(message: read String.from_int(value: output.status))

    let stdout = await Process.run_stdout_async(command: read "printf", args: read ["stdout"])?
    Log.write(message: read stdout)

    let timeout_output = await Process.run_timeout_async(command: read "printf", args: read ["timeout"], timeout_ms: 1000)?
    Log.write(message: read String.from_int(value: timeout_output.status))

    let timeout_stdout = await Process.run_stdout_timeout_async(command: read "printf", args: read ["timeout-stdout"], timeout_ms: 1000)?
    Log.write(message: read timeout_stdout)

    let many = await Process.run_many_stdout_async(command: read "printf", args: read [], appended_args: read ["a", "b"], jobs: 2)?
    Log.write(message: read List.join<String>(list: read many, separator: read "|"))

    let many_timeout = await Process.run_many_stdout_timeout_async(command: read "printf", args: read [], appended_args: read ["c", "d"], jobs: 2, timeout_ms: 1000)?
    Log.write(message: read List.join<String>(list: read many_timeout, separator: read "|"))

    let request = ProcessRequest(
        command: "cat",
        args: List<String>.new(),
        cwd: None,
        stdin: Some("request-stdin"),
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let request_output = await Process.run_request_async(request: read request)?
    Log.write(message: read request_output.stdout)

    let cancellable_request = ProcessRequest(
        command: "printf",
        args: ["cancellable"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    let cancellable_output = await Process.run_request_cancellable_async(request: read cancellable_request, token: read token)?
    Log.write(message: read cancellable_output.stdout)
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend(
        "parity-async-process.rss",
        "rsscript_parity_async_process",
        source,
    );
}

#[test]
fn eval_matches_lowered_rust_for_pure_core_example() {
    let source_path = "examples/scripts/core/interpreter_pure_parity.rss";
    let source = fs::read_to_string(source_path).expect("parity fixture should be readable");
    let eval = eval_source_main(source_path, &source).expect("eval should succeed");
    assert_eq!(eval.value, "Unit");

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package =
        lower_source_to_rust_package(source_path, &source, "rsscript_eval_parity", &runtime_path)
            .expect("parity fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, eval.stdout);
    assert_eq!(stderr, eval.stderr);
}

#[test]
fn eval_string_len_matches_lowered_rust_for_utf8_bytes() {
    let source = r#"
fn main() -> Unit {
    let len = String.len(value: read "é")
    Log.write(message: read String.from_int(value: len))
}
"#;
    let eval = eval_source_main("eval-string-len-utf8.rss", source).expect("eval should succeed");
    assert_eq!(eval.value, "Unit");
    assert_eq!(eval.stdout, "2\n");

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package = lower_source_to_rust_package(
        "eval-string-len-utf8.rss",
        source,
        "rsscript_eval_string_len_utf8",
        &runtime_path,
    )
    .expect("utf8 length fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-string-len-utf8");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "2\n");
    assert_eq!(stderr, "");
}

#[test]
fn eval_fails_closed_where_lowered_rust_crosses_declared_host_boundary() {
    let source_path = "examples/scripts/core/interpreter_host_boundary.rss";
    let source = fs::read_to_string(source_path).expect("host boundary fixture should be readable");
    let eval = eval_source_main(source_path, &source).expect_err("eval should fail closed");
    assert!(
        matches!(&eval, EvalError::Runtime(message) if message.contains("Env.set_current_dir")),
        "{eval:?}"
    );

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package = lower_source_to_rust_package(
        source_path,
        &source,
        "rsscript_eval_host_boundary",
        &runtime_path,
    )
    .expect("host boundary fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-host-boundary");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "host-ok\n");
    assert_eq!(stderr, "");
}

/// Differential parity harness: run the same source through the interpreter and
/// through the lowered-Rust backend, then assert their observable output agrees.
/// This is the mechanism (not docs) that keeps the interpreter from diverging
/// from the authoritative backend — one fixture per supported construct.
// parity: function:async function:sync
// parity: hir_stmt:Assign hir_stmt:Break hir_stmt:Continue hir_stmt:Expr hir_stmt:For
// parity: hir_stmt:If hir_stmt:Let hir_stmt:Loop hir_stmt:Match hir_stmt:Return hir_stmt:With
// parity: hir_expr:ArrayLiteral hir_expr:Await hir_expr:Binary hir_expr:Call hir_expr:Effect hir_expr:Field
// parity: hir_expr:Closure hir_expr:Ident hir_expr:Index hir_expr:Manage hir_expr:MapLiteral hir_expr:Match
// parity: hir_expr:Number hir_expr:ObjectLiteral hir_expr:String hir_expr:Try
// parity: value:Bool value:Bytes value:Int value:Json value:List
// parity: value:Char value:Closure value:Map value:String value:Struct value:Unit value:Variant
// parity: runtime:Args.all runtime:Args.count runtime:Args.get runtime:Args.get_or_default
// parity: runtime:Assert.equal runtime:Assert.equal_bool runtime:Assert.equal_int
// parity: runtime:Char.compare runtime:Char.from_code runtime:Char.is_alpha
// parity: runtime:Char.is_alphanumeric runtime:Char.is_digit runtime:Char.is_whitespace
// parity: runtime:Char.to_code runtime:Char.to_string
// parity: runtime:Deque.clear runtime:Deque.is_empty runtime:Deque.len runtime:Deque.new
// parity: runtime:Deque.pop_back runtime:Deque.pop_front runtime:Deque.push_back
// parity: runtime:Deque.push_front runtime:Deque.to_list
// parity: runtime:DbConnection.open runtime:DbConnection.query runtime:DbConnection.try_open
// parity: runtime:Directory.create runtime:Directory.create_all runtime:Directory.create_dir_all
// parity: runtime:Directory.exists runtime:Directory.is_dir runtime:Directory.is_file
// parity: runtime:Directory.list_files runtime:Directory.list_paths runtime:Directory.metadata
// parity: runtime:Directory.copy_file runtime:Directory.rename runtime:Directory.remove_file
// parity: runtime:Directory.remove_dir_all runtime:Directory.read_string runtime:Directory.write_string
// parity: runtime:Duration.add runtime:Duration.as_ms runtime:Duration.as_seconds
// parity: runtime:Duration.ms runtime:Duration.seconds
// parity: runtime:Environment.bind_function runtime:Environment.child
// parity: runtime:Environment.has_function runtime:Environment.has_parent runtime:Environment.root
// parity: runtime:Diff.unified runtime:Patch.apply_text
// parity: runtime:Bytes.concat runtime:Bytes.consume runtime:Bytes.from_string runtime:Bytes.is_empty
// parity: runtime:Bytes.from_buffer runtime:Bytes.len runtime:Bytes.slice runtime:Bytes.view
// parity: runtime:BytesView.is_empty runtime:BytesView.len runtime:BytesView.slice
// parity: runtime:BytesView.starts_with runtime:BytesView.to_bytes runtime:Int.to_string
// parity: runtime:Buffer.clear runtime:Buffer.consume runtime:Buffer.is_empty runtime:Buffer.len
// parity: runtime:Buffer.new runtime:Buffer.view runtime:BufferView.is_empty
// parity: runtime:BufferView.len runtime:BufferView.slice runtime:BufferView.to_bytes
// parity: runtime:Cache.get runtime:Cache.insert runtime:Cache.lookup runtime:Cache.new
// parity: runtime:Channel.bounded runtime:Channel.receiver runtime:Channel.sender
// parity: runtime:ChannelError.message
// parity: runtime:CancellationSource.cancel runtime:CancellationSource.new
// parity: runtime:CancellationSource.token runtime:CancellationToken.is_cancelled
// parity: runtime:Clock.now runtime:Clock.system_unix_ms
// parity: runtime:Config.load runtime:Config.name runtime:Config.new runtime:Config.rule_count
// parity: runtime:ConfigStore.name runtime:ConfigStore.new runtime:ConfigStore.replace
// parity: runtime:Counter.add runtime:Counter.new runtime:Counter.value
// parity: runtime:Csv.open_read runtime:Csv.parse_row runtime:Csv.read_into
// parity: runtime:Deadline.after runtime:Deadline.after_ms
// parity: runtime:Deadline.is_expired runtime:Deadline.remaining_ms
// parity: runtime:Json.array runtime:Json.array_bools runtime:Json.array_contains_prefix
// parity: runtime:Json.array_contains_string runtime:Json.array_contains_substring
// parity: runtime:Json.array_get runtime:Json.array_ints runtime:Json.array_len runtime:Json.array_strings
// parity: runtime:Json.as_bool runtime:Json.as_int runtime:Json.as_string runtime:Json.field
// parity: runtime:Json.at runtime:Json.at_bool runtime:Json.at_bool_or runtime:Json.at_int
// parity: runtime:Json.at_int_or runtime:Json.at_optional runtime:Json.at_optional_bool
// parity: runtime:Json.at_optional_int runtime:Json.at_optional_string runtime:Json.at_or
// parity: runtime:Json.at_string runtime:Json.at_string_or runtime:Json.at_to_string
// parity: runtime:Json.at_to_string_or
// parity: runtime:Json.bool_at runtime:Json.bool_at_or
// parity: runtime:Json.bool_field runtime:Json.clone
// parity: runtime:Json.field_bool runtime:Json.field_int runtime:Json.field_optional
// parity: runtime:Json.field_optional_bool runtime:Json.field_optional_int
// parity: runtime:Json.field_optional_string runtime:Json.field_string runtime:Json.is_array
// parity: runtime:Json.is_null runtime:Json.is_object runtime:Json.json_bool_at_or
// parity: runtime:Json.json_int_at_or runtime:Json.json_parse runtime:Json.json_string_at_or
// parity: runtime:Json.int_at runtime:Json.int_at_or runtime:Json.int_field
// parity: runtime:Json.kind runtime:Json.object runtime:Json.object_keys
// parity: runtime:Json.object_len runtime:Json.parse runtime:Json.parse_file
// parity: runtime:Json.quote_string runtime:Json.raw_field
// parity: runtime:Json.string_array runtime:Json.string_at runtime:Json.string_at_or
// parity: runtime:Json.strings runtime:Json.string_field runtime:Json.to_string
// parity: runtime:Json.to_string_at runtime:Json.to_string_at_or
// parity: runtime:Json.value runtime:Json.value_at runtime:Json.values runtime:JsonError.message
// parity: runtime:Instant.elapsed
// parity: runtime:Hash.sha256_bytes runtime:Hash.sha256_file runtime:Hash.sha256_string
// parity: runtime:Http.get runtime:Http.get_async runtime:Http.get_retry_async
// parity: runtime:Http.get_timeout_async runtime:Http.post_form runtime:Http.post_form_async
// parity: runtime:Http.post_json runtime:Http.post_json_async runtime:Http.post_json_bearer_retry_async
// parity: runtime:Http.post_json_retry_async runtime:Http.post_json_timeout_async
// parity: runtime:Http.send_async runtime:HttpError.message
// parity: runtime:HttpRequest.json runtime:HttpRequest.with_header
// parity: runtime:HttpRequest.with_retry runtime:HttpRequest.with_timeout
// parity: runtime:Image.inspect runtime:Image.load runtime:Image.normalize runtime:Image.resize
// parity: runtime:Image.save runtime:Image.sharpen runtime:ImageCache.len runtime:ImageCache.new
// parity: runtime:ImageCache.store
// parity: runtime:List.all runtime:List.any runtime:List.append runtime:List.clear
// parity: runtime:List.contains runtime:List.contains_value runtime:List.count_where
// parity: runtime:List.consume runtime:List.filter runtime:List.find runtime:List.first
// parity: runtime:List.flat_map runtime:List.get runtime:List.is_empty runtime:List.join
// parity: runtime:List.last runtime:List.len runtime:List.map runtime:List.new runtime:List.partition runtime:List.pop
// parity: runtime:List.push runtime:List.reverse runtime:List.remove_at runtime:List.set
// parity: runtime:List.skip runtime:List.slice runtime:List.sort runtime:List.take
// parity: runtime:List.to_json_strings runtime:List.to_json_values
// parity: runtime:Log.error runtime:Log.error_json runtime:Log.trace runtime:Log.write
// parity: runtime:Log.write_json
// parity: runtime:Env.current_dir runtime:Env.get runtime:Env.get_or_default
// parity: runtime:Env.home_dir runtime:Env.run_workspace_root runtime:Env.set runtime:Env.temp_dir
// parity: runtime:File.append_bytes runtime:File.append_string runtime:File.exists
// parity: runtime:File.open runtime:File.open_read runtime:File.open_write
// parity: runtime:File.read_all runtime:File.read_all_async
// parity: runtime:File.read_all_string runtime:File.read_all_string_async runtime:File.read_into
// parity: runtime:File.read_bytes runtime:File.read_string runtime:File.remove
// parity: runtime:File.write runtime:File.write_async runtime:File.write_atomic runtime:File.write_bytes
// parity: runtime:File.write_bytes_view runtime:File.write_buffer runtime:File.write_buffer_view
// parity: runtime:File.write_string runtime:File.write_string_async runtime:File.write_string_to_path
// parity: runtime:FileError.message
// parity: runtime:FunctionObject.has_closure runtime:FunctionObject.new
// parity: runtime:PersistentMap.clear runtime:PersistentMap.contains_key runtime:PersistentMap.get
// parity: runtime:PersistentMap.insert runtime:PersistentMap.is_empty runtime:PersistentMap.len
// parity: runtime:PersistentMap.new runtime:PersistentMap.remove
// parity: runtime:Regex.captures runtime:Regex.compile runtime:Regex.find
// parity: runtime:Regex.is_match runtime:Regex.replace_all runtime:Regex.split
// parity: runtime:RegexError.message
// parity: runtime:Receiver.close runtime:Receiver.into_stream
// parity: runtime:Path.extension runtime:Path.file_name runtime:Path.from_string
// parity: runtime:Path.exists runtime:Path.is_absolute runtime:Path.is_dir runtime:Path.is_file
// parity: runtime:Path.join runtime:Path.list_files runtime:Path.list_paths
// parity: runtime:Path.normalize runtime:Path.parent runtime:Path.read_string
// parity: runtime:Path.resolve_relative runtime:Path.safe_relative
// parity: runtime:Path.starts_with runtime:Path.to_string runtime:Path.with_extension
// parity: runtime:Path.write_string
// parity: runtime:String.safe_relative runtime:String.to_path runtime:Workspace.resolve
// parity: runtime:Process.run runtime:Process.run_async runtime:Process.run_many_stdout
// parity: runtime:Process.run_many_stdout_async runtime:Process.run_many_stdout_timeout
// parity: runtime:Process.run_many_stdout_timeout_async runtime:Process.run_request
// parity: runtime:Process.run_request_async runtime:Process.run_request_cancellable_async
// parity: runtime:Process.run_stdout runtime:Process.run_stdout_async runtime:Process.run_stdout_timeout
// parity: runtime:Process.run_stdout_timeout_async runtime:Process.run_timeout runtime:Process.run_timeout_async
// parity: runtime:Process.stream
// parity: runtime:Map.clear runtime:Map.contains_key runtime:Map.get
// parity: runtime:Map.get_or_default runtime:Map.insert runtime:Map.insert_old
// parity: runtime:Map.is_empty runtime:Map.keys runtime:Map.len runtime:Map.new
// parity: runtime:Map.remove runtime:Map.values
// parity: runtime:Option.and_then runtime:Option.filter runtime:Option.is_none
// parity: runtime:Option.is_some runtime:Option.map runtime:Option.ok_or
// parity: runtime:Option.or runtime:Option.unwrap_or
// parity: runtime:Result.and_then runtime:Result.map runtime:Result.map_error
// parity: runtime:Ord.compare
// parity: runtime:Request.new runtime:Request.path
// parity: runtime:Response.body runtime:Response.ok runtime:Response.status
// parity: runtime:Result.err runtime:Result.err_message runtime:Result.is_err
// parity: runtime:Result.is_ok runtime:Result.ok runtime:Result.unwrap_or
// parity: runtime:Row.field_string runtime:RowBuffer.new
// parity: runtime:RuleLoader.load_rules
// parity: runtime:Set.clear runtime:Set.contains runtime:Set.difference runtime:Set.insert
// parity: runtime:Set.intersection runtime:Set.is_empty runtime:Set.is_subset runtime:Set.len
// parity: runtime:Set.new runtime:Set.remove runtime:Set.to_list runtime:Set.union
// parity: runtime:Sender.close
// parity: runtime:SortedSet.clear runtime:SortedSet.contains runtime:SortedSet.insert
// parity: runtime:SortedSet.is_empty runtime:SortedSet.len runtime:SortedSet.new
// parity: runtime:SortedSet.remove runtime:SortedSet.to_list
// parity: runtime:SortedMap.clear runtime:SortedMap.contains_key runtime:SortedMap.get
// parity: runtime:SortedMap.insert runtime:SortedMap.is_empty runtime:SortedMap.keys
// parity: runtime:SortedMap.len runtime:SortedMap.new runtime:SortedMap.remove
// parity: runtime:SortedMap.values
// parity: runtime:String.after runtime:String.before runtime:String.concat
// parity: runtime:String.chars
// parity: runtime:String.contains runtime:String.copy runtime:String.ends_with
// parity: runtime:String.env runtime:String.env_or
// parity: runtime:String.from_bool runtime:String.from_int
// parity: runtime:String.index_of runtime:String.is_empty runtime:String.join runtime:String.len
// parity: runtime:String.lines runtime:String.parse_int runtime:String.repeat
// parity: runtime:String.replace runtime:String.slice runtime:String.split
// parity: runtime:String.starts_with runtime:String.strip_prefix runtime:String.to_bytes
// parity: runtime:String.to_lowercase runtime:String.to_uppercase runtime:String.to_url runtime:String.trim
// parity: runtime:String.view runtime:StringView.after runtime:StringView.before
// parity: runtime:StringView.contains runtime:StringView.is_empty runtime:StringView.len
// parity: runtime:StringView.slice runtime:StringView.starts_with runtime:StringView.to_string
// parity: runtime:StringBuilder.finish runtime:StringBuilder.new runtime:StringBuilder.push
// parity: runtime:Stream.collect_list runtime:Stream.from_list
// parity: runtime:GlobalConfig.new runtime:GlobalConfig.replace runtime:GlobalConfig.rule_count
// parity: runtime:TempDir.new runtime:TempDir.new_in runtime:TempDir.path
// parity: runtime:Timer.sleep runtime:Timer.sleep_cancellable runtime:Timer.sleep_until
// parity: runtime:Tcp.connect runtime:TcpError.message
// parity: runtime:Toml.parse_file
// parity: runtime:Url.from_string runtime:Url.to_string
// parity: runtime:WebSocket.connect runtime:WebSocketError.message
// parity: runtime:Yaml.parse runtime:Yaml.parse_file
fn assert_interpreter_matches_backend(name: &str, package: &str, source: &str) {
    assert_interpreter_matches_backend_with_args(name, package, source, &[]);
}

fn run_with_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("large-stack test thread should spawn")
        .join()
        .expect("large-stack test thread should pass");
}

fn assert_interpreter_matches_backend_with_args(
    name: &str,
    package: &str,
    source: &str,
    args: &[&str],
) {
    assert_interpreter_matches_backend_with_distinct_args(name, package, source, args, args);
}

fn assert_interpreter_matches_backend_with_distinct_args(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
) {
    assert_interpreter_matches_backend_internal(
        name,
        package,
        source,
        interpreter_args,
        backend_args,
        false,
    );
}

fn assert_interpreter_matches_backend_with_distinct_args_allowing_unused_mut_warning(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
) {
    assert_interpreter_matches_backend_internal(
        name,
        package,
        source,
        interpreter_args,
        backend_args,
        true,
    );
}

fn assert_interpreter_matches_backend_internal(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
    allow_unused_mut_warning: bool,
) {
    let eval = eval_source_main_with_args(name, source, interpreter_args.iter().copied())
        .unwrap_or_else(|error| panic!("interpreter eval failed for {name}: {error:?}"));

    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let lowered = lower_source_to_rust_package(name, source, package, &runtime_path)
        .expect("parity fixture should lower");
    let package_dir = common::unique_temp_dir(package);
    write_generated_rust_package(&package_dir, &lowered).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .arg("--")
        .args(backend_args)
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "backend run failed for {name}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, eval.stdout, "stdout divergence for {name}");
    if allow_unused_mut_warning && eval.stderr.is_empty() && stderr.contains("unused_mut") {
        assert!(
            stderr.lines().all(|line| line.is_empty()
                || line.contains("variable does not need to be mutable")
                || line.contains("-->")
                || line.contains("let mut ")
                || line.contains("----")
                || line.contains("|")
                || line.contains("help: remove this `mut`")
                || line.contains("#[warn(unused_mut)]")
                || line.contains("on by default")),
            "backend stderr contained non-unused_mut warning for {name}:\n{stderr}"
        );
    } else {
        assert_eq!(stderr, eval.stderr, "stderr divergence for {name}");
    }
}

#[test]
fn parity_arithmetic_and_control_flow() {
    let source = r#"
fn sign(value: Int) -> fresh String {
    if value < 0 {
        return "neg"
    }
    if value == 0 {
        return "zero"
    }
    return "pos"
}

fn main() -> Unit {
    let total = 6 * 7 / 3 - 2 + 1
    Log.write(message: read String.from_int(value: total))
    Log.write(message: read sign(value: total))
    Log.write(message: read sign(value: 0 - 5))
    if 1 < 2 && 3 >= 3 {
        Log.write(message: read "and-true")
    }
    if 5 < 1 || 2 != 2 {
        Log.write(message: read "or-true")
    } else {
        Log.write(message: read "or-false")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-arithmetic.rss",
        "rsscript_parity_arithmetic",
        source,
    );
}

#[test]
fn parity_strings_and_receiver_methods() {
    let source = r#"
fn main() -> Unit {
    let greeting = String.concat(left: read "hi ", right: read "there")
    Log.write(message: read greeting)
    Log.write(message: read String.from_int(value: String.len(value: read greeting)))
    let count = greeting.len()
    Log.write(message: read String.from_int(value: count))
    let n = 255
    Log.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Log.write(message: read "blank-empty")
    }
    if String.is_empty(value: read greeting) {
        Log.write(message: read "greeting-empty")
    } else {
        Log.write(message: read "greeting-nonempty")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-strings.rss", "rsscript_parity_strings", source);
}

#[test]
fn parity_struct_match_field_and_assignment() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main() -> Unit {
    let point = Point(x: 3, y: 4)
    match read point {
        Point { x, y } => {
            let mut sum = x + y
            sum = sum + 100
            Log.write(message: read String.from_int(value: sum))
        }
    }
    Log.write(message: read String.from_int(value: point.x))
    Log.write(message: read String.from_int(value: point.y))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-struct.rss", "rsscript_parity_struct", source);
}

#[test]
fn parity_option_and_result_match() {
    let source = r#"
fn lookup(found: Bool) -> Option<Int> {
    if found {
        return Some(7)
    }
    return None
}

fn check(ok: Bool) -> Result<Int, String> {
    if ok {
        return Ok(1)
    }
    return Err("bad")
}

fn main() -> Unit {
    match lookup(found: true) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "none")
        }
    }
    match lookup(found: false) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "none")
        }
    }
    match check(ok: true) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    match check(ok: false) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-option-result.rss",
        "rsscript_parity_opt_res",
        source,
    );
}

#[test]
fn parity_option_and_result_helpers() {
    let source = r#"
fn maybe(found: Bool) -> Option<Int> {
    if found {
        return Some(7)
    }
    return None
}

fn checked(ok: Bool) -> Result<Int, String> {
    if ok {
        return Ok(3)
    }
    return Err("bad")
}

fn main() -> Unit {
    if Option.is_some<Int>(value: read maybe(found: true)) {
        Log.write(message: read "some")
    }
    if Option.is_none<Int>(value: read maybe(found: false)) {
        Log.write(message: read "none")
    }
    Log.write(message: read String.from_int(value: Option.unwrap_or<Int>(value: read maybe(found: false), default: read 9)))
    match Option.ok_or<Int, String>(value: read maybe(found: true), error: read "missing") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Option.ok_or<Int, String>(value: read maybe(found: false), error: read "missing") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    Log.write(message: read String.from_int(value: Option.unwrap_or<Int>(value: read Option.or<Int>(value: read maybe(found: false), fallback: read Some(11)), default: read 0)))
    let offset = 2
    match Option.map<Int, Int>(value: read maybe(found: true), mapper: |item| {
        return item + offset
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "map-none")
        }
    }
    match Option.and_then<Int, Int>(value: read maybe(found: true), mapper: |item| {
        return Some(item + 5)
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "and-then-none")
        }
    }
    match Option.filter<Int>(value: read maybe(found: true), predicate: |item| {
        return item > 3
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "filter-none")
        }
    }
    match Option.filter<Int>(value: read maybe(found: true), predicate: |item| {
        return item > 10
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "filter-none")
        }
    }

    if Result.is_ok<Int, String>(value: read checked(ok: true)) {
        Log.write(message: read "ok")
    }
    if Result.is_err<Int, String>(value: read checked(ok: false)) {
        Log.write(message: read "err")
    }
    Log.write(message: read String.from_int(value: Result.unwrap_or<Int, String>(value: read checked(ok: false), default: read 12)))
    match Result.ok<Int, String>(value: read checked(ok: true)) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "ok-none")
        }
    }
    match Result.err<Int, String>(value: read checked(ok: false)) {
        Some(error) => {
            Log.write(message: read error)
        }
        None => {
            Log.write(message: read "err-none")
        }
    }
    match Result.err_message<Int, String>(value: read checked(ok: false)) {
        Some(message) => {
            Log.write(message: read message)
        }
        None => {
            Log.write(message: read "message-none")
        }
    }
    match Result.map<Int, String, Int>(result: read checked(ok: true), mapper: |item| {
        return item + 4
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Result.and_then<Int, String, Int>(result: read checked(ok: true), mapper: |item| {
        return Ok(item + 6)
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Result.map_error<Int, String, String>(result: read checked(ok: false), mapper: |error| {
        return String.concat(left: read error, right: read "!")
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-option-result-helpers.rss",
        "rsscript_parity_option_result_helpers",
        source,
    );
}

#[test]
fn parity_while_loop_break_continue() {
    let source = r#"
fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 10 {
        i = i + 1
        if i == 3 {
            continue
        }
        if i == 7 {
            break
        }
        total = total + i
    }
    Log.write(message: read String.from_int(value: total))
    Log.write(message: read String.from_int(value: i))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-while.rss", "rsscript_parity_while", source);
}

#[test]
fn parity_user_sum_variants() {
    let source = r#"
sum Direction {
    North
    South
    East
    West
}

fn direction_name(d: read Direction) -> fresh String {
    match d {
        North => {
            return "north"
        }
        South => {
            return "south"
        }
        East => {
            return "east"
        }
        West => {
            return "west"
        }
    }
}

fn main() -> Unit {
    let south = South
    let west = West
    Log.write(message: read direction_name(d: read south))
    Log.write(message: read direction_name(d: read west))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-sum.rss", "rsscript_parity_sum", source);
}

#[test]
fn parity_logging_stdout_and_stderr() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read "stdout line")
    Log.write_json(value: read Json.value(value: read {"stream": "stdout", "count": 1}))
    Log.error(message: read "stderr line")
    Log.error_json(value: read Json.value(value: read {"stream": "stderr", "count": 2}))
    Log.trace(event: read "parity.event", message: read "traced")
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-logging.rss", "rsscript_parity_logging", source);
}

#[test]
fn parity_environment_function_object_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local root_value = Environment.root()
    let root = manage root_value
    if !Environment.has_parent(env: read root) {
        Log.write(message: read "root-no-parent")
    }
    if !Environment.has_function(env: read root) {
        Log.write(message: read "root-no-function")
    }

    local child_value = Environment.child(parent: read root)
    let child = manage child_value
    if Environment.has_parent(env: read child) {
        Log.write(message: read "child-parent")
    }
    if !Environment.has_function(env: read child) {
        Log.write(message: read "child-no-function")
    }

    local function_value = FunctionObject.new(closure: read child)
    let function = manage function_value
    if FunctionObject.has_closure(function: read function) {
        Log.write(message: read "function-closure")
    }
    Environment.bind_function(env: mut child, function: read function)
    if Environment.has_function(env: read child) {
        Log.write(message: read "child-function")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-environment-function.rss",
        "rsscript_parity_environment_function",
        source,
    );
}

#[test]
fn parity_cancellation_intrinsics() {
    let source = r#"
features: native, local

fn main() -> Unit {
    local source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    if !CancellationToken.is_cancelled(token: read token) {
        Log.write(message: read "not-cancelled")
    }

    CancellationSource.cancel(source: mut source)
    if CancellationToken.is_cancelled(token: read token) {
        Log.write(message: read "cancelled")
    }

    let second = CancellationSource.token(source: read source)
    if CancellationToken.is_cancelled(token: read second) {
        Log.write(message: read "second-cancelled")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-cancellation.rss",
        "rsscript_parity_cancellation",
        source,
    );
}

#[test]
fn parity_db_connection_intrinsics() {
    let source = r#"
features: native

fn main() -> Result<Unit, DbError> {
    let url = Url.from_string(value: read "db://parity")
    with DbConnection.open(url: read url) as conn {
        DbConnection.query(conn: mut conn, sql: read "select 1")?
    }
    with DbConnection.try_open(url: read url)? as conn {
        DbConnection.query(conn: mut conn, sql: read "select 2")?
    }
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend(
        "parity-db-connection.rss",
        "rsscript_parity_db_connection",
        source,
    );
}

#[test]
fn parity_string_scalar_intrinsics() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.trim(value: read "  Hello World  "))
    Log.write(message: read String.to_lowercase(value: read "MiXeD"))
    Log.write(message: read String.to_uppercase(value: read "MiXeD"))
    Log.write(message: read String.replace(value: read "a-b-c", from: read "-", to: read "+"))
    Log.write(message: read String.repeat(value: read "ab", count: 3))
    if String.contains(value: read "hello", needle: read "ell") {
        Log.write(message: read "contains-yes")
    } else {
        Log.write(message: read "contains-no")
    }
    if String.starts_with(value: read "hello", prefix: read "he") {
        Log.write(message: read "starts-yes")
    } else {
        Log.write(message: read "starts-no")
    }
    if String.ends_with(value: read "hello", suffix: read "xo") {
        Log.write(message: read "ends-yes")
    } else {
        Log.write(message: read "ends-no")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-string-scalar.rss",
        "rsscript_parity_string_scalar",
        source,
    );
}

#[test]
fn parity_string_option_intrinsics() {
    let source = r#"
fn main() -> Unit {
    match String.parse_int(value: read "42") {
        Some(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        None => {
            Log.write(message: read "parse-none")
        }
    }
    match String.parse_int(value: read "notnum") {
        Some(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        None => {
            Log.write(message: read "parse-none")
        }
    }
    match String.index_of(value: read "hello", needle: read "l") {
        Some(i) => {
            Log.write(message: read String.from_int(value: i))
        }
        None => {
            Log.write(message: read "idx-none")
        }
    }
    match String.strip_prefix(value: read "foobar", prefix: read "foo") {
        Some(rest) => {
            Log.write(message: read rest)
        }
        None => {
            Log.write(message: read "strip-none")
        }
    }
    match String.before(value: read "a=b", delimiter: read "=") {
        Some(part) => {
            Log.write(message: read part)
        }
        None => {
            Log.write(message: read "before-none")
        }
    }
    match String.after(value: read "a=b", delimiter: read "=") {
        Some(part) => {
            Log.write(message: read part)
        }
        None => {
            Log.write(message: read "after-none")
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-string-option.rss",
        "rsscript_parity_string_option",
        source,
    );
}

#[test]
fn parity_string_collection_and_conversion_intrinsics() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.copy(value: read "copy-me"))
    Log.write(message: read String.from_bool(value: true))
    Log.write(message: read String.slice(value: read "aébc", start: 0, len: 3))
    Log.write(message: read String.from_int(value: Bytes.len(value: read String.to_bytes(value: read "byte"))))
    let parts = String.split(value: read "red,green,blue", delimiter: read ",")
    Log.write(message: read String.from_int(value: List.len<String>(list: read parts)))
    Log.write(message: read List.join<String>(list: read parts, separator: read "|"))
    Log.write(message: read String.join(parts: read parts, separator: read "/"))
    let lines = String.lines(value: read "one\ntwo\n")
    Log.write(message: read String.from_int(value: List.len<String>(list: read lines)))
    Log.write(message: read List.join<String>(list: read lines, separator: read "+"))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-string-collections.rss",
        "rsscript_parity_string_collections",
        source,
    );
}

#[test]
fn parity_string_view_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let text = "aébc=tail"
    let view = String.view(value: read text, start: 0, len: 6)
    Log.write(message: read StringView.to_string(value: read view))
    Log.write(message: read String.from_int(value: StringView.len(value: read view)))
    if StringView.starts_with(value: read view, prefix: read "aé") {
        Log.write(message: read "starts")
    }
    if StringView.contains(value: read view, needle: read "bc") {
        Log.write(message: read "contains")
    }
    let empty = StringView.slice(value: read view, start: 99, len: 3)
    if StringView.is_empty(value: read empty) {
        Log.write(message: read "empty")
    }
    let slice = StringView.slice(value: read view, start: 1, len: 3)
    Log.write(message: read StringView.to_string(value: read slice))
    match StringView.before(value: read view, delimiter: read "=") {
        Some(left) => {
            Log.write(message: read StringView.to_string(value: read left))
        }
        None => {
            Log.write(message: read "before-none")
        }
    }
    match StringView.after(value: read view, delimiter: read "=") {
        Some(right) => {
            Log.write(message: read StringView.to_string(value: read right))
        }
        None => {
            Log.write(message: read "after-none")
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-string-view.rss",
        "rsscript_parity_string_view",
        source,
    );
}

#[test]
fn parity_char_and_string_chars_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let chars = String.chars(value: read "a1 \n")
    Log.write(message: read String.from_int(value: List.len<Char>(list: read chars)))
    let first = chars[0]
    Log.write(message: read Char.to_string(value: read first))
    Log.write(message: read String.from_int(value: Char.to_code(value: read first)))
    if Char.is_alpha(value: read first) {
        Log.write(message: read "alpha")
    }
    if Char.is_alphanumeric(value: read first) {
        Log.write(message: read "alnum")
    }
    match Char.from_code(value: 49) {
        Some(ch) => {
            if Char.is_digit(value: read ch) {
                Log.write(message: read "digit")
            }
            let original = chars[1]
            Log.write(message: read String.from_int(value: Char.compare(left: read original, right: read ch)))
        }
        None => {
            Log.write(message: read "bad-code")
        }
    }
    match Char.from_code(value: 32) {
        Some(ch) => {
            if Char.is_whitespace(value: read ch) {
                Log.write(message: read "space")
            }
        }
        None => {
            Log.write(message: read "bad-space")
        }
    }
    match Char.from_code(value: 0 - 1) {
        Some(ch) => {
            Log.write(message: read Char.to_string(value: read ch))
        }
        None => {
            Log.write(message: read "invalid")
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-char.rss", "rsscript_parity_char", source);
}

#[test]
fn parity_string_builder_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local builder = StringBuilder.new()
    StringBuilder.push(builder: mut builder, value: read "rss")
    StringBuilder.push(builder: mut builder, value: read "cript")
    StringBuilder.push(builder: mut builder, value: read "-")
    StringBuilder.push(builder: mut builder, value: read String.from_int(value: 6))
    Log.write(message: read StringBuilder.finish(builder: take builder))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-string-builder.rss",
        "rsscript_parity_string_builder",
        source,
    );
}

#[test]
fn parity_list_literal_index_and_for_loop() {
    let source = r#"
fn main() -> Unit {
    let values: List<Int> = [1, 2, 3, 4]
    Log.write(message: read String.from_int(value: values[2]))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read values, index: 1)))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    let mut total = 0
    for value in values {
        if value == 2 {
            continue
        }
        total = total + value
    }
    Log.write(message: read String.from_int(value: total))
    match List.first<Int>(list: read values) {
        Some(first) => {
            Log.write(message: read String.from_int(value: first))
        }
        None => {
            Log.write(message: read "empty")
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-list.rss", "rsscript_parity_list", source);
}

#[test]
fn parity_map_literal_and_read_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let table: Map<String, Int> = {"a" => 1, "b" => 2}
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    if Map.contains_key<String, Int>(map: read table, key: read "b") {
        Log.write(message: read "has-b")
    }
    match Map.get<String, Int>(map: read table, key: read "a") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    Log.write(message: read String.from_int(value: Map.get_or_default<String, Int>(map: read table, key: read "z", default: read 9)))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-map.rss", "rsscript_parity_map", source);
}

#[test]
fn parity_persistent_map_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let empty = PersistentMap<String, Int>.new()
    let one = PersistentMap.insert<String, Int>(map: read empty, key: read "one", value: read 1)
    let two = PersistentMap.insert<String, Int>(map: read one, key: read "two", value: read 2)
    let old_missing = PersistentMap.contains_key<String, Int>(map: read empty, key: read "one")
    let has_one = PersistentMap.contains_key<String, Int>(map: read one, key: read "one")
    let value = PersistentMap.get<String, Int>(map: read one, key: read "one")
    let removed = PersistentMap.remove<String, Int>(map: read two, key: read "one")
    let cleared = PersistentMap.clear<String, Int>(map: read two)

    if old_missing {
        Log.write(message: read "bad-empty")
    }
    if has_one {
        Log.write(message: read "has-one")
    }
    Log.write(message: read String.from_int(value: PersistentMap.len<String, Int>(map: read two)))
    if PersistentMap.is_empty<String, Int>(map: read cleared) {
        Log.write(message: read "cleared")
    }
    match value {
        Some(item) => {
            Log.write(message: read String.from_int(value: item))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    if PersistentMap.contains_key<String, Int>(map: read removed, key: read "one") {
        Log.write(message: read "bad-removed")
    }
    if PersistentMap.contains_key<String, Int>(map: read two, key: read "one") {
        Log.write(message: read "original-kept")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-persistent-map.rss",
        "rsscript_parity_persistent_map",
        source,
    );
}

#[test]
fn parity_path_intrinsics() {
    run_with_large_stack(|| {
        let source = r#"
fn main() -> Unit {
    let root = Path.from_string(value: read "fixtures")
    let path = Path.join(base: read root, child: read "rsscript-path.txt")
    Log.write(message: read Path.to_string(path: read path))
    Log.write(message: read Path.to_string(path: read "fixtures/rsscript-path.txt".to_path()))
    Log.write(message: read Path.to_string(path: read Path.normalize(path: read Path.join(base: read path, child: read ".."))))

    match Path.file_name(path: read path) {
        Some(name) => {
            Log.write(message: read name)
        }
        None => {
            Log.write(message: read "no-name")
        }
    }
    match Path.extension(path: read path) {
        Some(extension) => {
            Log.write(message: read extension)
        }
        None => {
            Log.write(message: read "no-extension")
        }
    }
    match Path.parent(path: read path) {
        Some(parent) => {
            Log.write(message: read Path.to_string(path: read parent))
        }
        None => {
            Log.write(message: read "no-parent")
        }
    }

    if Path.is_absolute(path: read Path.from_string(value: read "/tmp/rsscript")) {
        Log.write(message: read "absolute")
    }
    if Path.starts_with(path: read path, base: read root) {
        Log.write(message: read "starts")
    }
    Log.write(message: read Path.to_string(path: read Path.with_extension(path: read path, extension: read "json")))

    match Path.safe_relative(value: read "fixtures/./rsscript-path.txt") {
        Ok(safe) => {
            Log.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match "../escape".safe_relative() {
        Ok(safe) => {
            Log.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Path.resolve_relative(root: read root, relative: read "rsscript-path.txt") {
        Ok(resolved) => {
            Log.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Workspace.resolve(root: read root, relative: read "nested/../bad") {
        Ok(resolved) => {
            Log.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Unit
}
"#;
        assert_interpreter_matches_backend("parity-path.rss", "rsscript_parity_path", source);
    });
}

#[test]
fn parity_url_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let direct = Url.from_string(value: read "https://example.test/a?b=1")
    Log.write(message: read Url.to_string(url: read direct))
    let from_method: Url = "https://example.test/from-method".to_url()
    Log.write(message: read Url.to_string(url: read from_method))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-url.rss", "rsscript_parity_url", source);
}

#[test]
fn parity_string_env_intrinsics_for_missing_values() {
    let source = r#"
fn main() -> Result<Unit, FileError> {
    match Env.get(name: read "__RSSCRIPT_PARITY_ENV_MISSING__") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "env-missing")
        }
    }
    Log.write(message: read Env.get_or_default(name: read "__RSSCRIPT_PARITY_ENV_MISSING__", default: read "env-fallback"))
    Env.set(name: read "__RSSCRIPT_PARITY_ENV_SET__", value: read "ignored")
    let current = Env.current_dir()?
    if Path.is_dir(path: read current) {
        Log.write(message: read "current-dir")
    }
    let root = Env.run_workspace_root()
    if Path.is_dir(path: read root) {
        Log.write(message: read "workspace-root")
    }
    match Env.home_dir() {
        Some(path) => {
            if Path.is_dir(path: read path) {
                Log.write(message: read "home-dir")
            } else {
                Log.write(message: read "home-path")
            }
        }
        None => {
            Log.write(message: read "home-none")
        }
    }
    if Path.is_dir(path: read Env.temp_dir()) {
        Log.write(message: read "temp-dir")
    }
    match String.env(value: read "__RSSCRIPT_PARITY_ENV_MISSING__") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "string-missing")
        }
    }
    Log.write(message: read String.env_or(value: read "__RSSCRIPT_PARITY_ENV_MISSING__", default: read "fallback"))
    Log.write(message: read "__RSSCRIPT_PARITY_ENV_MISSING__".env_or("method-fallback"))
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend("parity-env.rss", "rsscript_parity_env", source);
}

#[test]
fn parity_config_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-config-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-config-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter config dir should be created");
    fs::create_dir_all(&backend_root).expect("backend config dir should be created");

    for root in [&interpreter_root, &backend_root] {
        fs::write(root.join("config.txt"), "\nprimary\nignored\n")
            .expect("config fixture should write");
        fs::write(root.join("rules.txt"), "one\ntwo\n\nthree\n")
            .expect("rules fixture should write");
    }

    let interpreter_args = [
        interpreter_root.join("config.txt").display().to_string(),
        interpreter_root.join("rules.txt").display().to_string(),
    ];
    let backend_args = [
        backend_root.join("config.txt").display().to_string(),
        backend_root.join("rules.txt").display().to_string(),
    ];
    let interpreter_arg_refs = interpreter_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let backend_arg_refs = backend_args.iter().map(String::as_str).collect::<Vec<_>>();

    let source = r#"
fn main() -> Result<Unit, ConfigError> {
    let config_path = Path.from_string(value: read Args.get_or_default(index: 0, default: read "config.txt"))
    let rules_path = Path.from_string(value: read Args.get_or_default(index: 1, default: read "rules.txt"))

    let value = Config.load(path: read config_path)?
    Log.write(message: read Config.name(value: read value))
    let mut store = ConfigStore.new(value: read value)
    Log.write(message: read ConfigStore.name(store: read store))

    let rules = RuleLoader.load_rules(path: read rules_path)?
    let config = Config.new(name: read "rules", rules: read rules)
    Log.write(message: read String.from_int(value: Config.rule_count(config: read config)))
    let mut global = GlobalConfig.new(value: read config)
    Log.write(message: read String.from_int(value: GlobalConfig.rule_count(global: read global)))

    let next = Config.load(path: read config_path)?
    ConfigStore.replace(store: mut store, value: read next)
    Log.write(message: read ConfigStore.name(store: read store))

    let empty_rules = List<Rule>.new()
    let empty = Config.new(name: read "empty", rules: read empty_rules)
    GlobalConfig.replace(global: mut global, value: read empty)
    Log.write(message: read String.from_int(value: GlobalConfig.rule_count(global: read global)))
    return Ok(Unit)
}
"#;

    assert_interpreter_matches_backend_with_distinct_args(
        "parity-config.rss",
        "rsscript_parity_config",
        source,
        &interpreter_arg_refs,
        &backend_arg_refs,
    );

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_regex_intrinsics() {
    let source = r#"
features: native, local

fn main() -> Unit {
    match Regex.compile(pattern: read "([a-z]+)-(\\d+)") {
        Ok(regex) => {
            if Regex.is_match(regex: read regex, value: read "item-42") {
                Log.write(message: read "matched")
            }
            match Regex.find(regex: read regex, value: read "pre item-42 post") {
                Some(found) => {
                    Log.write(message: read found)
                }
                None => {
                    Log.write(message: read "find-none")
                }
            }
            let captures = Regex.captures(regex: read regex, value: read "item-42")
            Log.write(message: read List.join<String>(list: read captures, separator: read "|"))
            Log.write(message: read Regex.replace_all(regex: read regex, value: read "item-42 other-7", replacement: read "x"))
            let parts = Regex.split(regex: read regex, value: read "a item-42 b other-7 c")
            Log.write(message: read List.join<String>(list: read parts, separator: read "/"))
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    match Regex.compile(pattern: read "[") {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-regex.rss", "rsscript_parity_regex", source);
}

#[test]
fn parity_args_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let args = Args.all()
    Log.write(message: read String.from_int(value: Args.count()))
    Log.write(message: read List.join<String>(list: read args, separator: read "|"))
    match Args.get(index: 0) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "first-none")
        }
    }
    match Args.get(index: 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "third-none")
        }
    }
    match Args.get(index: 99) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing-none")
        }
    }
    match Args.get(index: 0 - 1) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "negative-none")
        }
    }
    Log.write(message: read Args.get_or_default(index: 1, default: read "fallback"))
    Log.write(message: read Args.get_or_default(index: 99, default: read "fallback"))
    Log.write(message: read Args.get_or_default(index: 0 - 1, default: read "negative-fallback"))
    return Unit
}
"#;
    assert_interpreter_matches_backend_with_args(
        "parity-args.rss",
        "rsscript_parity_args",
        source,
        &["alpha", "beta value", "gamma"],
    );
}

#[test]
fn parity_request_response_intrinsics() {
    let source = r#"
fn main() -> Result<Unit, HttpError> {
    let request = Request.new(path: read "/users/42")
    Log.write(message: read Request.path(request: read request))
    let response = Response.ok(body: read "handled")?
    Log.write(message: read String.from_int(value: Response.status(response: read response)))
    Log.write(message: read Response.body(response: read response))
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend(
        "parity-request-response.rss",
        "rsscript_parity_request_response",
        source,
    );
}

#[test]
fn parity_sync_http_error_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let url = Url.from_string(value: read "https://example.test/api")
    match Http.get(url: read url) {
        Ok(response) => {
            Log.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "GET https://example.test/api") {
                Log.write(message: read "get-error")
            }
        }
    }
    match Http.post_json(url: read url, body: read "{\"ok\":true}") {
        Ok(response) => {
            Log.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "POST JSON https://example.test/api") {
                Log.write(message: read "post-json-error")
            }
        }
    }
    match Http.post_form(url: read url, body: read "a=1") {
        Ok(response) => {
            Log.write(message: read String.from_int(value: HttpResponse.status(response: read response)))
        }
        Err(error) => {
            let message = HttpError.message(error: read error)
            if String.contains(value: read message, needle: read "POST form https://example.test/api") {
                Log.write(message: read "post-form-error")
            }
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-http-sync.rss", "rsscript_parity_http_sync", source);
}

#[test]
fn parity_async_http_error_intrinsics() {
    let source = r#"
features: async, native, local

fn log_http_error(error: read HttpError, label: read String) -> Unit {
    let message = HttpError.message(error: read error)
    if String.contains(value: read message, needle: read "") {
        Log.write(message: read label)
    }
    return Unit
}

async fn main() -> Unit {
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
    assert_interpreter_matches_backend(
        "parity-http-async.rss",
        "rsscript_parity_http_async",
        source,
    );
}

#[test]
fn parity_async_socket_error_intrinsics() {
    let source = r#"
features: async, native, local

async fn main() -> Unit {
    match await Tcp.connect(host: read "127.0.0.1", port: 9) {
        Ok(_) => {}
        Err(error) => {
            let message = TcpError.message(error: read error)
            if String.contains(value: read message, needle: read "") {
                Log.write(message: read "tcp-error")
            }
        }
    }
    let url = Url.from_string(value: read "ws://127.0.0.1:9/socket")
    match await WebSocket.connect(url: read url) {
        Ok(_) => {}
        Err(error) => {
            let message = WebSocketError.message(error: read error)
            if String.contains(value: read message, needle: read "") {
                Log.write(message: read "websocket-error")
            }
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-async-socket.rss",
        "rsscript_parity_async_socket",
        source,
    );
}

#[test]
fn parity_http_request_builder_intrinsics() {
    let source = r#"
features: native, local

fn main() -> Unit {
    let url = Url.from_string(value: read "https://example.test/api")
    local base = HttpRequest.json(url: read url, body: read "{\"ok\":true}")
    local timed = HttpRequest.with_timeout(request: take base, timeout_ms: 250)
    local retry = HttpRequest.with_retry(request: take timed, attempts: 3, backoff_ms: 50)
    local with_header = HttpRequest.with_header(request: take retry, name: read "X-Test", value: read "rss")
    HttpRequest.with_header(request: take with_header, name: read "X-Trace", value: read "1")
    Log.write(message: read "http-request-built")
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-http-request.rss",
        "rsscript_parity_http_request",
        source,
    );
}

#[test]
fn parity_path_file_directory_intrinsics() {
    run_with_large_stack(|| {
        let interpreter_root = common::unique_temp_dir("rsscript-parity-fs-interpreter");
        let backend_root = common::unique_temp_dir("rsscript-parity-fs-backend");
        let interpreter_root_arg = interpreter_root.display().to_string();
        let backend_root_arg = backend_root.display().to_string();
        let source = r#"
features: native, local

fn main() -> Result<Unit, FileError> {
    let root = Args.get_or_default(index: 0, default: read "target/rsscript-parity-fs")
    let root_path = Path.from_string(value: read root)
    let nested = Path.join(base: read root_path, child: read "nested")
    let single = Path.join(base: read root_path, child: read "single")
    Directory.create_all(path: read nested)?
    Directory.create_dir_all(path: read nested)?
    Directory.create(path: read single)?
    if Directory.exists(path: read root_path) {
        Log.write(message: read "root-exists")
    }
    if Directory.is_dir(path: read nested) {
        Log.write(message: read "nested-dir")
    }
    if Path.exists(path: read single) {
        Log.write(message: read "single-exists")
    }
    if Path.is_dir(path: read single) {
        Log.write(message: read "single-dir")
    }

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Path.write_string(path: read path_file, text: read "path text")?
    if Path.is_file(path: read path_file) {
        Log.write(message: read "path-file")
    }
    if File.exists(path: read path_file) {
        Log.write(message: read "file-exists")
    }
    if Directory.is_file(path: read path_file) {
        Log.write(message: read "directory-sees-file")
    }
    let path_text = Path.read_string(path: read path_file)?
    Log.write(message: read path_text)
    let path_digest = Hash.sha256_file(path: read path_file)?
    Assert.equal(left: read path_digest, right: read "c6465e0abd2e3c2f5ccfe7f639ddc0f72282904663b09ddd8dffbe060be35f97")

    File.write_string_to_path(path: read path_file, text: read "file text")?
    let file_text = File.read_string(path: read path_file)?
    Log.write(message: read file_text)

    let bytes_file = Path.join(base: read nested, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    File.append_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "de"))?
    File.append_string(path: read bytes_file, text: read "f")?
    let bytes = File.read_bytes(path: read bytes_file)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))

    let handle_file = Path.join(base: read nested, child: read "handle.txt")
    with File.open_write(path: read handle_file)? as writer {
        File.write(file: mut writer, data: read Bytes.from_string(value: read "ab"))?
        File.write_string(file: mut writer, text: read "cd")?
        local empty_buffer = Buffer.new(size: 0)
        File.write_buffer(file: mut writer, buffer: read empty_buffer)?
        File.write_bytes_view(file: mut writer, data: read Bytes.view(value: read Bytes.from_string(value: read "gh"), start: 0, len: 2))?
        let empty_view = Buffer.view(buffer: read empty_buffer, start: 0, len: 0)
        File.write_buffer_view(file: mut writer, buffer: read empty_view)?
    }
    with File.open(path: read handle_file)? as reader {
        let all = File.read_all(file: mut reader)?
        Log.write(message: read String.from_int(value: Bytes.len(value: read all)))
        let empty = File.read_all(file: mut reader)?
        Log.write(message: read String.from_int(value: Bytes.len(value: read empty)))
    }
    with File.open_read(path: read handle_file)? as reader_text {
        let text_all = File.read_all_string(file: mut reader_text)?
        Log.write(message: read text_all)
    }
    with File.open_read(path: read handle_file)? as reader_into {
        local into_buffer = Buffer.new(size: 0)
        if File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Log.write(message: read String.from_int(value: Buffer.len(buffer: read into_buffer)))
        }
        if !File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Log.write(message: read "read-into-empty")
        }
    }

    Directory.write_string(path: read path_file, content: read "directory text")?
    let directory_text = Directory.read_string(path: read path_file)?
    Log.write(message: read directory_text)
    let metadata = Directory.metadata(path: read path_file)?
    if metadata.is_file {
        Log.write(message: read "metadata-file")
    }
    Log.write(message: read String.from_int(value: metadata.len))

    let files = Directory.list_files(path: read root_path)?
    Log.write(message: read List.join<String>(list: read files, separator: read "|"))
    let files_from_path = Path.list_files(path: read root_path)?
    Log.write(message: read List.join<String>(list: read files_from_path, separator: read "|"))
    let paths = Directory.list_paths(path: read nested)?
    Log.write(message: read String.from_int(value: List.len<Path>(list: read paths)))
    match Path.file_name(path: read paths[0]) {
        Some(name) => {
            Log.write(message: read name)
        }
        None => {
            Log.write(message: read "path-name-none")
        }
    }
    let paths_from_path = Path.list_paths(path: read nested)?
    Log.write(message: read String.from_int(value: List.len<Path>(list: read paths_from_path)))

    match File.read_string(path: read Path.from_string(value: read "__rsscript_missing_file_for_parity__")) {
        Ok(value) => {
            Log.write(message: read value)
        }
        Err(error) => {
            let message = FileError.message(error: read error)
            if String.contains(value: read message, needle: read "No such file") {
                Log.write(message: read "missing-file-error")
            } else {
                Log.write(message: read "other-file-error")
            }
        }
    }
    let copied = Path.join(base: read nested, child: read "copied.txt")
    Directory.copy_file(from: read path_file, to: read copied)?
    let copied_text = File.read_string(path: read copied)?
    Log.write(message: read copied_text)
    let renamed = Path.join(base: read nested, child: read "renamed.txt")
    Directory.rename(from: read copied, to: read renamed)?
    if File.exists(path: read renamed) {
        Log.write(message: read "renamed-exists")
    }
    let atomic = Path.join(base: read nested, child: read "atomic.txt")
    File.write_atomic(path: read atomic, text: read "atomic text")?
    let atomic_text = File.read_string(path: read atomic)?
    Log.write(message: read atomic_text)
    File.remove(path: read atomic)?
    if !File.exists(path: read atomic) {
        Log.write(message: read "atomic-removed")
    }
    Directory.remove_file(path: read renamed)?
    if !File.exists(path: read renamed) {
        Log.write(message: read "renamed-removed")
    }
    Directory.remove_dir_all(path: read single)?
    if !Path.exists(path: read single) {
        Log.write(message: read "single-removed")
    }
    return Ok(Unit)
}
"#;
        assert_interpreter_matches_backend_with_distinct_args(
            "parity-fs.rss",
            "rsscript_parity_fs",
            source,
            &[interpreter_root_arg.as_str()],
            &[backend_root_arg.as_str()],
        );
        let _ = fs::remove_dir_all(&interpreter_root);
        let _ = fs::remove_dir_all(&backend_root);
    });
}

#[test]
fn parity_csv_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-csv-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-csv-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter csv dir should be created");
    fs::create_dir_all(&backend_root).expect("backend csv dir should be created");

    for root in [&interpreter_root, &backend_root] {
        fs::write(
            root.join("data.csv"),
            "name,amount\nRSScript, 42\nOther, 7\n",
        )
        .expect("csv fixture should write");
    }

    let interpreter_path = interpreter_root.join("data.csv").display().to_string();
    let backend_path = backend_root.join("data.csv").display().to_string();
    let source = r#"
features: local

fn main() -> Result<Unit, CsvError> {
    let path = Path.from_string(value: read Args.get_or_default(index: 0, default: read "data.csv"))
    local buffer = RowBuffer.new(size: 4096)

    with Csv.open_read(path: read path)? as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
    }

    let row = Csv.parse_row(buffer: read buffer)?
    let name = Row.field_string(row: read row, index: 0)?
    let amount = Row.field_string(row: read row, index: 1)?
    Log.write(message: read name)
    Log.write(message: read amount)
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend_with_distinct_args(
        "parity-csv.rss",
        "rsscript_parity_csv",
        source,
        &[interpreter_path.as_str()],
        &[backend_path.as_str()],
    );

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_tempdir_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-tempdir-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-tempdir-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter tempdir root should be created");
    fs::create_dir_all(&backend_root).expect("backend tempdir root should be created");
    let interpreter_root_arg = interpreter_root.display().to_string();
    let backend_root_arg = backend_root.display().to_string();

    let source = r#"
features: native, local

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rsscript-parity-tempdir"))
    with TempDir.new_in(parent: read root)? as child {
        let path = TempDir.path(dir: read child)
        if Path.is_dir(path: read path) {
            Log.write(message: read "new-in-dir")
        }
        Directory.remove_dir_all(path: read path)?
    }
    with TempDir.new()? as created {
        let path = TempDir.path(dir: read created)
        if Path.is_dir(path: read path) {
            Log.write(message: read "new-dir")
        }
        Directory.remove_dir_all(path: read path)?
    }
    return Ok(Unit)
}
"#;

    assert_interpreter_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-tempdir.rss",
        "rsscript_parity_tempdir",
        source,
        &[interpreter_root_arg.as_str()],
        &[backend_root_arg.as_str()],
    );

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_bytes_and_json_literals() {
    let source = r#"
fn main() -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let part = Bytes.slice(value: read data, start: 2, len: 3)
    Log.write(message: read String.from_int(value: Bytes.len(value: read part)))
    if BytesView.starts_with(value: read Bytes.view(value: read data, start: 1, len: 3), prefix: read Bytes.view(value: read data, start: 1, len: 2)) {
        Log.write(message: read "bytes-prefix")
    }
    let doc: JsonValue = {"ok": true, "name": "rss"}
    Log.write(message: read Json.kind(value: read doc))
    if Json.is_object(value: read doc) {
        Log.write(message: read "json-object")
    }
    Log.write(message: read Json.to_string(value: read Json.value(value: read {"answer": 42})))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-bytes-json.rss",
        "rsscript_parity_bytes_json",
        source,
    );
}

#[test]
fn parity_assert_hash_and_bytes_consume_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    let digest = Hash.sha256_string(value: read "abc")
    Assert.equal(left: read digest, right: read "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    let bytes = Bytes.from_string(value: read "abc")
    Assert.equal(left: read Hash.sha256_bytes(value: read bytes), right: read digest)
    Assert.equal_int(left: Bytes.len(value: read bytes), right: 3)
    Assert.equal_bool(left: Bytes.is_empty(value: read bytes), right: false)
    local disposable = Bytes.concat(left: read bytes, right: read Bytes.from_string(value: read "!"))
    Bytes.consume(bytes: take disposable)
    Log.write(message: read "assert-hash-ok")
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-assert-hash-bytes.rss",
        "rsscript_parity_assert_hash_bytes",
        source,
    );
}

#[test]
fn parity_diff_and_patch_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let original = "one\ntwo\nthree\n"
    let changed = "one\n2\nthree\n"
    let patch = Diff.unified(old: read original, new: read changed)
    match Patch.apply_text(original: read original, patch: read patch) {
        Ok(applied) => {
            Log.write(message: read applied)
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    let empty_patch = Diff.unified(old: read original, new: read original)
    match Patch.apply_text(original: read original, patch: read empty_patch) {
        Ok(applied) => {
            Assert.equal(left: read applied, right: read original)
            Log.write(message: read "empty-ok")
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Patch.apply_text(original: read "old\n", patch: read "--- old\n+++ new\n@@ -1,1 +1,1 @@\n-bad\n+new\n") {
        Ok(applied) => {
            Log.write(message: read applied)
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-diff-patch.rss",
        "rsscript_parity_diff_patch",
        source,
    );
}

#[test]
fn parity_process_run_intrinsics() {
    let source = r#"
features: native

fn main() -> Result<Unit, String> {
    let mut args = List<String>.new()
    List.push<String>(list: mut args, value: read "hello")
    let stdout = Process.run_stdout(command: read "printf", args: read args)?
    Log.write(message: read stdout)

    let mut run_args = List<String>.new()
    List.push<String>(list: mut run_args, value: read "world")
    let output = Process.run(command: read "printf", args: read run_args)?
    Log.write(message: read String.from_int(value: output.status))
    Log.write(message: read output.stdout)
    Log.write(message: read output.stderr)
    Log.write(message: read output.merged)
    if output.truncated == false {
        Log.write(message: read "not-truncated")
    }

    let mut timeout_args = List<String>.new()
    List.push<String>(list: mut timeout_args, value: read "timeout")
    let timeout_stdout = Process.run_stdout_timeout(command: read "printf", args: read timeout_args, timeout_ms: 1000)?
    Log.write(message: read timeout_stdout)

    let mut run_timeout_args = List<String>.new()
    List.push<String>(list: mut run_timeout_args, value: read "done")
    let timed = Process.run_timeout(command: read "printf", args: read run_timeout_args, timeout_ms: 1000)?
    Log.write(message: read String.from_int(value: timed.status))
    Log.write(message: read timed.stdout)
    Log.write(message: read timed.merged)

    let mut format_args = List<String>.new()
    List.push<String>(list: mut format_args, value: read "%s")
    let items = ["A", "B"]
    let many = Process.run_many_stdout(command: read "printf", args: read format_args, appended_args: read items, jobs: 2)?
    Log.write(message: read List.join<String>(list: read many, separator: read "|"))

    let many_timeout = Process.run_many_stdout_timeout(command: read "printf", args: read format_args, appended_args: read items, jobs: 2, timeout_ms: 1000)?
    Log.write(message: read List.join<String>(list: read many_timeout, separator: read "|"))

    let stdin_request = ProcessRequest(
        command: "cat",
        args: List<String>.new(),
        cwd: None,
        stdin: Some("stdin-body"),
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let stdin_output = Process.run_request(request: read stdin_request)?
    Log.write(message: read String.from_int(value: stdin_output.status))
    Log.write(message: read stdin_output.stdout)
    Log.write(message: read stdin_output.merged)

    let capped_request = ProcessRequest(
        command: "printf",
        args: ["%s", "abcdef"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 3,
    )
    let capped = Process.run_request(request: read capped_request)?
    Log.write(message: read capped.stdout)
    if capped.truncated {
        Log.write(message: read "request-truncated")
    }

    let stream_request = ProcessRequest(
        command: "true",
        args: List<String>.new(),
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let events = Process.stream(request: read stream_request)?
    match Stream.collect_list<ProcessEvent>(stream: read events) {
        Ok(collected_events) => {
            Log.write(message: read String.from_int(value: List.len<ProcessEvent>(list: read collected_events)))
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend("parity-process.rss", "rsscript_parity_process", source);
}

#[test]
fn parity_buffer_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local buffer = Buffer.new(size: 16)
    if Buffer.is_empty(buffer: read buffer) {
        Log.write(message: read "buffer-empty")
    }
    Log.write(message: read String.from_int(value: Buffer.len(buffer: read buffer)))
    let view = Buffer.view(buffer: read buffer, start: 0, len: 10)
    if BufferView.is_empty(value: read view) {
        Log.write(message: read "view-empty")
    }
    Log.write(message: read String.from_int(value: BufferView.len(value: read view)))
    let slice = BufferView.slice(value: read view, start: 1, len: 2)
    Log.write(message: read String.from_int(value: Bytes.len(value: read BufferView.to_bytes(value: read slice))))
    Log.write(message: read String.from_int(value: Bytes.len(value: read Bytes.from_buffer(buffer: read buffer))))
    Buffer.clear(buffer: mut buffer)
    Buffer.consume(buffer: take buffer)
    Log.write(message: read "consumed")
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-buffer.rss", "rsscript_parity_buffer", source);
}

#[test]
fn parity_list_mutating_intrinsics_and_index_assignment() {
    let source = r#"
fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)
    List.set<Int>(list: mut values, index: 1, value: read 20)
    values[2] = 30
    let suffix: List<Int> = [40, 50]
    List.append<Int>(list: mut values, values: read suffix)
    match List.pop<Int>(list: mut values) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "pop-none")
        }
    }
    match List.remove_at<Int>(list: mut values, index: 1) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read values, index: 1)))
    List.clear<Int>(list: mut values)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-list-mutating.rss",
        "rsscript_parity_list_mutating",
        source,
    );
}

#[test]
fn parity_list_non_closure_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut numbers: List<Int> = [3, 1, 2, 5, 4]
    List.sort<Int>(list: mut numbers)
    Log.write(message: read String.from_int(value: numbers[0]))
    Log.write(message: read String.from_int(value: numbers[4]))

    let reversed = List.reverse<Int>(list: read numbers)
    Log.write(message: read String.from_int(value: reversed[0]))
    let skipped = List.skip<Int>(list: read numbers, count: 2)
    Log.write(message: read String.from_int(value: skipped[0]))
    local taken = List.take<Int>(list: read numbers, count: 3)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read taken)))
    let sliced = List.slice<Int>(list: read numbers, start: 1, len: 3)
    Log.write(message: read String.from_int(value: sliced[0]))
    Log.write(message: read String.from_int(value: sliced[2]))

    let words: List<String> = ["a", "b"]
    let json_strings = List.to_json_strings(list: read words)
    Log.write(message: read Json.to_string(value: read json_strings))
    let json_values: List<JsonValue> = [Json.value(value: read {"n": 1}), Json.value(value: read {"n": 2})]
    let json_array = List.to_json_values(list: read json_values)
    Log.write(message: read Json.to_string(value: read json_array))

    List.consume<Int>(list: take taken)
    Log.write(message: read "consumed")
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-list-non-closure.rss",
        "rsscript_parity_list_non_closure",
        source,
    );
}

#[test]
fn parity_list_closure_intrinsics() {
    let source = r#"
features: local

fn is_even(value: Int) -> Bool {
    let half = value / 2
    return half * 2 == value
}

fn main() -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4, 5]
    let threshold = 3

    Log.write(message: read String.from_int(value: List.count_where<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    })))
    Log.write(message: read String.from_bool(value: List.any<Int>(list: read numbers, predicate: |item| {
        return item == 5
    })))
    Log.write(message: read String.from_bool(value: List.all<Int>(list: read numbers, predicate: |item| {
        return item > 0
    })))
    Log.write(message: read String.from_bool(value: List.contains<Int>(list: read numbers, predicate: |item| {
        return item == 3
    })))

    match List.find<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "find-none")
        }
    }

    let filtered = List.filter<Int>(list: read numbers, predicate: |item| {
        return is_even(value: item)
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read filtered)))
    Log.write(message: read String.from_int(value: filtered[0]))
    Log.write(message: read String.from_int(value: filtered[1]))

    let mapped = List.map<Int, Int>(list: read numbers, mapper: |item| {
        return item + threshold
    })
    Log.write(message: read String.from_int(value: mapped[0]))
    Log.write(message: read String.from_int(value: mapped[4]))

    let flattened = List.flat_map<Int, Int>(list: read filtered, mapper: |item| {
        let values: List<Int> = [item, item + 10]
        return values
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read flattened)))
    Log.write(message: read String.from_int(value: flattened[1]))
    Log.write(message: read String.from_int(value: flattened[3]))

    let parts = List.partition<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[0])))
    Log.write(message: read String.from_int(value: parts[0][0]))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[1])))
    Log.write(message: read String.from_int(value: parts[1][2]))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-list-closure.rss",
        "rsscript_parity_list_closure",
        source,
    );
}

#[test]
fn parity_map_mutating_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut table = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut table, key: read "one", value: read 1)
    Map.insert<String, Int>(map: mut table, key: read "two", value: read 2)
    match Map.insert_old<String, Int>(map: mut table, key: read "one", value: read 10) {
        Some(old) => {
            Log.write(message: read String.from_int(value: old))
        }
        None => {
            Log.write(message: read "insert-none")
        }
    }
    match Map.get<String, Int>(map: read table, key: read "one") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    match Map.remove<String, Int>(map: mut table, key: read "two") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    Map.clear<String, Int>(map: mut table)
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-map-mutating.rss",
        "rsscript_parity_map_mutating",
        source,
    );
}

#[test]
fn parity_set_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut set = Set<String>.new()
    if Set.is_empty<String>(set: read set) {
        Log.write(message: read "empty")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Log.write(message: read "insert-a")
    }
    if Set.insert<String>(set: mut set, value: read "b") {
        Log.write(message: read "insert-b")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Log.write(message: read "duplicate")
    } else {
        Log.write(message: read "duplicate-no")
    }
    if Set.contains<String>(set: read set, value: read "b") {
        Log.write(message: read "has-b")
    }
    Log.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    if Set.remove<String>(set: mut set, value: read "a") {
        Log.write(message: read "removed-a")
    }
    if Set.remove<String>(set: mut set, value: read "z") {
        Log.write(message: read "removed-z")
    } else {
        Log.write(message: read "removed-z-no")
    }

    let mut right = Set<String>.new()
    Set.insert<String>(set: mut right, value: read "b")
    Set.insert<String>(set: mut right, value: read "c")
    let union = Set.union<String>(left: read set, right: read right)
    let intersection = Set.intersection<String>(left: read set, right: read right)
    let difference = Set.difference<String>(left: read right, right: read set)
    if Set.contains<String>(set: read union, value: read "c") {
        Log.write(message: read "union-c")
    }
    Log.write(message: read String.from_int(value: Set.len<String>(set: read intersection)))
    if Set.contains<String>(set: read difference, value: read "c") {
        Log.write(message: read "diff-c")
    }
    if Set.is_subset<String>(left: read intersection, right: read union) {
        Log.write(message: read "subset")
    }
    Log.write(message: read String.from_int(value: List.len<String>(list: read Set.to_list<String>(set: read union))))
    Set.clear<String>(set: mut set)
    Log.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-set.rss", "rsscript_parity_set", source);
}

#[test]
fn parity_sorted_set_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut set = SortedSet<Int>.new()
    if SortedSet.is_empty<Int>(set: read set) {
        Log.write(message: read "empty")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 3) {
        Log.write(message: read "insert-3")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 1) {
        Log.write(message: read "insert-1")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Log.write(message: read "insert-2")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Log.write(message: read "duplicate")
    } else {
        Log.write(message: read "duplicate-no")
    }
    if SortedSet.contains<Int>(set: read set, value: read 1) {
        Log.write(message: read "has-1")
    }
    Log.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    let values = SortedSet.to_list<Int>(set: read set)
    Log.write(message: read String.from_int(value: values[0]))
    Log.write(message: read String.from_int(value: values[1]))
    Log.write(message: read String.from_int(value: values[2]))
    if SortedSet.remove<Int>(set: mut set, value: read 2) {
        Log.write(message: read "removed-2")
    }
    if SortedSet.remove<Int>(set: mut set, value: read 9) {
        Log.write(message: read "removed-9")
    } else {
        Log.write(message: read "removed-9-no")
    }
    SortedSet.clear<Int>(set: mut set)
    Log.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-sorted-set.rss",
        "rsscript_parity_sorted_set",
        source,
    );
}

#[test]
fn parity_sorted_map_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut map = SortedMap<Int, String>.new()
    if SortedMap.is_empty<Int, String>(map: read map) {
        Log.write(message: read "empty")
    }
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "two")
    SortedMap.insert<Int, String>(map: mut map, key: read 1, value: read "one")
    SortedMap.insert<Int, String>(map: mut map, key: read 3, value: read "three")
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "TWO")
    Log.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    if SortedMap.contains_key<Int, String>(map: read map, key: read 2) {
        Log.write(message: read "has-2")
    }
    match SortedMap.get<Int, String>(map: read map, key: read 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    let keys = SortedMap.keys<Int, String>(map: read map)
    Log.write(message: read String.from_int(value: keys[0]))
    Log.write(message: read String.from_int(value: keys[1]))
    Log.write(message: read String.from_int(value: keys[2]))
    let values = SortedMap.values<Int, String>(map: read map)
    Log.write(message: read values[0])
    Log.write(message: read values[1])
    Log.write(message: read values[2])
    match SortedMap.remove<Int, String>(map: mut map, key: read 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    match SortedMap.remove<Int, String>(map: mut map, key: read 9) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    SortedMap.clear<Int, String>(map: mut map)
    Log.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-sorted-map.rss",
        "rsscript_parity_sorted_map",
        source,
    );
}

#[test]
fn parity_deque_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local deque = Deque<Int>.new()
    if Deque.is_empty<Int>(deque: read deque) {
        Log.write(message: read "empty")
    }
    Deque.push_back<Int>(deque: mut deque, value: read 2)
    Deque.push_front<Int>(deque: mut deque, value: read 1)
    Deque.push_back<Int>(deque: mut deque, value: read 3)
    Log.write(message: read String.from_int(value: Deque.len<Int>(deque: read deque)))
    let values = Deque.to_list<Int>(deque: read deque)
    Log.write(message: read String.from_int(value: values[0]))
    Log.write(message: read String.from_int(value: values[1]))
    Log.write(message: read String.from_int(value: values[2]))
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    match Deque.pop_back<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "back-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    Deque.push_back<Int>(deque: mut deque, value: read 4)
    Deque.clear<Int>(deque: mut deque)
    if Deque.is_empty<Int>(deque: read deque) {
        Log.write(message: read "cleared")
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-deque.rss", "rsscript_parity_deque", source);
}

#[test]
fn parity_duration_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let short = Duration.ms(value: 750)
    let long = Duration.seconds(value: 2)
    let total = Duration.add(left: read short, right: read long)
    Log.write(message: read String.from_int(value: Duration.as_ms(value: read total)))
    Log.write(message: read String.from_int(value: Duration.as_seconds(value: read total)))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-duration.rss", "rsscript_parity_duration", source);
}

#[test]
fn parity_deadline_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let immediate = Deadline.after_ms(ms: 0)
    if Deadline.is_expired(deadline: read immediate) {
        Log.write(message: read "expired-now")
    }
    Log.write(message: read String.from_int(value: Deadline.remaining_ms(deadline: read immediate)))

    let negative = Deadline.after(duration: read Duration.ms(value: 0 - 1))
    if Deadline.is_expired(deadline: read negative) {
        Log.write(message: read "expired-negative")
    }
    Log.write(message: read String.from_int(value: Deadline.remaining_ms(deadline: read negative)))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-deadline.rss", "rsscript_parity_deadline", source);
}

#[test]
fn parity_counter_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut counter = Counter.new(value: 4)
    Counter.add(counter: mut counter, amount: 6)
    Log.write(message: read String.from_int(value: Counter.value(counter: read counter)))
    Counter.add(counter: mut counter, amount: 0 - 3)
    Log.write(message: read String.from_int(value: Counter.value(counter: read counter)))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-counter.rss", "rsscript_parity_counter", source);
}

#[test]
fn parity_cache_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut cache = Cache.new()
    Log.write(message: read Cache.lookup(cache: read cache, key: read "missing"))
    Cache.insert(cache: mut cache, key: read "one", value: read "first")
    Cache.insert(cache: mut cache, key: read "one", value: read "second")
    Cache.insert(cache: mut cache, key: read "two", value: read "other")
    Log.write(message: read Cache.lookup(cache: read cache, key: read "one"))
    Log.write(message: read Cache.lookup(cache: read cache, key: read "two"))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-cache.rss", "rsscript_parity_cache", source);
}

#[test]
fn parity_channel_sync_intrinsics() {
    let source = r#"
features: async, native, local

fn main() -> Result<Unit, ChannelError> {
    match Channel.bounded<Int>(capacity: 0) {
        Ok(channel) => {
            let sender: Sender<Int> = Channel.sender<Int>(channel: read channel)
            let _ = sender
            Log.write(message: read "unexpected-channel")
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }

    let mut channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut sender: Sender<Int> = Channel.sender<Int>(channel: read channel)
    Sender.close<Int>(sender: mut sender)
    Log.write(message: read "sender-closed")
    let mut receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut channel)?
    Receiver.close<Int>(receiver: mut receiver)
    Log.write(message: read "receiver-closed")
    match Channel.receiver<Int>(channel: mut channel) {
        Ok(receiver) => {
            let _ = receiver
            Log.write(message: read "unexpected-receiver")
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }

    local items = List<String>.new()
    List.push<String>(list: mut items, value: read "one")
    List.push<String>(list: mut items, value: read "two")
    let stream: Stream<String> = Stream.from_list<String>(items: take items)
    let collected = Stream.collect_list<String>(stream: read stream)?
    Log.write(message: read List.join<String>(list: read collected, separator: read ","))

    let mut empty_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut empty_sender: Sender<Int> = Channel.sender<Int>(channel: read empty_channel)
    Sender.close<Int>(sender: mut empty_sender)
    local empty_receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut empty_channel)?
    let empty_stream: Stream<Int> = Receiver.into_stream<Int>(receiver: take empty_receiver)
    let empty_items = Stream.collect_list<Int>(stream: read empty_stream)?
    Log.write(message: read String.from_int(value: List.len<Int>(list: read empty_items)))
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-channel-sync.rss",
        "rsscript_parity_channel_sync",
        source,
        &[],
        &[],
    );
}

#[test]
fn parity_image_cache_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let cache = ImageCache.new(capacity: 2)
    Log.write(message: read String.from_int(value: ImageCache.len(cache: read cache)))
    let zero = ImageCache.new(capacity: 0 - 1)
    Log.write(message: read String.from_int(value: ImageCache.len(cache: read zero)))
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-image-cache.rss",
        "rsscript_parity_image_cache",
        source,
    );
}

#[test]
fn parity_image_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-image-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-image-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter image dir should be created");
    fs::create_dir_all(&backend_root).expect("backend image dir should be created");

    for root in [&interpreter_root, &backend_root] {
        fs::write(root.join("input.img"), b"pixels").expect("image fixture should write");
    }

    let interpreter_input = interpreter_root.join("input.img").display().to_string();
    let interpreter_output = interpreter_root.join("output.img").display().to_string();
    let backend_input = backend_root.join("input.img").display().to_string();
    let backend_output = backend_root.join("output.img").display().to_string();
    let interpreter_args = [interpreter_input.as_str(), interpreter_output.as_str()];
    let backend_args = [backend_input.as_str(), backend_output.as_str()];

    let source = r#"
features: native, local

fn main() -> Result<Unit, ImageError> {
    let input = Path.from_string(value: read Args.get_or_default(index: 0, default: read "input.img"))
    let output = Path.from_string(value: read Args.get_or_default(index: 1, default: read "output.img"))
    local image = Image.load(path: read input)?
    Image.inspect(image: read image)
    Image.resize(image: mut image, width: 10, height: 20)
    Image.normalize(image: mut image)
    Image.sharpen(image: mut image)
    Image.inspect(image: read image)

    local image_cache = ImageCache.new(capacity: 1)
    let shared = manage image
    ImageCache.store(cache: mut image_cache, image: read shared)
    Log.write(message: read String.from_int(value: ImageCache.len(cache: read image_cache)))
    Image.save(image: read shared, path: read output)?

    local text_cache = Cache.new()
    Cache.insert(cache: mut text_cache, key: read "image", value: read "cached-image")
    let cached = Cache.get(cache: read text_cache)
    Image.inspect(image: read cached)
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend_with_distinct_args(
        "parity-image.rss",
        "rsscript_parity_image",
        source,
        &interpreter_args,
        &backend_args,
    );

    let interpreter_bytes =
        fs::read(interpreter_root.join("output.img")).expect("interpreter image output exists");
    let backend_bytes =
        fs::read(backend_root.join("output.img")).expect("backend image output exists");
    assert_eq!(interpreter_bytes, backend_bytes);

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_ord_compare_intrinsic() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.from_int(value: Ord.compare<Int>(self: read 1, other: read 2)))
    Log.write(message: read String.from_int(value: Ord.compare<Int>(self: read 2, other: read 2)))
    Log.write(message: read String.from_int(value: Ord.compare<String>(self: read "z", other: read "a")))
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-ord.rss", "rsscript_parity_ord", source);
}

#[test]
fn parity_clock_and_instant_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let unix = Clock.system_unix_ms()
    if unix > 0 {
        Log.write(message: read "unix-positive")
    }
    let start = Clock.now()
    let elapsed = Instant.elapsed(start: read start)
    let elapsed_ms = Duration.as_ms(value: read elapsed)
    Assert.equal_int(left: elapsed_ms, right: elapsed_ms)
    Log.write(message: read "elapsed-ok")
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-clock.rss", "rsscript_parity_clock", source);
}

#[test]
fn parity_json_builder_and_array_intrinsics() {
    run_with_large_stack(|| {
        let source = r#"
fn main() -> Result<Unit, JsonError> {
    let mut fields = List<String>.new()
    List.push<String>(list: mut fields, value: read Json.string_field(name: read "name", value: read "rss"))
    List.push<String>(list: mut fields, value: read Json.int_field(name: read "count", value: 2))
    List.push<String>(list: mut fields, value: read Json.bool_field(name: read "ok", value: true))
    List.push<String>(list: mut fields, value: read Json.raw_field(name: read "items", value: read Json.array(items: read ["1", "2"])))
    let object_text = Json.object(fields: read fields)
    Log.write(message: read object_text)
    let string_array_text = Json.string_array(items: read ["rss", "script"])
    Log.write(message: read string_array_text)

    let strings_json = Json.strings(items: read ["profile", "project", "other"])
    if Json.array_contains_string(value: read strings_json, item: read "profile")? {
        Log.write(message: read "has-profile")
    }
    if Json.array_contains_substring(value: read strings_json, text: read "roj")? {
        Log.write(message: read "has-substring")
    }
    if Json.array_contains_prefix(value: read strings_json, prefix: read "pro")? {
        Log.write(message: read "has-prefix")
    }
    let strings = Json.array_strings(value: read strings_json)?
    Log.write(message: read List.join<String>(list: read strings, separator: read "|"))

    let ints_json = Json.parse(text: read "[1,2,3]")?
    let ints = Json.array_ints(value: read ints_json)?
    Log.write(message: read String.from_int(value: ints[2]))
    let bools_json = Json.parse(text: read "[true,false]")?
    let bools = Json.array_bools(value: read bools_json)?
    if bools[0] {
        Log.write(message: read "bool-true")
    }

    let values = Json.values(items: read [Json.value(value: read {"n": 1}), Json.clone(value: read Json.value(value: read {"n": 2}))])
    Log.write(message: read Json.to_string(value: read values))
    let bad_strings_json = Json.parse(text: read "[1]")?
    match Json.array_strings(value: read bad_strings_json) {
        Ok(items) => {
            Log.write(message: read List.join<String>(list: read items, separator: read ","))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
        assert_interpreter_matches_backend(
            "parity-json-builder-array.rss",
            "rsscript_parity_json_builder_array",
            source,
        );
    });
}

#[test]
fn parity_json_path_intrinsics() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let text = "{\"profile\":{\"name\":\"rss\",\"age\":7,\"active\":true,\"nested\":{\"value\":\"ok\"}},\"items\":[{\"id\":1},{\"id\":2}],\"missing\":null}"
    let doc = Json.parse(text: read text)?

    let profile_name = Json.at_string(value: read doc, path: read "$.profile.name")?
    Log.write(message: read profile_name)
    Log.write(message: read String.from_int(value: Json.at_int(value: read doc, path: read "profile.age")?))
    if Json.at_bool(value: read doc, path: read "profile.active")? {
        Log.write(message: read "active")
    }
    let item_text = Json.at_to_string(value: read doc, path: read "items[1]")?
    Log.write(message: read item_text)
    let nested = Json.at(value: read doc, path: read "profile.nested")?
    Log.write(message: read Json.to_string(value: read nested))
    let first_id = Json.value_at(value: read doc, path: read "items[0].id")?
    Log.write(message: read Json.to_string(value: read first_id))
    let fallback_value = Json.value(value: read {"fallback": true})
    let missing_value = Json.at_or(value: read doc, path: read "profile.none", fallback: read fallback_value)
    Log.write(message: read Json.to_string(value: read missing_value))

    match Json.at_optional_string(value: read doc, path: read "missing")? {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing-none")
        }
    }
    match Json.at_optional_int(value: read doc, path: read "profile.age")? {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "age-none")
        }
    }
    match Json.at_optional_bool(value: read doc, path: read "profile.active")? {
        Some(value) => {
            if value {
                Log.write(message: read "active-some")
            }
        }
        None => {
            Log.write(message: read "active-none")
        }
    }
    match Json.at_optional(value: read doc, path: read "profile.unknown")? {
        Some(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        None => {
            Log.write(message: read "unknown-none")
        }
    }

    Log.write(message: read Json.at_string_or(value: read doc, path: read "profile.unknown", fallback: read "fallback-name"))
    Log.write(message: read String.from_int(value: Json.at_int_or(value: read doc, path: read "profile.unknown", fallback: 99)))
    if Json.at_bool_or(value: read doc, path: read "profile.unknown", fallback: true) {
        Log.write(message: read "fallback-bool")
    }
    Log.write(message: read Json.at_to_string_or(value: read doc, path: read "profile.unknown", fallback: read "{\"fallback\":true}"))

    let text_name = Json.string_at(text: read text, path: read "profile.name")?
    Log.write(message: read text_name)
    Log.write(message: read String.from_int(value: Json.int_at(text: read text, path: read "items[1].id")?))
    if Json.bool_at(text: read text, path: read "profile.active")? {
        Log.write(message: read "text-bool")
    }
    let nested_text = Json.to_string_at(text: read text, path: read "profile.nested")?
    Log.write(message: read nested_text)
    Log.write(message: read Json.string_at_or(text: read text, path: read "profile.none", fallback: read "string-fallback"))
    Log.write(message: read Json.json_string_at_or(text: read text, path: read "profile.none", fallback: read "json-string-fallback"))
    Log.write(message: read String.from_int(value: Json.int_at_or(text: read text, path: read "profile.none", fallback: 123)))
    Log.write(message: read String.from_int(value: Json.json_int_at_or(text: read text, path: read "profile.none", fallback: 124)))
    if Json.bool_at_or(text: read text, path: read "profile.none", fallback: true) {
        Log.write(message: read "bool-fallback")
    }
    if Json.json_bool_at_or(text: read text, path: read "profile.none", fallback: true) {
        Log.write(message: read "json-bool-fallback")
    }
    Log.write(message: read Json.to_string_at_or(text: read text, path: read "profile.none", fallback: read "string-json-fallback"))
    return Ok(Unit)
}
"#;
    assert_interpreter_matches_backend("parity-json-path.rss", "rsscript_parity_json_path", source);
}

#[test]
fn parity_json_result_intrinsics() {
    let source = r#"
fn main() -> Unit {
    match Json.parse(text: read "{\"count\":3,\"ok\":true,\"items\":[1,2,3]}") {
        Ok(doc) => {
            match Json.field_int(value: read doc, name: read "count") {
                Ok(count) => {
                    Log.write(message: read String.from_int(value: count))
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field_optional_bool(value: read doc, name: read "ok") {
                Ok(Some(flag)) => {
                    if flag {
                        Log.write(message: read "flag-true")
                    } else {
                        Log.write(message: read "flag-false")
                    }
                }
                Ok(None) => {
                    Log.write(message: read "flag-none")
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field_optional_string(value: read doc, name: read "missing") {
                Ok(Some(text)) => {
                    Log.write(message: read text)
                }
                Ok(None) => {
                    Log.write(message: read "missing-none")
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field(value: read doc, name: read "items") {
                Ok(items) => {
                    match Json.array_len(value: read items) {
                        Ok(len) => {
                            Log.write(message: read String.from_int(value: len))
                        }
                        Err(error) => {
                            Log.write(message: read JsonError.message(error: read error))
                        }
                    }
                    match Json.array_get(value: read items, index: 1) {
                        Ok(item) => {
                            match Json.as_int(value: read item) {
                                Ok(n) => {
                                    Log.write(message: read String.from_int(value: n))
                                }
                                Err(error) => {
                                    Log.write(message: read JsonError.message(error: read error))
                                }
                            }
                        }
                        Err(error) => {
                            Log.write(message: read JsonError.message(error: read error))
                        }
                    }
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.as_int(value: read Json.value(value: read {"text": "nope"})) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend(
        "parity-json-result.rss",
        "rsscript_parity_json_result",
        source,
    );
}

#[test]
fn parity_json_toml_yaml_parse_file_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-parse-files-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-parse-files-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter parse dir should be created");
    fs::create_dir_all(&backend_root).expect("backend parse dir should be created");

    for root in [&interpreter_root, &backend_root] {
        fs::write(root.join("data.json"), r#"{"name":"rss","count":7}"#)
            .expect("json fixture should write");
        fs::write(root.join("data.toml"), "name = \"rss\"\ncount = 8\n")
            .expect("toml fixture should write");
        fs::write(root.join("data.yaml"), "name: rss\ncount: 9\n")
            .expect("yaml fixture should write");
    }

    let interpreter_args = [
        interpreter_root.join("data.json").display().to_string(),
        interpreter_root.join("data.toml").display().to_string(),
        interpreter_root.join("data.yaml").display().to_string(),
    ];
    let backend_args = [
        backend_root.join("data.json").display().to_string(),
        backend_root.join("data.toml").display().to_string(),
        backend_root.join("data.yaml").display().to_string(),
    ];
    let interpreter_arg_refs = interpreter_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let backend_arg_refs = backend_args.iter().map(String::as_str).collect::<Vec<_>>();

    let source = r#"
features: native

fn main() -> Result<Unit, JsonError> {
    let json_path = Path.from_string(value: read Args.get_or_default(index: 0, default: read "data.json"))
    let toml_path = Path.from_string(value: read Args.get_or_default(index: 1, default: read "data.toml"))
    let yaml_path = Path.from_string(value: read Args.get_or_default(index: 2, default: read "data.yaml"))

    let json_doc = Json.parse_file(path: read json_path)?
    let json_name = Json.field_string(value: read json_doc, name: read "name")?
    let json_count = Json.field_int(value: read json_doc, name: read "count")?
    Log.write(message: read json_name)
    Log.write(message: read String.from_int(value: json_count))

    let toml_doc = Toml.parse_file(path: read toml_path)?
    let toml_name = Json.field_string(value: read toml_doc, name: read "name")?
    let toml_count = Json.field_int(value: read toml_doc, name: read "count")?
    Log.write(message: read toml_name)
    Log.write(message: read String.from_int(value: toml_count))

    let yaml_text_doc = Yaml.parse(text: read "name: rss\ncount: 10\n")?
    let yaml_text_count = Json.field_int(value: read yaml_text_doc, name: read "count")?
    Log.write(message: read String.from_int(value: yaml_text_count))
    let yaml_doc = Yaml.parse_file(path: read yaml_path)?
    let yaml_name = Json.field_string(value: read yaml_doc, name: read "name")?
    let yaml_count = Json.field_int(value: read yaml_doc, name: read "count")?
    Log.write(message: read yaml_name)
    Log.write(message: read String.from_int(value: yaml_count))
    return Ok(Unit)
}
"#;

    assert_interpreter_matches_backend_with_distinct_args(
        "parity-parse-files.rss",
        "rsscript_parity_parse_files",
        source,
        &interpreter_arg_refs,
        &backend_arg_refs,
    );

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_with_statement_and_try_expression() {
    let source = r#"
resource Box {
    value: Int
}

fn touch(item: mut Box) -> Unit {
    Log.write(message: read String.from_int(value: item.value))
    return Unit
}

fn checked(value: Int) -> Result<Int, String> {
    if value < 0 {
        return Err("negative")
    }
    return Ok(value + 1)
}

fn print_checked(value: Int) -> Result<Unit, String> {
    let next = checked(value: value)?
    Log.write(message: read String.from_int(value: next))
    return Ok(Unit)
}

fn main() -> Unit {
    with Box(value: 7) as handle {
        touch(item: mut handle)
    }
    match print_checked(value: 10) {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    match print_checked(value: 0 - 1) {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    return Unit
}
"#;
    assert_interpreter_matches_backend("parity-with-try.rss", "rsscript_parity_with_try", source);
}
