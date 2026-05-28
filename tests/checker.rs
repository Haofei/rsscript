use std::fs;
use std::path::{Path, PathBuf};

use rsscript::syntax::ast::Item;
use rsscript::syntax::parse_source;
use rsscript::{
    ReviewMapClassification, ReviewRisk, analyze_source, analyze_source_with_core,
    analyze_source_with_interfaces, core_interfaces, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_json, format_review_human,
    format_review_json, format_review_map_human, format_review_map_json, lower_source_to_rust,
    lower_source_to_rust_package, lower_source_to_rust_with_map, remap_rustc_diagnostic_json,
    remap_rustc_diagnostic_json_lines, review_map_sources, review_sources,
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
    for path in fixture_paths("core") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn bundled_core_interfaces_are_available_to_checker() {
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/test/assert.rssi")
    );

    let source = r#"
fn check_label(actual: read String, expected: read String) -> Unit {
    Assert.equal(left: read actual, right: read expected)
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
mode: uses-local

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
mode: uses-local

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
mode: uses-local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn pooled(pool: mut ResourcePool<DbConnection>) -> Unit
"#;
    let rust = lower_source_to_rust("pool.rssi", source).expect("source should lower");

    assert!(rust.contains("impl rsscript_runtime::Resource for DbConnection"));
    assert!(rust.contains("pool: &mut rsscript_runtime::ResourcePool<DbConnection>"));
    assert!(rust.contains("let _ = &pool;"));
}

#[test]
fn rust_lowering_emits_source_spans_for_resource_pool_borrow() {
    let source = r#"
mode: uses-local

resource DbConnection {
    fd: Int
}

fn pooled(pool: mut ResourcePool<DbConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read "select 1")
    }
}
"#;
    let rust = lower_source_to_rust("pool.rss", source).expect("source should lower");

    assert!(rust.contains("let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(&mut pool, rsscript_runtime::SourceSpan::new(\"pool.rss\", 9, 10, 12)));"));
}

#[test]
fn rust_lowering_wraps_managed_class_returns_in_gc() {
    let source = r#"
mode: uses-local

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
    assert!(rust.contains("let mut session = Session { id: id };"));
    assert!(rust.contains("return rsscript_runtime::manage_at(session, rsscript_runtime::SourceSpan::new(\"session.rss\", 10, 12, 6));"));
}

#[test]
fn rust_lowering_maps_read_and_mut_effects_to_rust_borrows() {
    let source = r#"
mode: uses-local

struct Counter {
    value: Int
}

fn read_value(counter: read Counter) -> Int {
    return counter.value
}

fn touch(counter: mut Counter) -> Unit

pub fn run() -> Int {
    local counter = Counter(value: 1)
    touch(counter: mut counter)
    return read_value(counter: read counter)
}
"#;
    let rust = lower_source_to_rust("effects.rss", source).expect("source should lower");

    assert!(rust.contains("fn read_value(counter: &Counter) -> i64"));
    assert!(rust.contains("fn touch(counter: &mut Counter)"));
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
    let source_map: Value =
        serde_json::from_str(&package.source_map_json).expect("source map JSON should parse");
    assert!(source_map.as_array().is_some_and(|items| !items.is_empty()));
}

#[test]
fn rust_lowering_is_gated_by_diagnostics() {
    let source = r#"
mode: uses-local

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
mode: managed

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
mode: uses-local

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
            .any(|item| item["code"] == "RSR001" && item["risk"] == "mode")
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
mode: managed

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
mode: uses-local

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
            .any(|finding| finding.code == "RSR001" && finding.risk == ReviewRisk::Mode)
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
mode: managed

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"
mode: managed

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
mode: managed

fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"
mode: managed

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
mode: managed

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
mode: managed

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
mode: uses-local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    Image.inspect(image: read Image.load(path: read path))
}
"#;
    let new_source = r#"
mode: uses-local

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

pub fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
"#;
    let program = parse_source("json.rssi", source);

    assert!(program.mode.is_none());
    assert_eq!(program.items.len(), 3);
    assert!(matches!(&program.items[0], Item::Type(type_decl) if type_decl.name == "JsonValue"));
    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "parse" && function.is_public && function.body.statements.is_empty())
    );
    assert!(
        matches!(&program.items[2], Item::Function(function) if function.name == "field_string" && function.is_public && function.body.statements.is_empty())
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
