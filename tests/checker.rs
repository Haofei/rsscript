use std::fs;
use std::path::{Path, PathBuf};

use rsscript::syntax::ast::Item;
use rsscript::syntax::parse_source;
use rsscript::{
    ReviewMapClassification, ReviewMapFileRisk, ReviewRisk, analyze_source,
    analyze_source_with_core, analyze_source_with_interfaces, core_interfaces,
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_json,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    lint_source, lower_source_to_rust, lower_source_to_rust_package, lower_source_to_rust_with_map,
    remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines, review_map_sources,
    review_sources,
};
use serde_json::Value;

#[test]
fn pass_fixtures_have_no_diagnostics() {
    for path in fixture_paths("tests/fixtures/pass") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn fail_fixtures_report_expected_diagnostic_codes() {
    for path in fixture_paths("tests/fixtures/fail") {
        let source = read_fixture(&path);
        let expected = expected_codes(&source);
        let actual: Vec<String> = analyze_source(path.to_str().unwrap(), &source)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect();

        for code in expected {
            assert!(
                actual.contains(&code),
                "{} expected {code}, got {actual:?}",
                path.display()
            );
        }
    }
}

#[test]
fn core_interface_files_have_no_diagnostics() {
    for path in recursive_fixture_paths("core") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn examples_have_no_diagnostics_and_lower_to_runnable_packages() {
    for path in fixture_paths("examples") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source_with_core(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());

        let package = lower_source_to_rust_package(
            path.to_str().unwrap(),
            &source,
            path.file_stem().and_then(|stem| stem.to_str()).unwrap(),
            "/workspace/rsscript/runtime",
        )
        .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", path.display()));

        assert!(
            package.main_rs.is_some(),
            "{} should lower to a runnable Rust package",
            path.display()
        );
    }
}

#[test]
fn bundled_core_interfaces_are_available_to_checker() {
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/test/assert.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/log/log.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/cache/cache.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/collections/buffer.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/collections/list.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/os/os.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/cache/image_cache.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/counter/counter.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/config/rules.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/interpreter/interpreter.rssi")
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
fn diagnostics_json_uses_protocol_shape() {
    let path = Path::new("tests/fixtures/fail/use-after-manage.rss");
    let source = read_fixture(path);
    let diagnostics = analyze_source(path.to_str().unwrap(), &source);
    let json = format_diagnostics_json(&diagnostics);
    let value: Value = serde_json::from_str(&json).expect("diagnostics JSON should parse");
    let first = value
        .as_array()
        .and_then(|items| items.first())
        .expect("expected at least one diagnostic");

    assert_eq!(first["code"], "RS0401");
    assert_eq!(first["severity"], "error");
    assert!(
        first["summary"]
            .as_str()
            .is_some_and(|summary| !summary.is_empty())
    );
    assert!(
        first["spans"]
            .as_array()
            .is_some_and(|spans| !spans.is_empty())
    );
    assert!(first["causes"].is_array());
    assert!(first["fixes"].is_array());
}

#[test]
fn diagnostic_explanations_are_available_by_code() {
    let explanation = explain_diagnostic_code("RS0401").expect("RS0401 should be registered");
    let formatted = format_diagnostic_explanation(explanation);

    assert_eq!(explanation.title, "use after manage");
    assert!(formatted.contains("RS0401"));
    assert!(formatted.contains("manage"));
    assert!(explain_diagnostic_code("RS9999").is_none());
}

#[test]
fn lint_warns_on_public_signature_complexity() {
    let source = r#"
pub fn overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
    effects(no_panic, noalloc, no_block, pure, native)
{
    return Ok(None)
}
"#;
    let diagnostics = lint_source("lint.rss", source);
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RSL001"));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.severity.is_error())
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("7 parameters"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("4 generic parameters"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("5 effects"))
    );
}

#[test]
fn lint_warns_on_duplicate_effects() {
    let source = r#"
fn cache(value: read Image) -> Unit
    effects(no_panic, no_panic, retains(value), retains(value))
{
    return Unit
}
"#;
    let diagnostics = lint_source("lint.rss", source);
    let duplicate_effects = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RSL002")
        .collect::<Vec<_>>();

    assert_eq!(duplicate_effects.len(), 2);
    assert!(
        duplicate_effects
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("no_panic"))
    );
    assert!(
        duplicate_effects
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("retains(value)"))
    );
}

#[test]
fn parser_accepts_qualified_interface_function_signatures() {
    let source = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let program = parse_source("html.rssi", source);

    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "Html.escape")
    );
}

#[test]
fn checker_resolves_calls_from_interface_signatures() {
    let interface = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let source = r#"
fn render(body: read String) -> fresh HtmlEscaped {
    return Html.escape(text: read body)
}
"#;

    assert_eq!(
        analyze_source_with_interfaces("page.rss", source, &[("html.rssi", interface)]),
        Vec::new()
    );
}

#[test]
fn checker_reports_interface_signature_call_violations() {
    let interface = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let source = r#"
fn render(body: read String) -> fresh HtmlEscaped {
    return Html.escape(value: read body)
}
"#;
    let codes = analyze_source_with_interfaces("page.rss", source, &[("html.rssi", interface)])
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0203".to_string()));
    assert!(codes.contains(&"RS0204".to_string()));
}

#[test]
fn rust_lowering_emits_checked_rust_source() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

pub fn make_point(x: Int, y: Int) -> fresh Point {
    return Point(x: x, y: y)
}
"#;
    let rust = lower_source_to_rust("point.rss", source).expect("source should lower");

    assert!(rust.contains("// Generated by RSScript."));
    assert!(rust.contains("// rss:span kind=function file=point.rss"));
    assert!(rust.contains("pub struct Point"));
    assert!(rust.contains("pub x: i64"));
    assert!(rust.contains("pub fn make_point(x: i64, y: i64) -> Point"));
    assert!(rust.contains("return Point { x: x, y: y };"));
}

#[test]
fn rust_lowering_maps_unit_and_result_constructors_to_rust() {
    let source = r#"
struct BuildError {
    code: Int
}

struct Point {
    x: Int
    y: Int
}

pub fn make_result(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Ok(Point(x: x, y: y))
}

pub fn fail(code: Int) -> Result<fresh Point, BuildError> {
    return Err(BuildError(code: code))
}

pub fn unit_result() -> Result<Unit, BuildError> {
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("result.rss", source).expect("source should lower");

    assert!(rust.contains("pub fn make_result(x: i64, y: i64) -> Result<Point, BuildError>"));
    assert!(rust.contains("return Ok(Point { x: x, y: y });"));
    assert!(rust.contains("return Err(BuildError { code: code });"));
    assert!(rust.contains("pub fn unit_result() -> Result<(), BuildError>"));
    assert!(rust.contains("return Ok(());"));
}

#[test]
fn rust_lowering_maps_option_type_and_constructors_to_rust() {
    let source = r#"
pub fn maybe_value(flag: Bool) -> Option<Int> {
    if flag {
        return Some(42)
    }
    return None
}
"#;
    let rust = lower_source_to_rust("option.rss", source).expect("source should lower");

    assert!(rust.contains("pub fn maybe_value(flag: bool) -> Option<i64>"));
    assert!(rust.contains("return Some(42);"));
    assert!(rust.contains("return None;"));
}

#[test]
fn rust_lowering_maps_core_surface_types_to_rust_std_types() {
    let source = r#"
struct CoreError {
    code: Int
}

pub fn inspect_core(
    path: read Path,
    bytes: read Bytes,
    buffer: mut Buffer,
    names: read Set<String>,
    counts: read Map<String, Int>,
    items: read List<Int>,
) -> Result<fresh Bytes, CoreError> {
    return Err(CoreError(code: 0))
}
"#;
    let rust = lower_source_to_rust("core-types.rss", source).expect("source should lower");

    assert!(rust.contains("path: &std::path::PathBuf"));
    assert!(rust.contains("bytes: &Vec<u8>"));
    assert!(rust.contains("buffer: &mut Vec<u8>"));
    assert!(rust.contains("names: &std::collections::HashSet<String>"));
    assert!(rust.contains("counts: &std::collections::HashMap<String, i64>"));
    assert!(rust.contains("items: &Vec<i64>"));
    assert!(rust.contains("-> Result<Vec<u8>, CoreError>"));
}

#[test]
fn rust_lowering_maps_take_consume_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn close_fd() -> Unit {
    OS.close(fd: 0)
}

fn consume_list(list: take List<Int>) -> Unit {
    List.consume(list: take list)
}

fn consume_buffer(buffer: take Buffer) -> Unit {
    Buffer.consume(buffer: take buffer)
}
"#;
    let rust = lower_source_to_rust("consume.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::os_close(0);"));
    assert!(rust.contains("fn consume_list(list: Vec<i64>)"));
    assert!(rust.contains("rsscript_runtime::list_consume(list);"));
    assert!(rust.contains("fn consume_buffer(buffer: Vec<u8>)"));
    assert!(rust.contains("rsscript_runtime::buffer_consume(buffer);"));
}

#[test]
fn rust_lowering_maps_cache_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let cache = Cache.new()
    Cache.insert(cache: mut cache, key: read "/users", value: read "handled /users")
    let body = Cache.lookup(cache: read cache, key: read "/users")
    Log.write(message: read body)
    return Unit
}
"#;
    let rust = lower_source_to_rust("cache.rss", source).expect("source should lower");

    assert!(rust.contains("let mut cache = rsscript_runtime::cache_new();"));
    assert!(rust.contains(
        "rsscript_runtime::cache_insert(&mut cache, &\"/users\".to_string(), &\"handled /users\".to_string());"
    ));
    assert!(
        rust.contains(
            "let body = rsscript_runtime::cache_lookup(&cache, &\"/users\".to_string());"
        )
    );
}

#[test]
fn rust_lowering_maps_file_core_calls_to_runtime_hooks() {
    let source = r#"
fn copy_file(input: read Path, output: read Path) -> Result<Unit, IOError> {
    with File.open_read(path: read input) as reader {
        with File.open_write(path: read output) as writer {
            let bytes = File.read_all(file: mut reader)?
            File.write(file: mut writer, data: read bytes)?
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), std::io::Error>"));
    assert!(rust.contains("let mut reader = rsscript_runtime::file_open_read(&input)?;"));
    assert!(rust.contains("let mut writer = rsscript_runtime::file_open_write(&output)?;"));
    assert!(rust.contains("let bytes = rsscript_runtime::file_read_all(&mut reader)?;"));
    assert!(rust.contains("rsscript_runtime::file_write(&mut writer, &bytes)?;"));
}

#[test]
fn rust_lowering_maps_json_core_calls_to_runtime_hooks() {
    let source = r#"
fn read_name(text: read String) -> Result<String, JsonError> {
    let value = Json.parse(text: read text)?
    let profile = Json.field(value: read value, name: read "profile")?
    return Json.field_string(value: read profile, name: read "name")
}
"#;
    let rust = lower_source_to_rust("json.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::JsonError>"));
    assert!(rust.contains("let value = rsscript_runtime::json_parse(&text)?;"));
    assert!(rust.contains(
        "let profile = rsscript_runtime::json_field(&value, &\"profile\".to_string())?;"
    ));
    assert!(
        rust.contains(
            "return rsscript_runtime::json_field_string(&profile, &\"name\".to_string());"
        )
    );
}

#[test]
fn rust_lowering_maps_csv_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn read_name(path: read Path) -> Result<String, CsvError> {
    local buffer = RowBuffer.new(size: 4096)
    with File.open_read(path: read path) as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
        let row = Csv.parse_row(buffer: read buffer)?
        return Row.field_string(row: read row, index: 0)
    }
}
"#;
    let rust = lower_source_to_rust("csv.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::CsvError>"));
    assert!(rust.contains("let mut buffer = rsscript_runtime::row_buffer_new(4096);"));
    assert!(rust.contains("rsscript_runtime::csv_read_into(&mut file, &mut buffer)?;"));
    assert!(rust.contains("let row = rsscript_runtime::csv_parse_row(&buffer)?;"));
    assert!(rust.contains("return rsscript_runtime::row_field_string(&row, 0);"));
}

#[test]
fn rust_lowering_maps_image_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn process(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?
    Image.resize(image: mut image, width: 320, height: 240)
    Image.normalize(image: mut image)
    Image.sharpen(image: mut image)
    Image.inspect(image: read image)
    Image.save(image: read image, path: read output)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("image.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), rsscript_runtime::ImageError>"));
    assert!(rust.contains("let mut image = rsscript_runtime::image_load(&input)?;"));
    assert!(rust.contains("rsscript_runtime::image_resize(&mut image, 320, 240);"));
    assert!(rust.contains("rsscript_runtime::image_normalize(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_sharpen(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_inspect(&image);"));
    assert!(rust.contains("rsscript_runtime::image_save(&image, &output)?;"));
}

#[test]
fn rust_lowering_maps_image_cache_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn cache_image(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local cache = ImageCache.new(capacity: 1)
    local image = Image.load(path: read input)?
    let shared = manage image
    ImageCache.store(cache: mut cache, image: read shared)
    Image.save(image: read shared, path: read output)?
    let count = ImageCache.len(cache: read cache)
    if count == 1 {
        Log.write(message: read "cached")
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("image-cache.rss", source).expect("source should lower");

    assert!(rust.contains("let mut cache = rsscript_runtime::image_cache_new(1);"));
    assert!(rust.contains("let shared = rsscript_runtime::manage_at(image,"));
    assert!(rust.contains("rsscript_runtime::image_cache_store(&mut cache, &shared);"));
    assert!(rust.contains("rsscript_runtime::image_save(&shared, &output)?;"));
    assert!(rust.contains("let count = rsscript_runtime::image_cache_len(&cache);"));
}

#[test]
fn rust_lowering_maps_http_handler_core_calls_to_runtime_hooks() {
    let source = r#"
fn handle_request(request: read Request) -> Result<fresh Response, HttpError> {
    let path = Request.path(request: read request)
    let body = String.concat(left: read "handled ", right: read path)
    return Response.ok(body: read body)
}

fn main() -> Result<Unit, HttpError> {
    let request = Request.new(path: read "/users")
    let response = handle_request(request: read request)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("http.rss", source).expect("source should lower");

    assert!(rust.contains("request: &rsscript_runtime::Request"));
    assert!(rust.contains("-> Result<rsscript_runtime::Response, rsscript_runtime::HttpError>"));
    assert!(rust.contains("let path = rsscript_runtime::request_path(&request);"));
    assert!(rust.contains("return rsscript_runtime::response_ok(&body);"));
    assert!(rust.contains("let request = rsscript_runtime::request_new(&\"/users\".to_string());"));
    assert!(rust.contains("let response = handle_request(&request)?;"));
}

#[test]
fn rust_lowering_maps_db_resource_pool_to_runtime_hooks() {
    let source = r#"
features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 2,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("db.rss", source).expect("source should lower");

    assert!(rust.contains("url: &String"));
    assert!(rust.contains("-> Result<(), rsscript_runtime::DbError>"));
    assert!(rust.contains("let mut pool = rsscript_runtime::ResourcePool::from_factory(2, || rsscript_runtime::db_connection_open(&url));"));
    assert!(rust.contains("let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(&mut pool, rsscript_runtime::SourceSpan::new(\"db.rss\""));
    assert!(rust.contains("rsscript_runtime::db_connection_query(&mut conn, &sql)?;"));
}

#[test]
fn rust_lowering_maps_config_reload_to_runtime_hooks() {
    let source = r#"
features: local

fn load_config(path: read String) -> Result<fresh ConfigValue, ConfigError> {
    return Config.load(path: read path)
}

fn reload_config(path: read String, store: mut ConfigStore) -> Result<Unit, ConfigError> {
    let next = load_config(path: read path)?
    ConfigStore.replace(store: mut store, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("config.rss", source).expect("source should lower");

    assert!(
        rust.contains("-> Result<rsscript_runtime::ConfigValue, rsscript_runtime::ConfigError>")
    );
    assert!(rust.contains("return rsscript_runtime::config_load(&path);"));
    assert!(rust.contains("store: &mut rsscript_runtime::ConfigStore"));
    assert!(rust.contains("rsscript_runtime::config_store_replace(store, &next);"));
}

#[test]
fn rust_lowering_maps_rules_config_reload_to_runtime_hooks() {
    let source = r#"
fn load_rules_config(path: read String) -> Result<fresh Config, ConfigError> {
    let rules = RuleLoader.load_rules(path: read path)?
    return Ok(Config.new(name: read "rules", rules: read rules))
}

fn reload_rules_config(path: read String, global: mut GlobalConfig) -> Result<Unit, ConfigError> {
    let next = load_rules_config(path: read path)?
    GlobalConfig.replace(global: mut global, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("rules-config.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<rsscript_runtime::Config, rsscript_runtime::ConfigError>"));
    assert!(rust.contains("let rules = rsscript_runtime::rule_loader_load_rules(&path)?;"));
    assert!(
        rust.contains("return Ok(rsscript_runtime::config_new(&\"rules\".to_string(), &rules));")
    );
    assert!(rust.contains("global: &mut rsscript_runtime::GlobalConfig"));
    assert!(rust.contains("rsscript_runtime::global_config_replace(global, &next);"));
}

#[test]
fn rust_lowering_maps_counter_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let counter = Counter.new(value: 1)
    Counter.add(counter: mut counter, amount: 2)
    let value = Counter.value(counter: read counter)
    if value == 3 {
        Log.write(message: read "counter ran")
    }
    return Unit
}
"#;
    let rust = lower_source_to_rust("counter.rss", source).expect("source should lower");

    assert!(rust.contains("let mut counter = rsscript_runtime::counter_new(1);"));
    assert!(rust.contains("rsscript_runtime::counter_add(&mut counter, 2);"));
    assert!(rust.contains("let value = rsscript_runtime::counter_value(&counter);"));
    assert!(rust.contains("if value == 3 {"));
}

#[test]
fn rust_lowering_maps_interpreter_cycle_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn main() -> Unit {
    local root_value = Environment.root()
    let root = manage root_value
    local child_value = Environment.child(parent: read root)
    let child = manage child_value
    local function_value = FunctionObject.new(closure: read child)
    let function = manage function_value
    Environment.bind_function(env: mut child, function: read function)
    if Environment.has_function(env: read child) && FunctionObject.has_closure(function: read function) {
        Log.write(message: read "linked")
    }
    return Unit
}
"#;
    let rust = lower_source_to_rust("interpreter-cycle.rss", source).expect("source should lower");

    assert!(rust.contains("let root_value = rsscript_runtime::environment_root();"));
    assert!(rust.contains("let root = rsscript_runtime::manage_at(root_value,"));
    assert!(rust.contains("let child_value = rsscript_runtime::environment_child(&root);"));
    assert!(rust.contains("let mut child = rsscript_runtime::manage_at(child_value,"));
    assert!(rust.contains("let function_value = rsscript_runtime::function_object_new(&child);"));
    assert!(rust.contains("let function = rsscript_runtime::manage_at(function_value,"));
    assert!(rust.contains("rsscript_runtime::environment_bind_function(&mut child, &function);"));
    assert!(rust.contains("rsscript_runtime::function_object_has_closure(&function)"));
}

#[test]
fn rust_lowering_decodes_string_escape_sequences() {
    let source = r#"
fn json_text() -> String {
    return "{\"profile\":{\"name\":\"RSScript\"}}"
}
"#;
    let rust = lower_source_to_rust("string-escapes.rss", source).expect("source should lower");

    assert!(
        rust.contains("return \"{\\\"profile\\\":{\\\"name\\\":\\\"RSScript\\\"}}\".to_string();")
    );
}

#[test]
fn rust_lowering_maps_log_write_to_runtime_output_hook() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read "hello RSScript")
    return Unit
}
"#;
    let rust = lower_source_to_rust("log.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::log_write(&\"hello RSScript\".to_string());"));
}

#[test]
fn rust_lowering_maps_assert_equal_to_runtime_hook() {
    let source = r#"
fn main() -> Unit {
    Assert.equal(left: read "rss", right: read "rss")
    return Unit
}
"#;
    let rust = lower_source_to_rust("assert.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "rsscript_runtime::assert_equal(&\"rss\".to_string(), &\"rss\".to_string());"
        )
    );
}

#[test]
fn rust_lowering_maps_string_concat_to_rust_std_expression() {
    let source = r#"
fn main() -> Unit {
    let message = String.concat(left: read "hello ", right: read "world")
    Log.write(message: read message)
    return Unit
}
"#;
    let rust = lower_source_to_rust("string.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let message = format!(\"{}{}\", &\"hello \".to_string(), &\"world\".to_string());"
    ));
    assert!(rust.contains("rsscript_runtime::log_write(&message);"));
}

#[test]
fn rust_lowering_maps_int_add_to_rust_std_expression() {
    let source = r#"
fn main() -> Unit {
    let value = 20 + 22
    return Unit
}
"#;
    let rust = lower_source_to_rust("int.rss", source).expect("source should lower");

    assert!(rust.contains("let value = 20 + 22;"));
}

#[test]
fn rust_lowering_maps_builtin_operators_to_rust_expressions() {
    let source = r#"
fn main() -> Unit {
    let difference = 44 - 2
    let product = 6 * 7
    let quotient = product / 2
    let equal = product == 42
    let different = quotient != 0
    let less = quotient < product
    let less_equal = quotient <= product
    let greater = product > quotient
    let greater_equal = product >= quotient
    return Unit
}
"#;
    let rust = lower_source_to_rust("operators.rss", source).expect("source should lower");

    assert!(rust.contains("let difference = 44 - 2;"));
    assert!(rust.contains("let product = 6 * 7;"));
    assert!(rust.contains("let quotient = product / 2;"));
    assert!(rust.contains("let equal = product == 42;"));
    assert!(rust.contains("let different = quotient != 0;"));
    assert!(rust.contains("let less = quotient < product;"));
    assert!(rust.contains("let less_equal = quotient <= product;"));
    assert!(rust.contains("let greater = product > quotient;"));
    assert!(rust.contains("let greater_equal = product >= quotient;"));
}

#[test]
fn rust_lowering_maps_bool_literals_to_rust_literals() {
    let source = r#"
fn main() -> Unit {
    let enabled = true
    let disabled = false
    let changed = enabled != disabled
    return Unit
}
"#;
    let rust = lower_source_to_rust("bool.rss", source).expect("source should lower");

    assert!(rust.contains("let enabled = true;"));
    assert!(rust.contains("let disabled = false;"));
    assert!(rust.contains("let changed = enabled != disabled;"));
}

#[test]
fn rust_lowering_maps_try_operator_to_rust_result_propagation() {
    let source = r#"
features: local

struct BuildError {
    code: Int
}

struct Point {
    x: Int
    y: Int
}

fn maybe_point(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Ok(Point(x: x, y: y))
}

fn shift(point: mut Point) -> Unit

pub fn use_try(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    local point = maybe_point(x: x, y: y)?
    shift(point: mut point)
    return Ok(point)
}
"#;
    let lowered = lower_source_to_rust_with_map("try.rss", source).expect("source should lower");

    assert!(
        lowered
            .rust_source
            .contains("let mut point = maybe_point(x, y)?;")
    );
    assert!(lowered.rust_source.contains("shift(&mut point);"));
    assert!(lowered.rust_source.contains("return Ok(point);"));
    assert!(lowered.source_map.iter().any(|entry| entry.kind == "try"));
}

#[test]
fn rust_lowering_matches_golden_fixture() {
    let source = read_fixture(Path::new("tests/golden/lowering/simple.rss"));
    let expected_rust = read_fixture(Path::new("tests/golden/lowering/simple.rs"));
    let expected_source_map =
        read_fixture(Path::new("tests/golden/lowering/simple.source-map.txt"));
    let lowered =
        lower_source_to_rust_with_map("simple.rss", &source).expect("source should lower");

    assert_eq!(lowered.rust_source, expected_rust);
    assert_eq!(source_map_summary(&lowered.source_map), expected_source_map);
}

#[test]
fn rust_lowering_emits_machine_readable_source_map() {
    let source = r#"
features: local

class Session {
    id: Int
}

fn save(session: read Session) -> Unit

pub fn make_session(id: Int) -> Session {
    local session = Session(id: id)
    save(session: read session)
    return manage session
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
    assert!(kinds.contains(&"field"));
    assert!(lowered.source_map.iter().any(|entry| {
        entry.source.file == "session.rss"
            && entry.generated.file == "src/lib.rs"
            && entry.generated.line > 0
            && entry.generated.column > 0
    }));
}

#[test]
fn rustc_diagnostics_map_back_to_rsscript_source_spans() {
    let source = r#"
features: local

class Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    local session = Session(id: id)
    return manage session
}
"#;
    let lowered =
        lower_source_to_rust_with_map("session.rss", source).expect("source should lower");
    let rust_line = lowered
        .rust_source
        .lines()
        .position(|line| line.contains("return rsscript_runtime::manage_at(session,"))
        .map(|index| index + 1)
        .expect("generated Rust should contain manage return");
    let rustc_json = format!(
        r#"{{"message":"mismatched types","code":{{"code":"E0308","explanation":null}},"level":"error","spans":[{{"file_name":"src/lib.rs","line_start":{rust_line},"line_end":{rust_line},"column_start":12,"column_end":40,"is_primary":true}}]}}"#
    );

    let remapped = remap_rustc_diagnostic_json(&lowered.source_map, &rustc_json)
        .expect("rustc JSON should parse")
        .expect("error should produce a diagnostic");

    assert!(remapped.mapped);
    assert_eq!(remapped.diagnostic.code, "RS1101");
    assert_eq!(remapped.diagnostic.span.file, "session.rss");
    assert!(
        remapped
            .diagnostic
            .causes
            .iter()
            .any(|cause| cause.contains("rustc code: E0308"))
    );
}

#[test]
fn rustc_diagnostics_report_unmappable_generated_spans() {
    let rustc_json = r#"{"message":"cannot find value","code":{"code":"E0425","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":99,"line_end":99,"column_start":5,"column_end":10,"is_primary":true}]}"#;

    let remapped = remap_rustc_diagnostic_json(&[], rustc_json)
        .expect("rustc JSON should parse")
        .expect("error should produce a diagnostic");

    assert!(!remapped.mapped);
    assert_eq!(remapped.diagnostic.code, "RS1102");
    assert_eq!(remapped.diagnostic.span.file, "src/lib.rs");
}

#[test]
fn rustc_diagnostic_line_remap_ignores_non_diagnostic_messages() {
    let lines = r#"{"reason":"compiler-artifact","target":{"name":"generated"}}
{"reason":"compiler-message","message":{"message":"cannot find value","code":{"code":"E0425","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":99,"line_end":99,"column_start":5,"column_end":10,"is_primary":true}]}}"#;

    let remapped =
        remap_rustc_diagnostic_json_lines(&[], lines).expect("rustc JSON lines should parse");

    assert_eq!(remapped.len(), 1);
    assert_eq!(remapped[0].diagnostic.code, "RS1102");
}

#[test]
fn rust_lowering_targets_runtime_crate_hooks() {
    let source = r#"
features: local

resource TestConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn pooled(pool: mut ResourcePool<TestConnection>) -> Unit
"#;
    let rust = lower_source_to_rust("pool.rssi", source).expect("source should lower");

    assert!(rust.contains("impl rsscript_runtime::Resource for TestConnection"));
    assert!(rust.contains("pool: &mut rsscript_runtime::ResourcePool<TestConnection>"));
    assert!(rust.contains("let _ = &pool;"));
}

#[test]
fn rust_lowering_emits_source_spans_for_resource_pool_borrow() {
    let source = r#"
features: local

resource TestConnection {
    fd: Int
}

fn TestConnection.query(conn: mut TestConnection, sql: read String) -> Unit

fn pooled(pool: mut ResourcePool<TestConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        TestConnection.query(conn: mut conn, sql: read "select 1")
    }
}
"#;
    let rust = lower_source_to_rust("pool.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(pool, rsscript_runtime::SourceSpan::new(\"pool.rss\""
    ));
}

#[test]
fn rust_lowering_wraps_managed_class_returns_in_gc() {
    let source = r#"
features: local

class Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    local session = Session(id: id)
    return manage session
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub struct Session"));
    assert!(rust.contains("pub fn make_session(id: i64) -> rsscript_runtime::Gc<Session>"));
    assert!(rust.contains("let session = Session { id: id };"));
    assert!(rust.contains("return rsscript_runtime::manage_at(session, rsscript_runtime::SourceSpan::new(\"session.rss\", 10, 12, 6));"));
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

fn touch(counter: mut Meter) -> Unit

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
    assert!(rust.contains("return read_value(&counter);"));
}

#[test]
fn rust_lowering_can_emit_cargo_package_artifacts() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

pub fn make_point(x: Int, y: Int) -> fresh Point {
    return Point(x: x, y: y)
}
"#;
    let package = lower_source_to_rust_package(
        "point.rss",
        source,
        "Point Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");

    assert_eq!(package.package_name, "point-example-rss");
    assert!(package.cargo_toml.contains("[workspace]"));
    assert!(
        package
            .cargo_toml
            .contains("rsscript-runtime = { path = \"/workspace/rsscript/runtime\" }")
    );
    assert!(package.lib_rs.contains("pub struct Point"));
    assert!(package.main_rs.is_none());
    let source_map: Value =
        serde_json::from_str(&package.source_map_json).expect("source map JSON should parse");
    assert!(source_map.as_array().is_some_and(|items| !items.is_empty()));
}

#[test]
fn rust_lowering_can_emit_runnable_main_harness() {
    let source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let package = lower_source_to_rust_package(
        "main.rss",
        source,
        "Runnable Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");
    let main_rs = package
        .main_rs
        .as_ref()
        .expect("zero-argument Unit main should emit a Rust binary harness");

    assert_eq!(package.package_name, "runnable-example-rss");
    assert!(package.lib_rs.contains("pub fn main()"));
    assert!(main_rs.contains("runnable_example_rss::main();"));
}

#[test]
fn rust_lowering_can_emit_result_main_harness() {
    let source = r#"
struct MainError

fn main() -> Result<Unit, MainError> {
    return Ok(Unit)
}
"#;
    let package = lower_source_to_rust_package(
        "main.rss",
        source,
        "Result Runnable Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");
    let main_rs = package
        .main_rs
        .as_ref()
        .expect("zero-argument Result<Unit, E> main should emit a Rust binary harness");

    assert!(
        package
            .lib_rs
            .contains("pub fn main() -> Result<(), MainError>")
    );
    assert!(main_rs.contains(
        "result_runnable_example_rss::main().expect(\"RSScript main returned an error\");"
    ));
}

#[test]
fn rust_lowering_is_gated_by_diagnostics() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn bad(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    Image.inspect(image: read image)
}
"#;
    let diagnostics = lower_source_to_rust("bad.rss", source).expect_err("source should fail");
    let codes = diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0401".to_string()));
}

#[test]
fn review_json_uses_protocol_shape() {
    let old_source = r#"

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
features: local

fn render(path: take Path) -> fresh Image
    effects(retains(path))
{
    Image.load(path: read path)
}
"#;
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    let items = value.as_array().expect("review JSON should be an array");

    assert!(
        items
            .iter()
            .any(|item| item["code"] == "RSR001" && item["risk"] == "feature")
    );
    assert!(
        items
            .iter()
            .any(|item| item["code"] == "RSR006" && item["risk"] == "effect")
    );
    let effect = items
        .iter()
        .find(|item| item["code"] == "RSR006")
        .expect("expected effect review finding");
    assert_eq!(effect["before"], "no_panic");
    assert_eq!(effect["after"], "retains(path)");
    assert!(
        effect["spans"]
            .as_array()
            .is_some_and(|spans| spans.len() == 2)
    );
    let effect_fixes = effect["fixes"]
        .as_array()
        .expect("review finding should include fixes");
    assert!(effect_fixes.iter().any(|fix| {
        fix["kind"] == "review_effect_contract" && fix["applicability"] == "manual"
    }));
    assert!(items.iter().all(|item| {
        item["fixes"]
            .as_array()
            .is_some_and(|fixes| !fixes.is_empty())
    }));
    assert!(items.iter().all(|item| {
        item["summary"]
            .as_str()
            .is_some_and(|summary| !summary.is_empty())
    }));
}

#[test]
fn syntax_parser_accepts_all_fixtures() {
    let mut paths = fixture_paths("tests/fixtures/pass");
    paths.extend(fixture_paths("tests/fixtures/fail"));
    paths.extend(fixture_paths("examples"));

    for path in paths {
        let source = read_fixture(&path);
        let program = parse_source(path.to_str().unwrap(), &source);
        assert!(
            !program.items.is_empty(),
            "{} missing items",
            path.display()
        );
        assert!(
            program
                .items
                .iter()
                .any(|item| matches!(item, Item::Function(_))),
            "{} missing function item",
            path.display()
        );
        if path.extension().is_some_and(|extension| extension == "rss") {
            assert!(
                program.items.iter().any(|item| match item {
                    Item::Function(function) => !function.body.statements.is_empty(),
                    Item::Type(_) => false,
                }),
                "{} missing function body statements",
                path.display()
            );
        }
    }
}

#[test]
fn review_reports_api_contract_changes() {
    let old_source = r#"

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
features: local

fn render(path: take Path, width: Int) -> fresh Image
    effects(retains(path))
{
    Image.load(path: read path)
}

fn inspect(image: read Image) -> Unit {
    Image.inspect(image: read image)
}
"#;

    let codes: Vec<String> = review_sources("old.rss", old_source, "new.rss", new_source)
        .into_iter()
        .map(|finding| finding.code)
        .collect();

    assert!(codes.contains(&"RSR001".to_string()));
    assert!(codes.contains(&"RSR003".to_string()));
    assert!(codes.contains(&"RSR004".to_string()));
    assert!(codes.contains(&"RSR005".to_string()));
    assert!(codes.contains(&"RSR006".to_string()));
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR001" && finding.risk == ReviewRisk::Feature)
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR006" && finding.risk == ReviewRisk::Effect)
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR004" && finding.risk == ReviewRisk::Api)
    );
    assert!(format_review_human(&findings).contains("RSR006[effect]:"));
}

#[test]
fn review_reports_function_kind_changes() {
    let old_source = r#"
fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    Net.fetch(url: read url)
}
"#;
    let new_source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    Net.fetch(url: read url)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let kind = findings
        .iter()
        .find(|finding| finding.code == "RSR014")
        .expect("expected function kind review finding");

    assert_eq!(kind.risk, ReviewRisk::Api);
    assert_eq!(kind.before.as_deref(), Some("fn"));
    assert_eq!(kind.after.as_deref(), Some("async fn"));
    assert!(format_review_human(&findings).contains("RSR014[api]: function `fetch` kind changed."));
}

#[test]
fn review_reports_new_unsafe_native_usage() {
    let old_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic, unsafe, native)
{
    Native.checksum(data: read data)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let unsafe_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR012")
        .expect("expected unsafe/native review finding");

    assert_eq!(unsafe_finding.risk, ReviewRisk::Unsafe);
    assert_eq!(unsafe_finding.before.as_deref(), Some("<none>"));
    assert_eq!(unsafe_finding.after.as_deref(), Some("native, unsafe"));
    assert!(format_review_human(&findings).contains("RSR012[unsafe]: function `checksum` added"));

    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR012" && item["risk"] == "unsafe")
    }));
}

#[test]
fn review_reports_removed_guarantees() {
    let old_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic)
{
    Bytes.checksum(data: read data)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let guarantee = findings
        .iter()
        .find(|finding| finding.code == "RSR013")
        .expect("expected removed guarantee finding");

    assert_eq!(guarantee.risk, ReviewRisk::Guarantee);
    assert_eq!(guarantee.before.as_deref(), Some("no_panic, noalloc, pure"));
    assert_eq!(guarantee.after.as_deref(), Some("no_panic"));
    assert!(
        guarantee
            .summary
            .contains("removed guarantee(s): noalloc, pure")
    );
    assert!(format_review_human(&findings).contains("RSR013[guarantee]:"));

    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR013" && item["risk"] == "guarantee")
    }));
}

#[test]
fn review_reports_type_layout_changes() {
    let old_source = r#"

struct Config {
    rules: List<Rule>
    version: Int
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
"#;
    let new_source = r#"

class Config {
    rules: handle List<Rule>
    version: Int
}

struct Session {
    config: Config
}
"#;

    let codes: Vec<String> = review_sources("old.rss", old_source, "new.rss", new_source)
        .into_iter()
        .map(|finding| finding.code)
        .collect();

    assert!(codes.contains(&"RSR007".to_string()));
    assert!(codes.contains(&"RSR008".to_string()));
    assert!(codes.contains(&"RSR009".to_string()));
    assert!(codes.contains(&"RSR010".to_string()));
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR010" && finding.risk == ReviewRisk::TypeLayout)
    );
}

#[test]
fn review_reports_local_manage_boundary_changes() {
    let old_source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    Image.inspect(image: read Image.load(path: read path))
}
"#;
    let new_source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let codes: Vec<String> = findings
        .iter()
        .map(|finding| finding.code.clone())
        .collect();

    assert!(codes.contains(&"RSR011".to_string()));
    let boundary = findings
        .iter()
        .find(|finding| finding.code == "RSR011")
        .expect("expected boundary review finding");
    assert!(boundary.summary.contains("added local binding `image`"));
    assert!(boundary.summary.contains("added manage `image`"));
    assert!(boundary.summary.contains("body[1]"));
    assert!(boundary.summary.contains("body[2].value"));
    assert_eq!(boundary.risk, ReviewRisk::Boundary);
    assert!(
        boundary
            .fixes
            .iter()
            .any(|fix| fix.kind == "review_local_manage_boundary")
    );
}

#[test]
fn review_map_partitions_functions_for_review() {
    let source = r#"
fn helper(value: read Int) -> Int {
    return value
}

pub fn publish(path: read Path) -> Image {
    local image = Image.load(path: read path)
    return manage image
}

fn delegated(value: read Int) -> Int {
    return unknown(value: read value)
}
"#;

    let map = review_map_sources(vec![("map.rss", source)]);
    let regions = &map.files[0].regions;

    assert_eq!(map.summary.total_functions, 3);
    assert_eq!(map.summary.review_required.functions, 1);
    assert_eq!(map.summary.foldable.functions, 1);
    assert_eq!(map.summary.unknown.functions, 1);
    assert!(map.summary.total_lines >= 3);
    assert!(format_review_map_human(&map).starts_with("summary: must-review 1 functions/"));
    let json = format_review_map_json(&map);
    let value: Value = serde_json::from_str(&json).expect("review map JSON should parse");
    assert_eq!(value["summary"]["total_functions"], 3);
    assert_eq!(value["summary"]["must_review"]["functions"], 1);
    assert_eq!(value["summary"]["safe_to_skip"]["functions"], 1);
    assert_eq!(value["summary"]["unknown"]["functions"], 1);
    assert!(value["summary"]["must_review_lines"].is_number());
    assert!(value["summary"]["safe_to_skip_lines"].is_number());
    assert!(value["summary"]["suggested_review_lines"].is_number());
    assert!(value["summary"]["review_ratio"].is_number());
    assert!(
        value["files"][0]["regions"]
            .as_array()
            .is_some_and(|regions| regions
                .iter()
                .any(|region| region["classification"] == "must_review"))
    );
    assert!(
        value["files"][0]["regions"]
            .as_array()
            .is_some_and(|regions| regions
                .iter()
                .any(|region| region["classification"] == "safe_to_skip"))
    );
    assert!(
        value["files"]
            .as_array()
            .is_some_and(|files| files.len() == 1)
    );
    assert!(
        value["files"][0]["features"]
            .as_array()
            .is_some_and(|features| features.is_empty())
    );

    assert!(regions.iter().any(|region| {
        region.function == "helper" && region.classification == ReviewMapClassification::Foldable
    }));
    assert!(regions.iter().any(|region| {
        region.function == "publish"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "public entry point")
    }));
    assert!(regions.iter().any(|region| {
        region.function == "delegated" && region.classification == ReviewMapClassification::Unknown
    }));
}

#[test]
fn parser_accepts_bodyless_rssi_interface() {
    let source = r#"
struct JsonValue

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>

pub fn field(
    value: read JsonValue,
    name: read String,
) -> Result<fresh JsonValue, JsonError>

pub fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
"#;
    let program = parse_source("json.rssi", source);

    assert!(program.features.is_empty());
    assert_eq!(program.items.len(), 4);
    assert!(matches!(&program.items[0], Item::Type(type_decl) if type_decl.name == "JsonValue"));
    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "parse" && function.is_public && function.body.statements.is_empty())
    );
    assert!(
        matches!(&program.items[2], Item::Function(function) if function.name == "field" && function.is_public && function.body.statements.is_empty())
    );
    assert!(
        matches!(&program.items[3], Item::Function(function) if function.name == "field_string" && function.is_public && function.body.statements.is_empty())
    );
    assert!(analyze_source("json.rssi", source).is_empty());
}

#[test]
fn parser_preserves_async_function_kind() {
    let source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>
"#;
    let program = parse_source("net.rssi", source);

    assert!(
        matches!(&program.items[0], Item::Function(function) if function.name == "fetch" && function.is_async)
    );
    assert!(analyze_source("net.rssi", source).is_empty());
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
fn review_map_marks_public_rssi_signatures_review_required() {
    let source = r#"
struct JsonValue

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#;
    let map = review_map_sources(vec![("json.rssi", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "parse")
        .expect("expected parse function in review map");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "public entry point")
    );
}

#[test]
fn review_map_marks_unknown_qualified_calls_unknown() {
    let source = r#"
fn delegated(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let map = review_map_sources(vec![("map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "delegated")
        .expect("expected delegated function in review map");

    assert_eq!(region.classification, ReviewMapClassification::Unknown);
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason.contains("Mystery.run"))
    );
}

#[test]
fn review_map_marks_callers_of_unknown_functions_unknown() {
    let source = r#"
fn delegated(value: read Int) -> Int {
    return Mystery.run(value: read value)
}

fn wrapper(value: read Int) -> Int {
    return delegated(value: read value)
}
"#;
    let map = review_map_sources(vec![("unknown-call.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.unknown.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::Unknown
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls unknown `delegated`")
    }));
}

#[test]
fn review_map_marks_private_entry_functions_review_required() {
    let source = r#"
fn helper(value: read Int) -> Int {
    return value
}

fn main() -> Unit {
    helper(value: read 1)
}

fn handle_request(request: read Request) -> Response {
    return Response.ok()
}
"#;
    let map = review_map_sources(vec![("entry.rss", source)]);

    assert_eq!(map.summary.total_functions, 3);
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 1);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "main"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region.reasons.iter().any(|reason| reason == "entry point")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "handle_request"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region.reasons.iter().any(|reason| reason == "entry point")
    }));
}

#[test]
fn review_map_marks_callers_of_review_required_functions() {
    let source = r#"
fn store(value: read Payload) -> Unit
    effects(retains(value))
{
    Cache.store(value: read value)
}

fn wrapper(value: read Payload) -> Unit {
    store(value: read value)
}
"#;
    let map = review_map_sources(vec![("calls.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls must-review `store`")
    }));
}

#[test]
fn review_map_marks_resourcepool_and_fresh_boundaries() {
    let source = r#"
resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Image {
    pixels: Buffer
}

fn make_image() -> fresh Image

fn pooled(pool: mut ResourcePool<DbConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.ping(conn: mut conn)
    }
}
"#;
    let map = review_map_sources(vec![("resourcepool.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "make_image"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "fresh guarantee boundary")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "pooled"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "ResourcePool usage")
    }));
}

#[test]
fn review_map_marks_runtime_guarantee_boundaries() {
    let source = r#"
fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic)
{
    return 1
}

fn pure_helper(value: read Int) -> Int
    effects(pure)
{
    return value
}
"#;
    let map = review_map_sources(vec![("guarantees.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 1);
    assert_eq!(map.summary.foldable.functions, 1);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "checksum"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `noalloc`")
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `no_panic`")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "pure_helper"
            && region.classification == ReviewMapClassification::Foldable
    }));
}

#[test]
fn review_map_marks_mut_call_site_effects() {
    let source = r#"
struct Counter {
    value: Int
}

fn bump(counter: mut Counter) -> Unit

fn update(counter: read Counter) -> Unit {
    bump(counter: mut counter)
}
"#;
    let map = review_map_sources(vec![("mut-call-site.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "update"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "mut call-site effect")
    }));
}

#[test]
fn review_map_reports_file_features() {
    let source = r#"
features: local, native, ffi, reflection

fn process() -> Unit {
    return Unit
}
"#;
    let map = review_map_sources(vec![("features.rss", source)]);

    assert_eq!(
        map.files[0].features,
        vec!["ffi", "local", "native", "reflection"]
    );
    assert_eq!(map.files[0].risk, ReviewMapFileRisk::High);
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "local capability enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "native boundary capability enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "ffi boundary capability enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reflection capability enabled")
    );
    let human = format_review_map_human(&map);
    assert!(human.contains("features.rss: features ffi, local, native, reflection; risk high"));
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["files"][0]["features"][0], "ffi");
    assert_eq!(json["files"][0]["features"][1], "local");
    assert_eq!(json["files"][0]["features"][2], "native");
    assert_eq!(json["files"][0]["features"][3], "reflection");
    assert_eq!(json["files"][0]["risk"], "high");
    assert!(
        json["files"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native boundary capability enabled"))
    );
}

fn fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory}: {error}"))
        .map(|entry| entry.expect("fixture entry should be readable").path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        })
        .collect();
    paths.sort();
    paths
}

fn recursive_fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_fixture_paths(Path::new(directory), &mut paths);
    paths.sort();
    paths
}

fn collect_fixture_paths(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory:?}: {error}"))
    {
        let path = entry.expect("fixture entry should be readable").path();
        if path.is_dir() {
            collect_fixture_paths(&path, paths);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        {
            paths.push(path);
        }
    }
}

fn read_fixture(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn expected_codes(source: &str) -> Vec<String> {
    let first_line = source.lines().next().unwrap_or_default();
    let Some(codes) = first_line.strip_prefix("// expect:") else {
        panic!("fail fixture must start with `// expect:`");
    };
    codes.split_whitespace().map(str::to_string).collect()
}

fn source_map_summary(entries: &[rsscript::RustSourceMapEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{} {}:{}:{} -> {}:{}:{}\n",
                entry.kind,
                entry.source.file,
                entry.source.line,
                entry.source.column,
                entry.generated.file,
                entry.generated.line,
                entry.generated.column
            )
        })
        .collect()
}
