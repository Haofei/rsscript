use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::syntax::ast::{EffectDecl, Item};
use rsscript::syntax::parse_source;
use rsscript::{
    NativeRustDependency, ReviewMapClassification, ReviewMapFileRisk, ReviewRisk, analyze_source,
    analyze_source_with_core, analyze_source_with_interfaces, check_generated_rust_package,
    check_package_dir, core_interfaces, diff_package_dirs, diff_package_locks,
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_json,
    format_package_lock_toml, format_review_human, format_review_json, format_review_map_human,
    format_review_map_json, lint_source, lock_package_dir, lower_source_to_rust,
    lower_source_to_rust_package, lower_source_to_rust_with_map,
    lower_sources_to_rust_package_with_options, package_lowering_input, package_metadata,
    package_tree, parse_runtime_diagnostics, publish_package_dry_run, remap_rustc_diagnostic_json,
    remap_rustc_diagnostic_json_lines, review_map_sources, review_package_dir, review_sources,
    vendor_package_dir, write_generated_rust_package,
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
        assert!(
            !package.lib_rs.contains("todo!"),
            "{} generated library should not contain todo! fallbacks",
            path.display()
        );
        if let Some(main_rs) = &package.main_rs {
            assert!(
                !main_rs.contains("todo!"),
                "{} generated main should not contain todo! fallbacks",
                path.display()
            );
        }
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
fn take_with_resource_reports_resource_escape_not_plain_take_error() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn consume_file(file: take File) -> Unit {
}

fn bad_take(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        consume_file(file: take file)
    }
}
"#;
    let codes = analyze_source("resource-take.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0702".to_string()));
    assert!(!codes.contains(&"RS0308".to_string()));
}

#[test]
fn managed_closure_capture_makes_fresh_local_unclean() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn bad_fresh(path: read Path) -> fresh Image {
    local image = Image.load(path: read path)

    let callback = || {
        Image.inspect(image: read image)
    }

    return image
}
"#;
    let codes = analyze_source("fresh-closure-capture.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0801".to_string()));
    assert!(codes.contains(&"RS0601".to_string()));
}

#[test]
fn retained_closure_capture_makes_fresh_local_unclean() {
    let source = r#"
features: local

class Scheduler {
    callbacks: List<Callback>
}

struct Image {
    pixels: Buffer
}

fn schedule(scheduler: mut Scheduler, callback: read Callback) -> Unit
    effects(retains(callback))
{
}

fn bad_schedule(scheduler: mut Scheduler, path: read Path) -> fresh Image {
    local image = Image.load(path: read path)
    schedule(
        scheduler: mut scheduler,
        callback: read || {
            Image.inspect(image: read image)
        },
    )
    return image
}
"#;
    let codes = analyze_source("fresh-retained-closure-capture.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0801".to_string()));
    assert!(codes.contains(&"RS0601".to_string()));
}

#[test]
fn fresh_return_allows_inline_field_of_clean_local() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

struct DecodeResult {
    image: Image
    metadata: Metadata
}

fn decode(path: read Path) -> fresh DecodeResult

fn load_image(path: read Path) -> fresh Image {
    local decoded = decode(path: read path)
    return decoded.image
}
"#;

    assert_eq!(analyze_source("fresh-inline-field.rss", source), Vec::new());
}

#[test]
fn fresh_return_rejects_handle_field_of_clean_local() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

struct ImageBox {
    image: handle Image
}

fn load_box(path: read Path) -> fresh ImageBox

fn bad_image(path: read Path) -> fresh Image {
    local boxed = load_box(path: read path)
    return boxed.image
}
"#;
    let codes = analyze_source("fresh-handle-field.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0602".to_string()));
}

#[test]
fn resource_pool_read_parameter_must_be_local_capability() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        Db.close(fd: fd)
    }
}

fn bad_pool(pool: read ResourcePool<DbConnection>) -> Unit {
    DbConnection.count(pool: read pool)
}
"#;
    let codes = analyze_source("resourcepool-read-param.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0705".to_string()));
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
    let fresh_unknown = explain_diagnostic_code("RS0602").expect("RS0602 should be registered");

    assert_eq!(explanation.title, "use after manage");
    assert!(formatted.contains("RS0401"));
    assert!(formatted.contains("manage"));
    assert!(fresh_unknown.explanation.contains("clean inline fields"));
    assert!(explain_diagnostic_code("RS9999").is_none());
}

#[test]
fn lint_warns_on_public_signature_complexity() {
    let source = r#"
features: native

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
fn rust_lowering_maps_path_construction_to_runtime_hook() {
    let source = r#"
fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "rsscript-path.txt")
    with File.open_write(path: read path) as file {
        File.write(file: mut file, data: read "path hook ran")?
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("path.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let path = rsscript_runtime::path_from_string(&\"rsscript-path.txt\".to_string());"
    ));
    assert!(rust.contains("let mut file = rsscript_runtime::file_open_write(&path)?;"));
}

#[test]
fn rust_lowering_maps_file_read_into_and_buffer_reuse_to_runtime_hooks() {
    let source = r#"
features: local

fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    local buffer = Buffer.new(size: 8192)
    with File.open_read(path: read input) as reader {
        with File.open_write(path: read output) as writer {
            while File.read_into(file: mut reader, buffer: mut buffer)? {
                File.write(file: mut writer, data: read buffer)?
                Buffer.clear(buffer: mut buffer)
            }
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file-buffer.rss", source).expect("source should lower");

    assert!(rust.contains("let mut buffer = rsscript_runtime::buffer_new(8192);"));
    assert!(rust.contains("while rsscript_runtime::file_read_into(&mut reader, &mut buffer)? {"));
    assert!(rust.contains("rsscript_runtime::file_write(&mut writer, &buffer)?;"));
    assert!(rust.contains("rsscript_runtime::buffer_clear(&mut buffer);"));
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

struct Session {
    id: Int
}

fn save(session: read Session) -> Unit

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
    assert!(kinds.contains(&"field"));
    assert!(lowered.source_map.iter().any(|entry| {
        entry.source.file == "session.rss"
            && entry.generated.file == "src/lib.rs"
            && entry.generated.line > 0
            && entry.generated.column > 0
    }));
}

#[test]
fn rust_lowering_maps_with_resource_drop_points() {
    let source = r#"
fn copy(path: read Path) -> Result<Unit, FileError> {
    with File.open_read(path: read path) as file {
        File.read_all(file: mut file)?
    }

    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust_with_map("resource.rss", source).expect("source should lower");

    assert!(
        lowered
            .rust_source
            .contains("    {\n        let mut file = rsscript_runtime::file_open_read(&path)?;")
    );
    assert!(lowered.rust_source.contains(
        "        rsscript_runtime::file_read_all(&mut file)?;\n        // rss:span kind=resource_drop"
    ));
    assert!(
        lowered
            .rust_source
            .contains("// rss:span kind=resource_drop file=resource.rss")
    );
    assert!(
        lowered
            .source_map
            .iter()
            .any(|entry| entry.kind == "resource_drop" && entry.source.file == "resource.rss")
    );
}

#[test]
fn rust_lowering_maps_native_call_boundaries() {
    let source = r#"
features: native

native fn host_emit(message: read String) -> Unit

pub fn run() -> Unit {
    host_emit(message: read "host")
    Log.write(message: read "core")
}
"#;
    let lowered = lower_source_to_rust_with_map("native.rss", source).expect("source should lower");
    let native_calls = lowered
        .source_map
        .iter()
        .filter(|entry| entry.kind == "native_call")
        .collect::<Vec<_>>();

    assert_eq!(native_calls.len(), 2);
    assert!(native_calls.iter().any(|entry| entry.source.line == 7));
    assert!(native_calls.iter().any(|entry| entry.source.line == 8));
}

#[test]
fn rustc_diagnostics_map_back_to_rsscript_source_spans() {
    let source = r#"
features: local

struct Session {
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
fn runtime_diagnostic_lines_parse_to_rsscript_diagnostics() {
    let stderr = r#"thread 'main' panicked at runtime/src/lib.rs:1:1:
RSSCRIPT_RUNTIME_DIAGNOSTIC:{"code":"RS1201","severity":"error","summary":"RSScript runtime error: resource pool has no available resources","file":"pool.rss","line":8,"column":10,"length":24,"label":"resource pool has no available resources","kind":"resource_pool_empty"}
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"#;

    let diagnostics = parse_runtime_diagnostics(stderr);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "RS1201");
    assert_eq!(
        diagnostics[0].summary,
        "RSScript runtime error: resource pool has no available resources"
    );
    assert_eq!(diagnostics[0].span.file, "pool.rss");
    assert_eq!(diagnostics[0].span.line, 8);
    assert!(
        diagnostics[0]
            .causes
            .iter()
            .any(|cause| cause == "runtime error kind: resource_pool_empty")
    );
}

#[test]
fn rss_run_maps_runtime_conflict_to_rsscript_diagnostic() {
    let temp_dir = unique_temp_dir("rsscript-runtime-diagnostic");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("empty_pool.rss");
    write_runtime_conflict_fixture(&source_path);

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(stdout.contains("error[RS1201]"), "{stdout}");
    assert!(
        stdout.contains("RSScript runtime error: resource pool has no available resources"),
        "{stdout}"
    );
    assert!(stdout.contains("empty_pool.rss"), "{stdout}");
    assert!(
        stdout.contains("runtime error kind: resource_pool_empty"),
        "{stdout}"
    );
}

#[test]
fn rss_run_json_maps_runtime_conflict_to_diagnostics_json() {
    let temp_dir = unique_temp_dir("rsscript-runtime-diagnostic-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("empty_pool.rss");
    write_runtime_conflict_fixture(&source_path);

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("--json")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(!output.status.success());
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json[0]["code"], "RS1201");
    assert_eq!(json[0]["severity"], "error");
    assert_eq!(
        json[0]["spans"][0]["file"],
        source_path.display().to_string()
    );
    assert!(
        json[0]["causes"]
            .as_array()
            .expect("causes should be an array")
            .iter()
            .any(|cause| cause == "runtime error kind: resource_pool_empty")
    );
}

#[test]
fn rss_fmt_outputs_canonical_source() {
    let temp_dir = unique_temp_dir("rsscript-fmt-cli");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("messy.rss");
    fs::write(
        &source_path,
        r#"features:   local
fn   main( )->Unit{
local value=String.concat(left:read "hello",right:read " fmt")
Log.write(message:read value)
return Unit
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("fmt")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss fmt should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(
        stdout,
        r#"features: local

fn main() -> Unit {
    local value = String.concat(left: read "hello", right: read " fmt")
    Log.write(message: read value)
    return Unit
}
"#
    );
}

#[test]
fn rss_run_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-run-package-cli");
    write_named_package_fixture(&temp_dir, "rss-run-package", "0.1.0", "", "");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "hello package")
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(stdout.contains("hello package"), "{stdout}");
}

#[test]
fn rss_run_accepts_multi_file_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-run-multi-package-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-run-multi-package",
        "0.1.0",
        "",
        "pub fn package_message() -> fresh String\n",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/helper.rss"),
        r#"pub fn package_message() -> fresh String {
    return String.concat(left: read "hello", right: read " multi package")
}
"#,
    )
    .expect("helper source should be written");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    let message = package_message()
    Log.write(message: read message)
}
"#,
    )
    .expect("main source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(stdout.contains("hello multi package"), "{stdout}");
}

#[test]
fn rss_run_json_remaps_rustc_compile_errors() {
    let temp_dir = unique_temp_dir("rsscript-run-rustc-remap");
    write_named_package_fixture(
        &temp_dir,
        "rss-run-rustc-remap",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello")
    Log.write(message: read message)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: String) -> String { message }\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(!output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json[0]["code"], "RS1101");
    assert!(
        json[0]["spans"][0]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("src/main.rss"))
    );
    assert!(
        json[0]["causes"]
            .as_array()
            .expect("causes should be an array")
            .iter()
            .any(|cause| cause
                .as_str()
                .is_some_and(|cause| cause.contains("rustc code: E0308")))
    );
}

#[test]
fn rss_verify_rust_json_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-verify-package-cli");
    write_named_package_fixture(&temp_dir, "rss-verify-package", "0.1.0", "", "");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "verify package")
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
}

#[test]
fn rss_verify_rust_accepts_package_native_wrapper_dependency() {
    let temp_dir = unique_temp_dir("rsscript-verify-native-package-cli");
    let out_dir = temp_dir.join("generated-rust");
    write_named_package_fixture(
        &temp_dir,
        "rss-verify-native-package",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "verify native package")
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .arg("--out-dir")
        .arg(&out_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let generated_cargo_toml =
        fs::read_to_string(out_dir.join("Cargo.toml")).expect("generated Cargo.toml should exist");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
    assert!(generated_cargo_toml.contains("\"rss_json_native\" = { path = "));
}

#[test]
fn rss_verify_rust_lowers_native_binding_manifest_calls() {
    let temp_dir = unique_temp_dir("rsscript-verify-native-binding-package-cli");
    let out_dir = temp_dir.join("generated-rust");
    write_named_package_fixture(
        &temp_dir,
        "rss-verify-native-binding-package",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .arg("--out-dir")
        .arg(&out_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let generated_lib_rs =
        fs::read_to_string(out_dir.join("src/lib.rs")).expect("generated lib.rs should exist");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
    assert!(generated_lib_rs.contains("rss_json_native::echo"));
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
    assert!(rust.contains("impl Drop for TestConnection"));
    assert!(rust.contains("rsscript_runtime::os_close(self.fd);"));
    assert!(rust.contains("pool: &mut rsscript_runtime::ResourcePool<TestConnection>"));
    assert!(rust.contains("let _ = &pool;"));
    assert!(!rust.contains("todo!"));
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
fn rust_lowering_wraps_managed_class_returns_in_managed_handle() {
    let source = r#"
class Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    return Session(id: id)
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub struct Session"));
    assert!(rust.contains("pub fn make_session(id: i64) -> rsscript_runtime::Managed<Session>"));
    assert!(rust.contains("return rsscript_runtime::manage_at(Session { id: id }, rsscript_runtime::SourceSpan::new(\"session.rss\", 7, 12, 7));"));
}

#[test]
fn rust_lowering_wraps_handle_fields_once() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: User
    explicit_owner: handle User
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub owner: rsscript_runtime::Managed<User>"));
    assert!(rust.contains("pub explicit_owner: rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("rsscript_runtime::Managed<rsscript_runtime::Managed<User>>"));
}

#[test]
fn rust_lowering_maps_weak_class_fields_to_runtime_weak_handles() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn make_session() -> Session {
    let user = User(id: 1)
    return Session(owner: read user)
}

fn make_session_from_param(user: read User) -> Session {
    return Session(owner: read user)
}
"#;
    let rust = lower_source_to_rust("weak-session.rss", source).expect("source should lower");

    assert!(rust.contains("pub owner: rsscript_runtime::WeakManaged<User>"));
    assert!(rust.contains("owner: rsscript_runtime::weak(&user)"));
    assert!(rust.contains("owner: rsscript_runtime::weak(user)"));

    let package = lower_source_to_rust_package(
        "weak-session.rss",
        source,
        "weak-session",
        &format!("{}/runtime", env!("CARGO_MANIFEST_DIR")),
    )
    .expect("source should lower into package");
    let temp_dir = unique_temp_dir("rsscript-weak-session");
    write_generated_rust_package(&temp_dir, &package).expect("generated package should be written");
    let check =
        check_generated_rust_package(&temp_dir).expect("generated package should be checked");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        check.success,
        "diagnostics={:?}\nstderr={}",
        check.diagnostics, check.stderr
    );
}

#[test]
fn rust_lowering_uses_shared_handles_for_managed_class_mut_parameters() {
    let source = r#"
features: local

class User {
    id: Int
}

fn touch(user: mut User) -> Unit

fn call(user: mut User) -> Unit {
    touch(user: mut user)
}

fn promote(id: Int) -> Unit {
    let shared = User(id: id)
    touch(user: mut shared)
}
"#;
    let rust = lower_source_to_rust("user.rss", source).expect("source should lower");

    assert!(rust.contains("fn touch(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("fn call(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("touch(user);"));
    assert!(rust.contains("touch(&shared);"));
    assert!(!rust.contains("&mut rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("touch(&mut shared);"));
}

#[test]
fn rust_lowering_reads_managed_class_fields_through_runtime_handle() {
    let source = r#"
features: local

class User {
    id: Int
}

fn user_id(user: read User) -> Int {
    return user.id
}

fn main() -> Unit {
    let shared = User(id: 42)
    let id = user_id(user: read shared)
    Assert.equal(left: read "42", right: read "42")
    return Unit
}
"#;
    let package = lower_source_to_rust_package(
        "user.rss",
        source,
        "managed-class-field",
        &format!("{}/runtime", env!("CARGO_MANIFEST_DIR")),
    )
    .expect("source should lower into package");
    assert!(package.lib_rs.contains(
        "return rsscript_runtime::unwrap_runtime(user.try_read_at(rsscript_runtime::SourceSpan::new(\"user.rss\", 9, 12, 4))).id.clone();"
    ));

    let temp_dir = unique_temp_dir("rsscript-managed-class-field");
    write_generated_rust_package(&temp_dir, &package).expect("generated package should be written");
    let check =
        check_generated_rust_package(&temp_dir).expect("generated package should be checked");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        check.success,
        "diagnostics={:?}\nstderr={}",
        check.diagnostics, check.stderr
    );
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
fn rust_lowering_can_emit_native_wrapper_path_dependencies() {
    let source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let package = lower_sources_to_rust_package_with_options(
        &[("main.rss".to_string(), source.to_string())],
        "Native Example.rss",
        "/workspace/rsscript/runtime",
        &[],
        &[NativeRustDependency {
            crate_name: "rss_json_native".to_string(),
            path: "/workspace/rss-json/native/rust".to_string(),
            bindings: BTreeMap::new(),
        }],
    )
    .expect("source should lower into package with native dependency");

    assert!(
        package
            .cargo_toml
            .contains("\"rss_json_native\" = { path = \"/workspace/rss-json/native/rust\" }")
    );
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
    assert!(main_rs.contains("rsscript_runtime::install_runtime_diagnostic_panic_hook();"));
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
    assert!(main_rs.contains("rsscript_runtime::install_runtime_diagnostic_panic_hook();"));
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
fn rust_lowering_rejects_unsupported_syntax_before_generation() {
    let source = r#"
fn bad(path: read Path) -> Unit {
    with File.open(path: read path) {
        return Unit
    }
}
"#;
    let diagnostics = lower_source_to_rust("unsupported.rss", source)
        .expect_err("unsupported source should fail before Rust generation");
    let codes = diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0015".to_string()));
}

#[test]
fn checker_reports_unknown_top_level_items_as_unsupported() {
    let source = r#"
enum Color {
    Red
}

fn main() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("unknown-top-level.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "unsupported top-level item"
    }));
}

#[test]
fn checker_reports_malformed_declarations_as_unsupported() {
    let source = r#"
fn (value: read String) -> Unit {
    return Unit
}

struct {
    value: String
}

fn main() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-declarations.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed declaration"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_bindings_and_arguments_as_unsupported() {
    let source = r#"
fn main() -> Unit {
    let missing =
    print(value:)
}
"#;
    let diagnostics = analyze_source("malformed-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "malformed statement")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported expression")
    );
}

#[test]
fn checker_rejects_retaining_local_inline_field() {
    let source = r#"
features: local

struct Image
struct Holder {
    image: Image
}
struct Cache

fn Cache.store(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))

fn make_holder(path: read Path) -> fresh Holder

fn bad_store(cache: mut Cache, path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    Cache.store(cache: mut cache, value: read holder.image)
}
"#;
    let diagnostics = analyze_source("retaining-local-field.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0501" && diagnostic.label == "local value retained"
    ));
}

#[test]
fn checker_accepts_managed_closure_capturing_handle_field() {
    let source = r#"
features: local

class Image

struct Holder {
    image: handle Image
}

fn make_holder(path: read Path) -> fresh Holder

fn use_image(image: read Image) -> Unit

fn ok_capture(path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    let callback = || {
        use_image(image: read holder.image)
    }
}
"#;
    let diagnostics = analyze_source("managed-closure-handle-field.rss", source);

    assert_eq!(diagnostics, Vec::new());
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
        .expect("expected unsafe review finding");
    let native_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR015")
        .expect("expected native review finding");

    assert_eq!(unsafe_finding.risk, ReviewRisk::Unsafe);
    assert_eq!(unsafe_finding.before.as_deref(), Some("<none>"));
    assert_eq!(unsafe_finding.after.as_deref(), Some("unsafe"));
    assert_eq!(native_finding.risk, ReviewRisk::Boundary);
    assert_eq!(native_finding.before.as_deref(), Some("<none>"));
    assert_eq!(native_finding.after.as_deref(), Some("native"));
    assert!(
        format_review_human(&findings)
            .contains("RSR012[unsafe]: function `checksum` added unsafe boundary.")
    );
    assert!(
        format_review_human(&findings)
            .contains("RSR015[boundary]: function `checksum` added native boundary.")
    );

    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR012" && item["risk"] == "unsafe")
    }));
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR015" && item["risk"] == "boundary")
    }));
}

#[test]
fn review_reports_native_fn_as_native_boundary() {
    let old_source = r#"
fn host_emit(message: read String) -> Unit
{
    Log.write(message: read message)
}
"#;
    let new_source = r#"
native fn host_emit(message: read String) -> Unit
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let native_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR015")
        .expect("expected native boundary review finding");

    assert_eq!(native_finding.risk, ReviewRisk::Boundary);
    assert_eq!(native_finding.before.as_deref(), Some("<none>"));
    assert_eq!(native_finding.after.as_deref(), Some("native"));
    assert!(!findings.iter().any(|finding| finding.code == "RSR012"));
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
fn checker_reports_async_bodies_as_unsupported_until_async_lowering_exists() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    return Network.fetch(url: read url)
}
"#;
    let diagnostics = analyze_source("async-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported async function body")
    );
}

#[test]
fn parser_accepts_native_function_declaration() {
    let source = r#"
features: native

native fn Host.emit(message: read String) -> Unit
"#;
    let program = parse_source("host.rssi", source);

    let Item::Function(function) = &program.items[0] else {
        panic!("expected native function declaration");
    };
    assert_eq!(function.name, "Host.emit");
    assert!(function.is_native);
    assert!(
        function
            .effects
            .iter()
            .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "native"))
    );
    assert!(analyze_source("host.rssi", source).is_empty());
}

#[test]
fn checker_reports_native_bodies_as_unsupported_until_native_binding_exists() {
    let source = r#"
features: native

native fn Host.emit(message: read String) -> Unit {
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
fn checker_accepts_noescape_callback_that_temporarily_uses_local() {
    let source = r#"
features: local

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn use_local(path: read Path) -> Result<fresh Image, ImageError> {
    local image = Image.load(path: read path)?
    apply(callback: || {
        Image.inspect(image: read image)
    })
    return Ok(image)
}

fn main() -> Result<Unit, ImageError> {
    let path = Path.from_string(value: read "rsscript-image-input.bin")
    use_local(path: read path)?
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("noescape.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let program = parse_source("noescape.rss", source);
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .find(|function| function.name == "apply")
        .expect("apply should parse");
    assert!(function.params[0].ty.is_noescape);
    assert_eq!(function.params[0].ty.name, "Fn");

    let lowered = lower_source_to_rust("noescape.rss", source)
        .expect("noescape callback source should lower");
    assert!(lowered.contains("callback: impl FnOnce()"));
    assert!(lowered.contains("callback();"));
}

#[test]
fn checker_accepts_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
        None => return 0
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("match.rss", source).expect("match should lower");
    assert!(lowered.contains("match value"));
    assert!(lowered.contains("Some(result) =>"));
    assert!(lowered.contains("None =>"));
}

#[test]
fn checker_reports_non_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0021" && diagnostic.label == "non-exhaustive match"
    ));
}

#[test]
fn checker_reports_for_as_unsupported_until_iteration_lowering_exists() {
    let source = r#"
fn run(items: read List<Int>) -> Unit {
    for item in items {
        Log.write(message: read "x")
    }
}
"#;
    let diagnostics = analyze_source("for.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0015" && diagnostic.label == "unsupported statement"
    ));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0206")
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
fn review_map_marks_public_direct_unknown_calls_unknown() {
    let source = r#"
pub fn run(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let map = review_map_sources(vec![("public-unknown.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "run")
        .expect("expected run function in review map");

    assert_eq!(region.classification, ReviewMapClassification::Unknown);
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "public entry point")
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason.contains("Mystery.run"))
    );
    assert_eq!(map.summary.unknown.functions, 1);
}

#[test]
fn checker_does_not_resolve_qualified_calls_by_short_name() {
    let source = r#"
fn run(value: read Int) -> Int {
    return value
}

fn caller(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let diagnostics = analyze_source("qualified-short-name.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0206" && diagnostic.summary.contains("Mystery.run")
    }));
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
    return Unit
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
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Image {
    pixels: Buffer
}

struct Buffer

fn make_image() -> fresh Image {
    return Image(pixels: Buffer())
}

fn pooled(pool: mut ResourcePool<DbConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        return Unit
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
fn review_map_marks_error_handling_boundaries() {
    let source = r#"
fn may_fail() -> Result<Unit, IOError>

fn load() -> Result<Unit, IOError> {
    may_fail()?
    return Ok(Unit)
}
"#;
    let map = review_map_sources(vec![("error-boundary.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "load"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "error handling boundary")
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
fn review_map_marks_writes_to_managed_state() {
    let source = r#"
features: local

struct Counter {
    value: Int
}

fn bump(counter: mut Counter) -> Unit

fn update_managed(counter: read Counter) -> Unit {
    bump(counter: mut counter)
}

fn update_local() -> Unit {
    local counter = Counter(value: 1)
    bump(counter: mut counter)
}
"#;
    let map = review_map_sources(vec![("managed-write.rss", source)]);

    let managed = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "update_managed")
        .expect("expected managed update region");
    assert_eq!(
        managed.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        managed
            .reasons
            .iter()
            .any(|reason| reason == "writes to managed state")
    );

    let local = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "update_local")
        .expect("expected local update region");
    assert!(
        local
            .reasons
            .iter()
            .all(|reason| reason != "writes to managed state")
    );
}

#[test]
fn review_map_marks_writes_through_handle_fields() {
    let source = r#"
class Cache {
    value: Int
}

struct State {
    cache: handle Cache
}

fn touch(cache: mut Cache) -> Unit

fn update(state: read State) -> Unit {
    touch(cache: mut state.cache)
}
"#;
    let map = review_map_sources(vec![("handle-write.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "update"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "writes through handle field")
    }));
}

#[test]
fn review_map_reports_file_features() {
    let source = r#"
features: local, native, device, ffi, reflection

fn process() -> Unit {
    return Unit
}
"#;
    let map = review_map_sources(vec![("features.rss", source)]);

    assert_eq!(
        map.files[0].features,
        vec!["device", "ffi", "local", "native", "reflection"]
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
            .any(|reason| reason == "reserved device review marker enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reserved ffi review marker enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reserved reflection review marker enabled")
    );
    let human = format_review_map_human(&map);
    assert!(
        human.contains("features.rss: features device, ffi, local, native, reflection; risk high")
    );
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["files"][0]["features"][0], "device");
    assert_eq!(json["files"][0]["features"][1], "ffi");
    assert_eq!(json["files"][0]["features"][2], "local");
    assert_eq!(json["files"][0]["features"][3], "native");
    assert_eq!(json["files"][0]["features"][4], "reflection");
    assert_eq!(json["files"][0]["risk"], "high");
    assert!(
        json["files"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native boundary capability enabled"))
    );
    assert!(
        json["files"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "reserved ffi review marker enabled"))
    );
}

#[test]
fn package_review_reads_manifest_and_reports_semantic_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-review");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-json"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["src"]

[dependencies]
rss-core = "0.5"

[features]
streaming = []

[review]
risk = "low"
allow_native = true

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
proc_macros = "forbid"
unsafe = "forbid"
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/json.rssi"),
        r#"features: native

struct JsonValue
struct JsonError

native fn Json.parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    )
    .expect("interface should be written");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(text: read String) -> String {
    return text
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["package"]["name"], "rss-json");
    assert_eq!(json["risk"], "high");
    assert_eq!(json["features"], serde_json::json!(["streaming"]));
    assert_eq!(json["summary"]["interface_files"], 1);
    assert_eq!(json["summary"]["source_files"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust wrapper enabled")
    }));
    assert!(human.contains("package features: streaming"));
}

#[test]
fn package_review_loads_path_dependency_interfaces_for_source_checks() {
    let root_dir = unique_temp_dir("rsscript-package-dep-interface-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-interface-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(text: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(review.summary.source_files, 1);
    assert_eq!(review.diagnostics, Vec::new());
}

#[test]
fn package_review_reports_missing_interface_implementation() {
    let temp_dir = unique_temp_dir("rsscript-package-missing-interface-impl");
    write_named_package_fixture(
        &temp_dir,
        "rss-missing-impl",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_implementation_signature_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-interface-mismatch");
    write_named_package_fixture(
        &temp_dir,
        "rss-interface-mismatch",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> fresh String
    effects(no_panic)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn render(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes.iter().any(|cause| cause.contains(
            "interface: pub fn render(body: read String) -> fresh String effects(no_panic)"
        )),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: pub fn render(body: read String) -> String")),
        "{causes:?}"
    );
}

#[test]
fn package_review_reports_missing_interface_type_declaration() {
    let temp_dir = unique_temp_dir("rsscript-package-missing-interface-type");
    write_named_package_fixture(
        &temp_dir,
        "rss-missing-type",
        "0.1.0",
        "",
        r#"struct PublicConfig {
    name: String
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_type_contract_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-interface-type-mismatch");
    write_named_package_fixture(
        &temp_dir,
        "rss-type-mismatch",
        "0.1.0",
        "",
        r#"class Session<T: Managed> {
    user: handle User
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"struct Session<T: Managed> {
    user: User
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes
            .iter()
            .any(|cause| cause
                .contains("interface: class Session<T: Managed> { user: handle User }")),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: struct Session<T: Managed> { user: User }")),
        "{causes:?}"
    );
}

#[test]
fn package_review_reports_path_dependency_interface_call_violations() {
    let root_dir = unique_temp_dir("rsscript-package-dep-interface-violation-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-interface-violation-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(value: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0203"), "{codes:?}");
    assert!(codes.contains(&"RS0204"), "{codes:?}");
    assert!(!codes.contains(&"RS0206"), "{codes:?}");
}

#[test]
fn package_review_reports_dependency_interface_symbol_conflicts_without_sources() {
    let root_dir = unique_temp_dir("rsscript-package-interface-conflict-root");
    let dep_dir = unique_temp_dir("rsscript-package-interface-conflict-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0005"), "{codes:?}");
}

#[test]
fn package_review_includes_lint_warnings_for_public_contracts() {
    let temp_dir = unique_temp_dir("rsscript-package-review-lint");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

pub fn Api.overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
    effects(no_panic, noalloc, no_block, pure, native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["errors"], 0);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package contains frontend warnings")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "RSL001"
                && diagnostic["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("7 parameters"))
        })
    }));
}

#[test]
fn rss_package_review_json_reports_package_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-review-cli");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-math"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/math.rssi"),
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    )
    .expect("interface should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("review")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package review should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package review JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["package"]["name"], "rss-math");
    assert_eq!(json["summary"]["interface_files"], 1);
}

#[test]
fn package_review_json_counts_native_and_unsafe_apis_separately() {
    let temp_dir = unique_temp_dir("rsscript-package-review-api-effects");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native, unsafe

native fn Native.echo(message: read String) -> String
fn Native.danger(message: read String) -> String
    effects(unsafe)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["native_apis"], 1);
    assert_eq!(json["summary"]["unsafe_apis"], 1);
}

#[test]
fn package_review_json_counts_public_api_review_categories() {
    let temp_dir = unique_temp_dir("rsscript-package-review-api-categories");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"struct Image
resource DbConnection

pub fn Image.load(path: read String) -> fresh Image
pub fn Cache.store(conn: mut DbConnection, image: read Image) -> Unit
    effects(retains(image))
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_types"], 2);
    assert_eq!(json["summary"]["public_functions"], 2);
    assert_eq!(json["summary"]["public_apis"], 4);
    assert_eq!(json["summary"]["mutating_apis"], 1);
    assert_eq!(json["summary"]["retaining_apis"], 1);
    assert_eq!(json["summary"]["resource_apis"], 1);
    assert_eq!(json["summary"]["fresh_returning_apis"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 0);
    let exports = json["exports"]
        .as_array()
        .expect("exports should be an array");
    assert!(exports.iter().any(|export| {
        export["name"] == "DbConnection"
            && export["kind"] == "type"
            && export["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().any(|reason| reason == "resource type"))
    }));
    assert!(exports.iter().any(|export| {
        export["name"] == "Cache.store"
            && export["kind"] == "function"
            && export["classification"] == "review_if_changed"
            && export["reasons"].as_array().is_some_and(|reasons| {
                reasons
                    .iter()
                    .any(|reason| reason == "mut parameter `conn`")
                    && reasons.iter().any(|reason| reason == "retains(image)")
                    && reasons
                        .iter()
                        .any(|reason| reason == "resource parameter `conn`")
            })
    }));
    assert!(human.contains("exports:"));
    assert!(human.contains("function Cache.store: review_if_changed"));
    assert!(human.contains("retains(image)"));
    assert!(human.contains("type DbConnection: review_if_changed"));
}

#[test]
fn package_review_json_counts_public_apis_with_unknown_review_regions() {
    let temp_dir = unique_temp_dir("rsscript-package-review-unknown-api");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    helper()
    return Unit
}

fn helper() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 2);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
    assert!(human.contains("function Api.run: unknown"));
    assert!(human.contains("unknown review-map region"));
}

#[test]
fn package_review_json_counts_public_api_with_direct_unknown_call() {
    let temp_dir = unique_temp_dir("rsscript-package-review-direct-unknown-api");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 1);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
}

#[test]
fn package_check_fails_unknown_review_when_configured_as_error() {
    let temp_dir = unique_temp_dir("rsscript-package-check-unknown-is-error");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[sources]
paths = ["src"]

[review]
risk = "unknown"
unknown_is_error = true
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["lock"]["matches"], true);
    assert_eq!(json["summary"]["errors"], 0);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "unknown package risk is configured as an error")
    }));
}

#[test]
fn package_review_marks_broken_rssi_contract_diagnostics_unknown() {
    let temp_dir = unique_temp_dir("rsscript-package-review-broken-rssi");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"fn (value: read String) -> Unit
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "public .rssi contract contains frontend errors")
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["kind"] == "contract_diagnostic"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "frontend error RS0015")
                })
        })
    }));
    assert!(human.contains("contract_diagnostic interface/lib.rssi:1:1: unknown"));
}

#[test]
fn package_metadata_dry_run_reports_review_metadata_without_writing() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-dry-run");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata",
        "0.1.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let metadata_path_exists = temp_dir.join("review").join("package-review.json").exists();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(metadata.ok);
    assert!(!metadata_path_exists);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["written"], false);
    assert_eq!(json["metadata"]["schema"], "rss.review.package.v1");
    assert_eq!(json["metadata"]["package"]["name"], "rss-metadata");
    assert_eq!(json["metadata"]["features"], serde_json::json!(["fast"]));
}

#[test]
fn package_metadata_reports_unknown_review_risk_not_ok() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-unknown-risk");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata-unknown",
        "0.1.0",
        r#"[review]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!metadata.ok);
    assert_eq!(json["ok"], false);
    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["metadata"]["summary"]["errors"], 0);
}

#[test]
fn rss_package_metadata_json_writes_review_metadata_file() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("metadata")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package metadata should execute");
    let metadata_path = temp_dir.join("review").join("package-review.json");
    let metadata_source =
        fs::read_to_string(&metadata_path).expect("metadata file should be written");
    let metadata_file_json: Value =
        serde_json::from_str(&metadata_source).expect("metadata file JSON should parse");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be metadata JSON");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["written"], true);
    assert_eq!(json["metadata"]["schema"], "rss.review.package.v1");
    assert_eq!(metadata_file_json["schema"], "rss.review.package.v1");
    assert_eq!(metadata_file_json["package"]["name"], "rss-metadata");
}

#[test]
fn package_diff_reports_manifest_and_interface_contract_changes() {
    let old_dir = unique_temp_dir("rsscript-package-diff-old");
    let new_dir = unique_temp_dir("rsscript-package-diff-new");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[dependencies]
rss-core = "0.5"
rss-cache = "0.1"

[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
"#,
        r#"features: native

struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["new_package"]["version"], "0.2.0");
    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "dependency" && change["name"] == "rss-cache")
    }));
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "native-rust" && change["name"] == "build_scripts")
    }));
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["file"] == "interface/lib.rssi" && change["risk"] == "high")
    }));
}

#[test]
fn rss_package_diff_json_reports_dependency_upgrade() {
    let old_dir = unique_temp_dir("rsscript-package-diff-cli-old");
    let new_dir = unique_temp_dir("rsscript-package-diff-cli-new");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.1",
        r#"[dependencies]
rss-core = "0.6"
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("diff")
        .arg("--json")
        .arg(&old_dir)
        .arg(&new_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package diff should execute");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package diff JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "dependency" && change["name"] == "rss-core")
    }));
}

#[test]
fn package_lock_records_contract_review_and_native_hashes() {
    let temp_dir = unique_temp_dir("rsscript-package-lock");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse(_: &str) {}\n",
    )
    .expect("native source should be written");

    let lock = lock_package_dir(&temp_dir).expect("package lock should succeed");
    let toml = format_package_lock_toml(&lock);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(lock.version, 1);
    assert_eq!(lock.packages.len(), 1);
    assert_eq!(lock.packages[0].name, "rss-json");
    assert_eq!(lock.packages[0].features, vec!["streaming".to_string()]);
    assert!(lock.packages[0].checksum.starts_with("sha256:"));
    assert!(lock.packages[0].interface_hash.starts_with("sha256:"));
    assert!(lock.packages[0].review_hash.starts_with("sha256:"));
    assert!(
        lock.packages[0]
            .native_hash
            .as_ref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(toml.contains("[[package]]"));
    assert!(toml.contains("rss_version = \""));
    assert!(toml.contains("interface_hash = \"sha256:"));
}

#[test]
fn package_lock_review_hash_tracks_native_api_count_changes() {
    let old_dir = unique_temp_dir("rsscript-package-lock-native-api-old");
    let new_dir = unique_temp_dir("rsscript-package-lock-native-api-new");
    let native_manifest = r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#;
    write_package_fixture(
        &old_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
native fn Native.two(message: read String) -> String
"#,
    );
    fs::create_dir_all(old_dir.join("native")).expect("old native dir should be created");
    fs::write(
        old_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"#,
    )
    .expect("old native bindings should be written");
    fs::create_dir_all(new_dir.join("native")).expect("new native dir should be created");
    fs::write(
        new_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"Native.two" = "rss_native::two"
"#,
    )
    .expect("new native bindings should be written");

    let old_review = review_package_dir(&old_dir).expect("old package review should succeed");
    let new_review = review_package_dir(&new_dir).expect("new package review should succeed");
    let old_lock = lock_package_dir(&old_dir).expect("old package lock should succeed");
    let new_lock = lock_package_dir(&new_dir).expect("new package lock should succeed");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(old_review.summary.native_apis, 1);
    assert_eq!(new_review.summary.native_apis, 2);
    assert_ne!(
        old_lock.packages[0].review_hash,
        new_lock.packages[0].review_hash
    );
}

#[test]
fn package_lock_records_local_path_dependency_graph() {
    let root_dir = unique_temp_dir("rsscript-package-lock-graph-root");
    let dep_dir = unique_temp_dir("rsscript-package-lock-graph-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );

    let lock = lock_package_dir(&root_dir).expect("package lock should include path deps");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_json(&lock))
        .expect("package lock JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(lock.packages.len(), 2);
    assert_eq!(json["package"][0]["name"], "rss-app");
    assert_eq!(json["package"][1]["name"], "rss-dep");
    assert_eq!(json["package"][1]["features"][0], "fast");
    assert!(
        json["package"][1]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn rss_package_lock_json_reports_hashes() {
    let temp_dir = unique_temp_dir("rsscript-package-lock-cli");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("lock")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package lock should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package lock JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["version"], 1);
    assert_eq!(json["package"][0]["name"], "rss-json");
    assert!(
        json["package"][0]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(
        json["package"][0]["review_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn package_check_reports_stale_dependency_interface_lock() {
    let root_dir = unique_temp_dir("rsscript-package-check-dep-lock-root");
    let dep_dir = unique_temp_dir("rsscript-package-check-dep-lock-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        dep_dir.join("interface/lib.rssi"),
        r#"pub fn Dep.parse(value: read String) -> String
"#,
    )
    .expect("dependency interface should be changed");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert_eq!(json["lock"]["matches"], false);
    assert!(
        json["lock"]["package_changes"]
            .as_array()
            .is_some_and(|changes| {
                changes.iter().any(|change| {
                    change["name"] == "rss-dep"
                        && change["changes"].as_array().is_some_and(|fields| {
                            fields.iter().any(|field| {
                                field["field"] == "interface_hash" && field["risk"] == "high"
                            })
                        })
                })
            })
    );
}

#[test]
fn package_check_reports_local_dependency_version_conflict() {
    let root_dir = unique_temp_dir("rsscript-package-check-conflict-root");
    let dep_a_dir = unique_temp_dir("rsscript-package-check-conflict-dep-a");
    let dep_b_dir = unique_temp_dir("rsscript-package-check-conflict-dep-b");
    let shared_v1_dir = unique_temp_dir("rsscript-package-check-conflict-shared-v1");
    let shared_v2_dir = unique_temp_dir("rsscript-package-check-conflict-shared-v2");
    write_named_package_fixture(
        &shared_v1_dir,
        "rss-shared",
        "0.1.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    write_named_package_fixture(
        &shared_v2_dir,
        "rss-shared",
        "0.2.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    write_named_package_fixture(
        &dep_a_dir,
        "rss-dep-a",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            shared_v1_dir.display()
        ),
        r#"pub fn DepA.run() -> Unit
"#,
    );
    write_named_package_fixture(
        &dep_b_dir,
        "rss-dep-b",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            shared_v2_dir.display()
        ),
        r#"pub fn DepB.run() -> Unit
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep-a = {{ path = "{}" }}
rss-dep-b = {{ path = "{}" }}
"#,
            dep_a_dir.display(),
            dep_b_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_a_dir);
    let _ = fs::remove_dir_all(&dep_b_dir);
    let _ = fs::remove_dir_all(&shared_v1_dir);
    let _ = fs::remove_dir_all(&shared_v2_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert_eq!(json["graph"]["risk"], "high");
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                .as_str()
                .is_some_and(|reason| reason.contains("rss-shared"))
        })
    }));
}

#[test]
fn package_review_update_reports_lockfile_contract_changes() {
    let old_dir = unique_temp_dir("rsscript-package-update-old");
    let new_dir = unique_temp_dir("rsscript-package-update-new");
    let lock_dir = unique_temp_dir("rsscript-package-update-locks");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    );
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    fs::write(
        &old_lock_path,
        format_package_lock_toml(&lock_package_dir(&old_dir).expect("old lock should be built")),
    )
    .expect("old lock should be written");
    fs::write(
        &new_lock_path,
        format_package_lock_toml(&lock_package_dir(&new_dir).expect("new lock should be built")),
    )
    .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("lock diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let _ = fs::remove_dir_all(&lock_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package feature selection changed")
    }));
    assert!(json["package_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["name"] == "rss-json"
                && change["changes"].as_array().is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| field["field"] == "interface_hash" && field["risk"] == "high")
                })
        })
    }));
}

#[test]
fn rss_package_review_update_json_reports_lock_changes() {
    let old_dir = unique_temp_dir("rsscript-package-update-cli-old");
    let new_dir = unique_temp_dir("rsscript-package-update-cli-new");
    let lock_dir = unique_temp_dir("rsscript-package-update-cli-locks");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.1",
        "",
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    );
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    fs::write(
        &old_lock_path,
        format_package_lock_toml(&lock_package_dir(&old_dir).expect("old lock should be built")),
    )
    .expect("old lock should be written");
    fs::write(
        &new_lock_path,
        format_package_lock_toml(&lock_package_dir(&new_dir).expect("new lock should be built")),
    )
    .expect("new lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("review")
        .arg("update")
        .arg("--json")
        .arg("--from")
        .arg(&old_lock_path)
        .arg("--to")
        .arg(&new_lock_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package review update should execute");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let _ = fs::remove_dir_all(&lock_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value =
        serde_json::from_str(&stdout).expect("stdout should be package review update JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["risk"], "high");
    assert!(
        json["package_changes"][0]["changes"]
            .as_array()
            .is_some_and(|changes| changes
                .iter()
                .any(|change| change["field"] == "interface_hash"))
    );
}

#[test]
fn package_check_reports_stale_semantic_lock() {
    let temp_dir = unique_temp_dir("rsscript-package-check-stale");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        temp_dir.join("interface/lib.rssi"),
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    )
    .expect("interface should be changed");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert_eq!(json["lock"]["present"], true);
    assert_eq!(json["lock"]["matches"], false);
    assert!(json["lock"]["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
}

#[test]
fn rss_package_check_json_reports_consistent_package() {
    let temp_dir = unique_temp_dir("rsscript-package-check-cli");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package check should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["lock"]["present"], true);
    assert_eq!(json["lock"]["matches"], true);
}

#[test]
fn rss_check_json_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-check-package-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-check-package",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss check package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["package"]["name"], "rss-check-package");
}

#[test]
fn rss_package_check_fails_when_lock_missing() {
    let temp_dir = unique_temp_dir("rsscript-package-check-missing-lock");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package check should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(!output.status.success(), "stdout={stdout}");
    assert_eq!(json["ok"], false);
    assert_eq!(json["lock"]["present"], false);
    assert!(
        json["lock"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == "rsspkg.lock missing"))
    );
}

#[test]
fn package_check_reports_native_rust_consistency_issues() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["cargo_toml_present"], false);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], false);
    assert_eq!(json["native_rust"]["risk"], "high");
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust Cargo.toml missing"))
    );
}

#[test]
fn package_check_reports_native_cargo_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-metadata");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(json["native_rust"]["cargo_toml_present"], true);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert_eq!(json["native_rust"]["cargo_package_name"], "rss_json_native");
    assert_eq!(json["native_rust"]["unsafe_detected"], false);
    assert_eq!(json["native_rust"]["build_env_detected"], false);
    assert_eq!(json["native_rust"]["build_download_detected"], false);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!([])
    );
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
    );
}

#[test]
fn package_check_accepts_bound_native_interface_functions() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok, "{:?}", check.diagnostics);
    assert!(
        check
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS1301")
    );
}

#[test]
fn package_check_reports_unknown_native_binding_symbols() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-unknown");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.ehco" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1302" && diagnostic.label == "unknown native binding symbol"
    }));
}

#[test]
fn package_check_reports_native_binding_crate_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-crate");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "other_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1302" && diagnostic.label == "native binding crate mismatch"
    }));
}

#[test]
fn package_check_reports_native_binding_without_native_rust() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-no-native");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1302"
            && diagnostic.label == "native binding without native Rust wrapper"
    }));
}

#[test]
fn package_check_reports_native_binding_missing_crate() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-no-crate");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1302" && diagnostic.label == "native binding crate missing"
    }));
}

#[test]
fn package_check_reports_native_unsafe_usage() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-unsafe");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        r#"pub fn parse() {
    let _ = "unsafe in a string";
    // unsafe in a comment should not count
    unsafe {}
}
"#,
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["unsafe_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust unsafe usage detected"))
    );
}

#[test]
fn package_check_reports_native_linked_libraries() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-links");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
links = ["ssl"]
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(check.risk, rsscript::PackageRisk::High);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!(["ssl"])
    );
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust links external libraries")
    }));
}

#[test]
fn package_check_reports_native_build_script_environment_usage() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-env");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    let _ = std::env::var("OUT_DIR");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_env_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script reads environment"))
    );
}

#[test]
fn package_check_reports_native_build_script_download_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-download");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    // https://example.invalid/commented-out should not be the only signal
    let _ = std::process::Command::new("curl").arg("https://example.invalid/archive.tar.gz");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_download_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script may download code"))
    );
}

#[test]
fn package_check_reports_native_build_script_from_cargo_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-script");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(temp_dir.join("native/rust/build.rs"), "fn main() {}\n")
        .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"))
    );
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script target present"))
    );
}

#[test]
fn package_lowering_input_records_native_wrapper_dependency() {
    let temp_dir = unique_temp_dir("rsscript-package-native-lowering-input");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");

    let input = package_lowering_input(&temp_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with native dependency");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(input.native_dependencies[0].crate_name, "rss_json_native");
    assert!(input.native_dependencies[0].path.ends_with("native/rust"));
    assert!(
        package
            .cargo_toml
            .contains("\"rss_json_native\" = { path = ")
    );
}

#[test]
fn package_tree_expands_path_dependencies_and_marks_unresolved() {
    let root_dir = unique_temp_dir("rsscript-package-tree-root");
    let dep_dir = unique_temp_dir("rsscript-package-tree-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_dep_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(dep_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        dep_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_dep_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        dep_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["streaming"] }}
rss-remote = "0.5"
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let tree = package_tree(&root_dir).expect("package tree should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_tree_json(&tree))
        .expect("package tree JSON should parse");
    let human = rsscript::format_package_tree_human(&tree);
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(json["root"]["name"], "rss-json");
    assert_eq!(json["summary"]["packages"], 3);
    assert_eq!(json["summary"]["path_dependencies"], 1);
    assert_eq!(json["summary"]["unresolved_dependencies"], 1);
    assert_eq!(json["summary"]["native_packages"], 1);
    assert!(json["root"]["dependencies"].as_array().is_some_and(|deps| {
        deps.iter().any(|dep| {
            dep["name"] == "rss-dep"
                && dep["version"] == "0.2.0"
                && dep["features"][0] == "streaming"
                && dep["native"] == true
        }) && deps
            .iter()
            .any(|dep| dep["name"] == "rss-remote" && dep["risk"] == "unknown")
    }));
    assert!(human.contains("|-- rss-dep 0.2.0 [elevated, native, features streaming]"));
    assert!(human.contains("`-- rss-remote req 0.5 [unknown]"));
}

#[test]
fn rss_package_tree_json_reports_dependency_summary() {
    let root_dir = unique_temp_dir("rsscript-package-tree-cli-root");
    let dep_dir = unique_temp_dir("rsscript-package-tree-cli-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("tree")
        .arg("--json")
        .arg(&root_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package tree should execute");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package tree JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["summary"]["packages"], 2);
    assert_eq!(json["summary"]["path_dependencies"], 1);
    assert_eq!(json["root"]["dependencies"][0]["name"], "rss-dep");
}

#[test]
fn package_publish_dry_run_reports_ready_package() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-ready");
    write_named_package_fixture(
        &temp_dir,
        "rss-ready",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::create_dir_all(temp_dir.join("target/debug")).expect("target dir should be created");
    fs::write(temp_dir.join("target/debug/junk"), "do not publish")
        .expect("target file should be written");
    fs::create_dir_all(temp_dir.join("review")).expect("review dir should be created");
    fs::write(temp_dir.join("review/package-review.json"), "{}\n")
        .expect("generated review metadata should be written");

    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let publish_again =
        publish_package_dry_run(&temp_dir).expect("publish dry-run should be deterministic");
    let registry_dir = temp_dir.join("registry");
    let publish_with_registry =
        rsscript::publish_package_dry_run_with_registry(&temp_dir, Some(&registry_dir))
            .expect("publish dry-run should report registry paths");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);
    let archive_paths = publish
        .archive_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert!(publish.ready);
    assert_eq!(publish.archive_hash, publish_again.archive_hash);
    assert!(
        publish_with_registry
            .registry_target
            .as_ref()
            .is_some_and(|target| target.index_path.ends_with("index/rss-ready/0.1.0.json"))
    );
    assert!(
        publish_with_registry
            .registry_target
            .as_ref()
            .is_some_and(|target| target
                .archive_manifest_path
                .ends_with("archives/rss-ready/0.1.0/archive-manifest.json"))
    );
    assert_eq!(json["package"]["name"], "rss-ready");
    assert_eq!(json["registry_index"]["schema"], "rss.registry.index.v1");
    assert_eq!(json["registry_index"]["name"], "rss-ready");
    assert_eq!(json["registry_index"]["version"], "0.1.0");
    assert_eq!(json["registry_index"]["checksum"], json["archive_hash"]);
    assert_eq!(json["registry_index"]["risk"], "elevated");
    assert_eq!(json["registry_index"]["native"], false);
    assert_eq!(json["registry_index"]["unsafe"], false);
    assert!(
        json["registry_index"]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(
        json["registry_index"]["review_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert_eq!(json["archive_format"], "rss.package.archive.v1");
    assert!(
        json["archive_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(archive_paths.contains(&"rsspkg.toml"), "{archive_paths:?}");
    assert!(
        archive_paths.contains(&"interface/lib.rssi"),
        "{archive_paths:?}"
    );
    assert!(archive_paths.contains(&"rsspkg.lock"), "{archive_paths:?}");
    assert!(
        !archive_paths.iter().any(|path| path.starts_with("target/")),
        "{archive_paths:?}"
    );
    assert!(
        !archive_paths
            .iter()
            .any(|path| *path == "review/package-review.json"),
        "{archive_paths:?}"
    );
    assert!(json["archive_files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["path"] == "rsspkg.toml"
                && file["sha256"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("sha256:"))
        })
    }));
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["name"] == "package archive reproducible" && check["ok"] == true)
    }));
}

#[test]
fn package_publish_dry_run_blocks_unknown_review_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-unknown-risk");
    write_named_package_fixture(
        &temp_dir,
        "rss-unknown",
        "0.1.0",
        r#"[review]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!publish.ready);
    assert_eq!(json["risk"], "unknown");
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["name"] == "package review risk classified"
                && check["ok"] == false
                && check["risk"] == "unknown"
        })
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package review risk classified failed: review risk unknown")
    }));
}

#[test]
fn rss_package_publish_dry_run_reports_local_registry_target() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-registry-cli");
    let registry_dir = unique_temp_dir("rsscript-package-publish-registry-target");
    write_named_package_fixture(
        &temp_dir,
        "rss-registry",
        "0.1.0",
        "",
        r#"pub fn Registry.value() -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("publish")
        .arg("--dry-run")
        .arg("--json")
        .arg("--registry")
        .arg(&registry_dir)
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package publish should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be publish JSON");
    let index_written = registry_dir
        .join("index")
        .join("rss-registry")
        .join("0.1.0.json")
        .exists();
    let _ = fs::remove_dir_all(&temp_dir);
    let _ = fs::remove_dir_all(&registry_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(
        json["registry_target"]["index_path"]
            .as_str()
            .map(|path| path.ends_with("index/rss-registry/0.1.0.json")),
        Some(true)
    );
    assert_eq!(
        json["registry_target"]["archive_manifest_path"]
            .as_str()
            .map(|path| path.ends_with("archives/rss-registry/0.1.0/archive-manifest.json")),
        Some(true)
    );
    assert!(!index_written);
}

#[test]
fn package_publish_dry_run_reports_registry_index_dependencies() {
    let root_dir = unique_temp_dir("rsscript-package-publish-index-root");
    let dep_dir = unique_temp_dir("rsscript-package-publish-index-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.value() -> Int
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-index-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ version = "0.2.0", path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn Root.value() -> Int
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let publish = publish_package_dry_run(&root_dir).expect("publish dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(publish.ready);
    assert_eq!(json["registry_index"]["dependencies"]["rss-dep"], "0.2.0");
}

#[test]
fn rss_package_publish_dry_run_json_reports_unresolved_dependency() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-blocked");
    write_named_package_fixture(
        &temp_dir,
        "rss-blocked",
        "0.1.0",
        r#"[dependencies]
rss-remote = "0.5.0"
"#,
        r#"pub fn main() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("publish")
        .arg("--dry-run")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package publish should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be publish JSON");

    assert!(!output.status.success(), "stdout={stdout}");
    assert_eq!(json["ready"], false);
    assert_eq!(json["risk"], "unknown");
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["name"] == "dependency graph review" && check["ok"] == false)
    }));
}

#[test]
fn package_vendor_dry_run_reports_path_and_unresolved_dependencies() {
    let root_dir = unique_temp_dir("rsscript-package-vendor-root");
    let dep_dir = unique_temp_dir("rsscript-package-vendor-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
rss-remote = "0.5.0"
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let vendor =
        vendor_package_dir(&root_dir, true).expect("vendor dry-run should produce a report");
    let json: Value = serde_json::from_str(&rsscript::format_package_vendor_json(&vendor))
        .expect("vendor JSON should parse");
    let vendor_dir_exists = root_dir.join("vendor").exists();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!vendor.ok);
    assert!(!vendor_dir_exists);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["entries"][0]["name"], "rss-dep");
    assert_eq!(json["unresolved"][0]["name"], "rss-remote");
    assert_eq!(json["risk"], "unknown");
}

#[test]
fn rss_package_vendor_json_writes_vendor_directory_and_metadata() {
    let root_dir = unique_temp_dir("rsscript-package-vendor-cli-root");
    let dep_dir = unique_temp_dir("rsscript-package-vendor-cli-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("vendor")
        .arg("--json")
        .arg(&root_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package vendor should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be vendor JSON");
    let vendored_manifest = root_dir
        .join("vendor")
        .join("rss-dep-0.2.0")
        .join("rsspkg.toml");
    let vendor_metadata = root_dir.join("vendor").join("rss-vendor.json");
    let vendored_manifest_exists = vendored_manifest.exists();
    let vendor_metadata_exists = vendor_metadata.exists();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["entries"][0]["name"], "rss-dep");
    assert!(vendored_manifest_exists);
    assert!(vendor_metadata_exists);
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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn write_package_fixture(
    directory: &Path,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    write_named_package_fixture(
        directory,
        "rss-json",
        version,
        extra_manifest,
        interface_source,
    );
}

fn write_named_package_fixture(
    directory: &Path,
    name: &str,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    fs::create_dir_all(directory.join("interface")).expect("interface dir should be created");
    fs::write(
        directory.join("rsspkg.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
edition = "2026"

[interfaces]
paths = ["interface"]

{extra_manifest}
"#
        ),
    )
    .expect("package manifest should be written");
    fs::write(directory.join("interface/lib.rssi"), interface_source)
        .expect("interface should be written");
}

fn write_runtime_conflict_fixture(path: &Path) {
    fs::write(
        path,
        r#"features: local

fn main() -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read "db://local"),
        max_size: 0,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        Log.write(message: read "unreachable")
    }

    return Unit
}
"#,
    )
    .expect("runtime diagnostic fixture should be written");
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
