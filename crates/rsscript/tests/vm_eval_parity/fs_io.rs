//! eval≡lowered parity: filesystem and process IO
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_path_intrinsics() {
    common::run_with_large_stack(|| {
        let source = r#"
fn main() -> Unit {
    let root = Path.from_string(value: read "fixtures")
    let path = Path.join(base: read root, child: read "rsscript-path.txt")
    Output.write(message: read Path.to_string(path: read path))
    Output.write(message: read Path.to_string(path: read "fixtures/rsscript-path.txt".to_path()))
    Output.write(message: read Path.to_string(path: read Path.normalize(path: read Path.join(base: read path, child: read ".."))))

    match Path.file_name(path: read path) {
        Some(name) => {
            Output.write(message: read name)
        }
        None => {
            Output.write(message: read "no-name")
        }
    }
    match Path.extension(path: read path) {
        Some(extension) => {
            Output.write(message: read extension)
        }
        None => {
            Output.write(message: read "no-extension")
        }
    }
    match Path.parent(path: read path) {
        Some(parent) => {
            Output.write(message: read Path.to_string(path: read parent))
        }
        None => {
            Output.write(message: read "no-parent")
        }
    }

    if Path.is_absolute(path: read Path.from_string(value: read "/tmp/rsscript")) {
        Output.write(message: read "absolute")
    }
    if Path.starts_with(path: read path, base: read root) {
        Output.write(message: read "starts")
    }
    Output.write(message: read Path.to_string(path: read Path.with_extension(path: read path, extension: read "json")))

    match Path.safe_relative(value: read "fixtures/./rsscript-path.txt") {
        Ok(safe) => {
            Output.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match "../escape".safe_relative() {
        Ok(safe) => {
            Output.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match Path.resolve_relative(root: read root, relative: read "rsscript-path.txt") {
        Ok(resolved) => {
            Output.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match Workspace.resolve(root: read root, relative: read "nested/../bad") {
        Ok(resolved) => {
            Output.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    return Unit
}
"#;
        common::assert_vm_eval_matches_backend("parity-path.rss", "rsscript_parity_path", source);
    });
}

#[test]
fn parity_path_file_directory_intrinsics() {
    common::run_with_large_stack(|| {
        let interpreter_root = common::unique_temp_dir("rsscript-parity-fs-interpreter");
        let backend_root = common::unique_temp_dir("rsscript-parity-fs-backend");
        let interpreter_root_arg = interpreter_root.display().to_string();
        let backend_root_arg = backend_root.display().to_string();
        let source = r#"

fn main() -> Result<Unit, FileError> {
    let root = Args.get_or_default(index: 0, default: read "target/rsscript-parity-fs")
    let root_path = Path.from_string(value: read root)
    let nested = Path.join(base: read root_path, child: read "nested")
    let single = Path.join(base: read root_path, child: read "single")
    Directory.create_all(path: read nested)?
    Directory.create_dir_all(path: read nested)?
    Directory.create(path: read single)?
    if Directory.exists(path: read root_path) {
        Output.write(message: read "root-exists")
    }
    if Directory.is_dir(path: read nested) {
        Output.write(message: read "nested-dir")
    }
    if Path.exists(path: read single) {
        Output.write(message: read "single-exists")
    }
    if Path.is_dir(path: read single) {
        Output.write(message: read "single-dir")
    }

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Path.write_string(path: read path_file, text: read "path text")?
    if Path.is_file(path: read path_file) {
        Output.write(message: read "path-file")
    }
    if File.exists(path: read path_file) {
        Output.write(message: read "file-exists")
    }
    if Directory.is_file(path: read path_file) {
        Output.write(message: read "directory-sees-file")
    }
    let path_text = Path.read_string(path: read path_file)?
    Output.write(message: read path_text)
    let path_digest = Hash.sha256_file(path: read path_file)?
    Assert.equal(left: read path_digest, right: read "c6465e0abd2e3c2f5ccfe7f639ddc0f72282904663b09ddd8dffbe060be35f97")

    File.write_string_to_path(path: read path_file, text: read "file text")?
    let file_text = File.read_string(path: read path_file)?
    Output.write(message: read file_text)

    let bytes_file = Path.join(base: read nested, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    File.append_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "de"))?
    File.append_string(path: read bytes_file, text: read "f")?
    let bytes = File.read_bytes(path: read bytes_file)?
    Output.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    match File.bytes_stream(path: read bytes_file, chunk_size: 2) {
        Ok(stream) => {
            match Stream.collect_list<Bytes>(stream: read stream) {
                Ok(chunks) => {
                    Output.write(message: read String.from_int(value: List.len<Bytes>(list: read chunks)))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[0])))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[1])))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[2])))
                }
                Err(error) => {
                    Output.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }

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
        Output.write(message: read String.from_int(value: Bytes.len(value: read all)))
        let empty = File.read_all(file: mut reader)?
        Output.write(message: read String.from_int(value: Bytes.len(value: read empty)))
    }
    with File.open_read(path: read handle_file)? as reader_text {
        let text_all = File.read_all_string(file: mut reader_text)?
        Output.write(message: read text_all)
    }
    with File.open_read(path: read handle_file)? as reader_into {
        local into_buffer = Buffer.new(size: 0)
        if File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Output.write(message: read String.from_int(value: Buffer.len(buffer: read into_buffer)))
        }
        if !File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Output.write(message: read "read-into-empty")
        }
    }

    Directory.write_string(path: read path_file, content: read "directory text")?
    let directory_text = Directory.read_string(path: read path_file)?
    Output.write(message: read directory_text)
    let metadata = Directory.metadata(path: read path_file)?
    if metadata.is_file {
        Output.write(message: read "metadata-file")
    }
    Output.write(message: read String.from_int(value: metadata.len))

    let files = Directory.list_files(path: read root_path)?
    Output.write(message: read List.join<String>(list: read files, separator: read "|"))
    let files_from_path = Path.list_files(path: read root_path)?
    Output.write(message: read List.join<String>(list: read files_from_path, separator: read "|"))
    let paths = Directory.list_paths(path: read nested)?
    Output.write(message: read String.from_int(value: List.len<Path>(list: read paths)))
    match Path.file_name(path: read paths[0]) {
        Some(name) => {
            Output.write(message: read name)
        }
        None => {
            Output.write(message: read "path-name-none")
        }
    }
    let paths_from_path = Path.list_paths(path: read nested)?
    Output.write(message: read String.from_int(value: List.len<Path>(list: read paths_from_path)))

    match File.read_string(path: read Path.from_string(value: read "__rsscript_missing_file_for_parity__")) {
        Ok(value) => {
            Output.write(message: read value)
        }
        Err(error) => {
            let message = FileError.message(error: read error)
            if String.contains(value: read message, needle: read "No such file") {
                Output.write(message: read "missing-file-error")
            } else {
                Output.write(message: read "other-file-error")
            }
        }
    }
    let copied = Path.join(base: read nested, child: read "copied.txt")
    Directory.copy_file(from: read path_file, to: read copied)?
    let copied_text = File.read_string(path: read copied)?
    Output.write(message: read copied_text)
    let renamed = Path.join(base: read nested, child: read "renamed.txt")
    Directory.rename(from: read copied, to: read renamed)?
    if File.exists(path: read renamed) {
        Output.write(message: read "renamed-exists")
    }
    let atomic = Path.join(base: read nested, child: read "atomic.txt")
    File.write_atomic(path: read atomic, text: read "atomic text")?
    let atomic_text = File.read_string(path: read atomic)?
    Output.write(message: read atomic_text)
    File.remove(path: read atomic)?
    if !File.exists(path: read atomic) {
        Output.write(message: read "atomic-removed")
    }
    Directory.remove_file(path: read renamed)?
    if !File.exists(path: read renamed) {
        Output.write(message: read "renamed-removed")
    }
    Directory.remove_dir_all(path: read single)?
    if !Path.exists(path: read single) {
        Output.write(message: read "single-removed")
    }
    return Ok(Unit)
}
"#;
        common::assert_vm_eval_matches_backend_with_distinct_args(
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

fn main() -> Result<Unit, CsvError> {
    let path = Path.from_string(value: read Args.get_or_default(index: 0, default: read "data.csv"))
    local buffer = RowBuffer.new(size: 4096)

    with Csv.open_read(path: read path)? as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
    }

    let row = Csv.parse_row(buffer: read buffer)?
    let name = Row.field_string(row: read row, index: 0)?
    let amount = Row.field_string(row: read row, index: 1)?
    Output.write(message: read name)
    Output.write(message: read amount)
    match Csv.rows(path: read path, buffer_size: 16) {
        Ok(stream) => {
            match Stream.collect_list<Row>(stream: read stream) {
                Ok(rows) => {
                    Output.write(message: read String.from_int(value: List.len<Row>(list: read rows)))
                    let first_stream_name = Row.field_string(row: read rows[0], index: 0)?
                    let second_stream_amount = Row.field_string(row: read rows[1], index: 1)?
                    Output.write(message: read first_stream_name)
                    Output.write(message: read second_stream_amount)
                }
                Err(error) => {
                    Output.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args(
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

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rsscript-parity-tempdir"))
    with TempDir.new_in(parent: read root)? as child {
        let path = TempDir.path(dir: read child)
        if Path.is_dir(path: read path) {
            Output.write(message: read "new-in-dir")
        }
        Directory.remove_dir_all(path: read path)?
    }
    with TempDir.new()? as created {
        let path = TempDir.path(dir: read created)
        if Path.is_dir(path: read path) {
            Output.write(message: read "new-dir")
        }
        Directory.remove_dir_all(path: read path)?
    }
    with TempDir.new_in(parent: read root)? as kept {
        let path = TempDir.keep(dir: take kept)
        if Path.is_dir(path: read path) {
            Output.write(message: read "kept-dir")
        }
        Directory.remove_dir_all(path: read path)?
    }
    return Ok(Unit)
}
"#;

    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
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
fn parity_process_run_intrinsics() {
    let source = r#"

fn main() -> Result<Unit, String> {
    let mut args = List<String>.new()
    List.push<String>(list: mut args, value: read "hello")
    let stdout = Process.run_stdout(command: read "printf", args: read args)?
    Output.write(message: read stdout)

    let mut run_args = List<String>.new()
    List.push<String>(list: mut run_args, value: read "world")
    let output = Process.run(command: read "printf", args: read run_args)?
    Output.write(message: read String.from_int(value: output.status))
    Output.write(message: read output.stdout)
    Output.write(message: read output.stderr)
    Output.write(message: read output.merged)
    if output.truncated == false {
        Output.write(message: read "not-truncated")
    }

    let mut timeout_args = List<String>.new()
    List.push<String>(list: mut timeout_args, value: read "timeout")
    let timeout_stdout = Process.run_stdout_timeout(command: read "printf", args: read timeout_args, timeout_ms: 1000)?
    Output.write(message: read timeout_stdout)

    let mut run_timeout_args = List<String>.new()
    List.push<String>(list: mut run_timeout_args, value: read "done")
    let timed = Process.run_timeout(command: read "printf", args: read run_timeout_args, timeout_ms: 1000)?
    Output.write(message: read String.from_int(value: timed.status))
    Output.write(message: read timed.stdout)
    Output.write(message: read timed.merged)

    let mut format_args = List<String>.new()
    List.push<String>(list: mut format_args, value: read "%s")
    let items = ["A", "B"]
    let many = Process.run_many_stdout(command: read "printf", args: read format_args, appended_args: read items, jobs: 2)?
    Output.write(message: read List.join<String>(list: read many, separator: read "|"))

    let many_timeout = Process.run_many_stdout_timeout(command: read "printf", args: read format_args, appended_args: read items, jobs: 2, timeout_ms: 1000)?
    Output.write(message: read List.join<String>(list: read many_timeout, separator: read "|"))

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
    Output.write(message: read String.from_int(value: stdin_output.status))
    Output.write(message: read stdin_output.stdout)
    Output.write(message: read stdin_output.merged)

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
    Output.write(message: read capped.stdout)
    if capped.truncated {
        Output.write(message: read "request-truncated")
    }

    let large_output_request = ProcessRequest(
        command: "sh",
        args: ["-c", "yes x | head -c 200000"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 3,
    )
    let large_output = Process.run_request(request: read large_output_request)?
    Output.write(message: read large_output.stdout)
    if large_output.truncated {
        Output.write(message: read "large-output-truncated")
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
            Output.write(message: read String.from_int(value: List.len<ProcessEvent>(list: read collected_events)))
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend("parity-process.rss", "rsscript_parity_process", source);
}

#[test]
fn parity_json_path_intrinsics() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let text = "{\"profile\":{\"name\":\"rss\",\"age\":7,\"active\":true,\"nested\":{\"value\":\"ok\"}},\"items\":[{\"id\":1},{\"id\":2}],\"missing\":null}"
    let doc = Json.parse(text: read text)?

    let profile_name = Json.at_string(value: read doc, path: read "$.profile.name")?
    Output.write(message: read profile_name)
    Output.write(message: read String.from_int(value: Json.at_int(value: read doc, path: read "profile.age")?))
    if Json.at_bool(value: read doc, path: read "profile.active")? {
        Output.write(message: read "active")
    }
    let item_text = Json.at_to_string(value: read doc, path: read "items[1]")?
    Output.write(message: read item_text)
    let nested = Json.at(value: read doc, path: read "profile.nested")?
    Output.write(message: read Json.to_string(value: read nested))
    let first_id = Json.value_at(value: read doc, path: read "items[0].id")?
    Output.write(message: read Json.to_string(value: read first_id))
    let fallback_value = Json.value(value: read {"fallback": true})
    let missing_value = Json.at_or(value: read doc, path: read "profile.none", fallback: read fallback_value)
    Output.write(message: read Json.to_string(value: read missing_value))

    match Json.at_optional_string(value: read doc, path: read "missing")? {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing-none")
        }
    }
    match Json.at_optional_int(value: read doc, path: read "profile.age")? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "age-none")
        }
    }
    match Json.at_optional_bool(value: read doc, path: read "profile.active")? {
        Some(value) => {
            if value {
                Output.write(message: read "active-some")
            }
        }
        None => {
            Output.write(message: read "active-none")
        }
    }
    match Json.at_optional(value: read doc, path: read "profile.unknown")? {
        Some(value) => {
            Output.write(message: read Json.to_string(value: read value))
        }
        None => {
            Output.write(message: read "unknown-none")
        }
    }

    Output.write(message: read Json.at_string_or(value: read doc, path: read "profile.unknown", fallback: read "fallback-name"))
    Output.write(message: read String.from_int(value: Json.at_int_or(value: read doc, path: read "profile.unknown", fallback: 99)))
    if Json.at_bool_or(value: read doc, path: read "profile.unknown", fallback: true) {
        Output.write(message: read "fallback-bool")
    }
    Output.write(message: read Json.at_to_string_or(value: read doc, path: read "profile.unknown", fallback: read "{\"fallback\":true}"))

    let text_name = Json.string_at(text: read text, path: read "profile.name")?
    Output.write(message: read text_name)
    Output.write(message: read String.from_int(value: Json.int_at(text: read text, path: read "items[1].id")?))
    if Json.bool_at(text: read text, path: read "profile.active")? {
        Output.write(message: read "text-bool")
    }
    let nested_text = Json.to_string_at(text: read text, path: read "profile.nested")?
    Output.write(message: read nested_text)
    Output.write(message: read Json.string_at_or(text: read text, path: read "profile.none", fallback: read "string-fallback"))
    Output.write(message: read Json.json_string_at_or(text: read text, path: read "profile.none", fallback: read "json-string-fallback"))
    Output.write(message: read String.from_int(value: Json.int_at_or(text: read text, path: read "profile.none", fallback: 123)))
    Output.write(message: read String.from_int(value: Json.json_int_at_or(text: read text, path: read "profile.none", fallback: 124)))
    if Json.bool_at_or(text: read text, path: read "profile.none", fallback: true) {
        Output.write(message: read "bool-fallback")
    }
    if Json.json_bool_at_or(text: read text, path: read "profile.none", fallback: true) {
        Output.write(message: read "json-bool-fallback")
    }
    Output.write(message: read Json.to_string_at_or(text: read text, path: read "profile.none", fallback: read "string-json-fallback"))
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-json-path.rss",
        "rsscript_parity_json_path",
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

fn main() -> Result<Unit, JsonError> {
    let json_path = Path.from_string(value: read Args.get_or_default(index: 0, default: read "data.json"))
    let toml_path = Path.from_string(value: read Args.get_or_default(index: 1, default: read "data.toml"))
    let yaml_path = Path.from_string(value: read Args.get_or_default(index: 2, default: read "data.yaml"))

    let json_doc = Json.parse_file(path: read json_path)?
    let json_name = Json.field_string(value: read json_doc, name: read "name")?
    let json_count = Json.field_int(value: read json_doc, name: read "count")?
    Output.write(message: read json_name)
    Output.write(message: read String.from_int(value: json_count))

    let toml_doc = Toml.parse_file(path: read toml_path)?
    let toml_name = Json.field_string(value: read toml_doc, name: read "name")?
    let toml_count = Json.field_int(value: read toml_doc, name: read "count")?
    Output.write(message: read toml_name)
    Output.write(message: read String.from_int(value: toml_count))

    let yaml_text_doc = Yaml.parse(text: read "name: rss\ncount: 10\n")?
    let yaml_text_count = Json.field_int(value: read yaml_text_doc, name: read "count")?
    Output.write(message: read String.from_int(value: yaml_text_count))
    let yaml_doc = Yaml.parse_file(path: read yaml_path)?
    let yaml_name = Json.field_string(value: read yaml_doc, name: read "name")?
    let yaml_count = Json.field_int(value: read yaml_doc, name: read "count")?
    Output.write(message: read yaml_name)
    Output.write(message: read String.from_int(value: yaml_count))
    return Ok(Unit)
}
"#;

    common::assert_vm_eval_matches_backend_with_distinct_args(
        "parity-parse-files.rss",
        "rsscript_parity_parse_files",
        source,
        &interpreter_arg_refs,
        &backend_arg_refs,
    );

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}
