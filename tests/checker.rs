use rsscript::analyze_source;

fn codes(source: &str) -> Vec<String> {
    analyze_source("test.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn assert_ok(source: &str) {
    let diagnostics = analyze_source("test.rss", source);
    assert_eq!(diagnostics, Vec::new());
}

fn assert_has_code(source: &str, code: &str) {
    assert!(
        codes(source).contains(&code.to_string()),
        "expected diagnostic code {code}"
    );
}

#[test]
fn accepts_canonical_local_thumbnail_pipeline() {
    let source = r#"
mode: uses-local

struct Image {
    pixels: Buffer
}

fn make_thumbnail(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?

    Image.resize(image: mut image, width: 256, height: 256)
    Image.normalize(image: mut image)

    let shared = manage image

    Image.save(image: read shared, path: read output)?

    return Ok(Unit)
}
"#;

    assert_eq!(codes(source), Vec::<String>::new());
}

#[test]
fn accepts_file_read_write_with_resource() {
    let source = r#"
mode: managed

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

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

    assert_ok(source);
}

#[test]
fn accepts_json_parse_with_fresh_nested_struct_and_managed_field() {
    let source = r#"
mode: managed

class Schema {
    name: String
}

struct JsonValue {
    raw: String
}

struct UserConfig {
    schema: handle Schema
    json: JsonValue
    name: String
}

fn parse_user_config(text: read String, schema: read Schema) -> Result<fresh UserConfig, JsonError> {
    let json = Json.parse(text: read text)?
    let name = Json.field_string(value: read json, name: read "name")?

    return UserConfig(
        schema: read schema,
        json: read json,
        name: read name,
    )
}
"#;

    assert_ok(source);
}

#[test]
fn accepts_managed_http_server_handler_without_local() {
    let source = r#"
mode: managed

class AppState {
    cache: Cache
}

struct Response {
    status: Int
    body: String
}

fn handle_request(state: mut AppState, request: read Request) -> Result<fresh Response, HttpError> {
    let path = Request.path(request: read request)
    let body = Cache.lookup(cache: read state.cache, key: read path)

    return Response(status: 200, body: read body)
}
"#;

    assert_ok(source);
}

#[test]
fn accepts_image_pipeline_local_hot_path_with_manage_exit() {
    let source = r#"
mode: uses-local

struct Image {
    pixels: Buffer
}

fn process_upload(input: read Path, cache: mut ImageCache) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?

    Image.resize(image: mut image, width: 1280, height: 720)
    Image.normalize(image: mut image)
    Image.sharpen(image: mut image)

    let shared = manage image
    ImageCache.store(cache: mut cache, image: read shared)

    return Ok(Unit)
}
"#;

    assert_ok(source);
}

#[test]
fn lru_cache_retaining_managed_values_requires_manage_for_local_values() {
    let source = r#"
mode: uses-local

class LruCache {
    entries: Map<String, Image>
}

struct Image {
    pixels: Buffer
}

fn lru_put(cache: mut LruCache, key: read String, value: read Image) -> Unit
    effects(retains(value))
{
    Map.insert(map: mut cache.entries, key: read key, value: read value)
}

fn store_thumbnail(cache: mut LruCache, key: read String, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    lru_put(cache: mut cache, key: read key, value: read (manage image))
    Image.inspect(image: read image)
}
"#;

    assert_has_code(source, "RS0401");
}

#[test]
fn accepts_csv_stream_processing_with_reused_local_buffer() {
    let source = r#"
mode: uses-local

struct RowBuffer {
    bytes: Buffer
}

fn sum_csv(path: read Path) -> Result<Int, CsvError> {
    local buffer = RowBuffer.new(size: 4096)
    let total = 0

    with File.open_read(path: read path) as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
        let row = Csv.parse_row(buffer: read buffer)?
        Int.add(left: total, right: row.amount)
    }

    return Ok(total)
}
"#;

    assert_ok(source);
}

#[test]
fn accepts_database_connection_pool_with_resource_leases() {
    let source = r#"
mode: uses-local

resource DbConnection {
    fd: Int

    drop {
        Db.close(fd: fd)
    }
}

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 8,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;

    assert_ok(source);
}

#[test]
fn accepts_config_load_fresh_then_replace_managed_global() {
    let source = r#"
mode: managed

struct Rule {
    name: String
}

struct Config {
    name: String
    rules: handle List<Rule>
}

fn load_config(path: read Path) -> Result<fresh Config, ConfigError> {
    let rules = RuleLoader.load_rules(path: read path)?

    return Config(
        name: read "default",
        rules: read rules,
    )
}

fn reload_config(path: read Path, global: mut GlobalConfig) -> Result<Unit, ConfigError> {
    let next = load_config(path: read path)?
    GlobalConfig.replace(global: mut global, value: read next)

    return Ok(Unit)
}
"#;

    assert_ok(source);
}

#[test]
fn documents_single_isolate_counter_shape() {
    let source = r#"
mode: managed

class Counter {
    value: Int
}

fn increment(counter: mut Counter) -> Unit {
    Counter.add(counter: mut counter, amount: 1)

    return Unit
}

fn read_counter(counter: read Counter) -> Int {
    return Counter.value(counter: read counter)
}
"#;

    assert_ok(source);
}

#[test]
fn accepts_interpreter_graph_with_classes_and_cycles() {
    let source = r#"
mode: managed

class Environment {
    parent: Environment
    values: Map<String, Object>
}

class FunctionObject {
    closure: Environment
}

class Object {
    prototype: Object
}

fn make_function(env: read Environment) -> FunctionObject {
    let function = FunctionObject.new(closure: read env)

    return function
}
"#;

    assert_ok(source);
}

#[test]
fn reports_mode_and_call_style_violations() {
    let source = r#"
mode: managed

struct Image {
    pixels: Buffer
}

fn resize(image: mut Image, width: Int, height: Int) -> Unit {
    local image = Image.load(path: read "in.png")
    Image.resize(image: image, width: 800, height: 600)
    Image.save(read image, path: read "out.png")
}
"#;

    let result = codes(source);
    assert!(result.contains(&"RS0101".to_string()));
    assert!(result.contains(&"RS0201".to_string()));
    assert!(result.contains(&"RS0202".to_string()));
}

#[test]
fn reports_use_after_manage() {
    let source = r#"
mode: uses-local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    Image.save(image: read image, path: read path)
    Image.save(image: read shared, path: read path)
}
"#;

    assert!(codes(source).contains(&"RS0401".to_string()));
}

#[test]
fn reports_retaining_local_value() {
    let source = r#"
mode: uses-local

class Cache {
    entries: Map<String, Image>
}

struct Image {
    pixels: Buffer
}

fn cache_put(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))
{
}

fn run(cache: mut Cache, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    cache_put(cache: mut cache, value: read image)
}
"#;

    assert!(codes(source).contains(&"RS0501".to_string()));
}

#[test]
fn reports_resource_escape_and_resource_field() {
    let source = r#"
mode: managed

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Logger {
    file: File
}

fn leak(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        return file
    }
}
"#;

    let result = codes(source);
    assert!(result.contains(&"RS0701".to_string()));
    assert!(result.contains(&"RS0702".to_string()));
}

#[test]
fn reports_fresh_function_returning_managed_value() {
    let source = r#"
mode: managed

struct Image {
    pixels: Buffer
}

fn cached(cache: read Cache) -> fresh Image {
    let image = Cache.get(cache: read cache)
    return image
}
"#;

    assert!(codes(source).contains(&"RS0601".to_string()));
}
