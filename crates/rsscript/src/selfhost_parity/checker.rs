// ---------------------------------------------------------------------------
// Phase 3 — checker parity (semantic diagnostics).
//
// The rss checker (`selfhost/check.rss`) reproduces a chosen subset of analyzer
// diagnostics and prints the codes it finds (one per line, or `CLEAN`). Oracle:
// the real analyzer `crate::analyze_source`, filtered to the same target codes.
// We start with RS0005 (DUPLICATE_DECLARATION — duplicate top-level item names
// and duplicate struct/sum fields), decidable from declaration structure alone
// (no expression/statement parsing needed; see SH-021).
// ---------------------------------------------------------------------------

/// Dev-loop optimization: extra target codes from `RSS_CHECKER_EXTRA_CODES`
/// (comma-separated) are unioned into the target set at runtime.
/// `SELFHOST_CHECKER_TARGET_CODES` is compiled, so adding a code to it forces a
/// rebuild; `check.rss` is read from disk at runtime. While developing a new
/// code, wire it into `check.rss` and run
/// `RSS_CHECKER_EXTRA_CODES=RS0XXX cargo test … checker_parity_corpus` to
/// iterate without baking it into the target table yet.
fn extra_target_codes() -> &'static Vec<String> {
    static EXTRA: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    EXTRA.get_or_init(|| {
        std::env::var("RSS_CHECKER_EXTRA_CODES")
            .map(|s| {
                s.split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn is_target_code(code: &str) -> bool {
    SELFHOST_CHECKER_TARGET_CODES.contains(&code) || extra_target_codes().iter().any(|c| c == code)
}

#[test]
fn checker_target_codes_are_known_and_unique() {
    let mut seen = BTreeSet::new();
    for code in SELFHOST_CHECKER_TARGET_CODES {
        assert!(
            code.starts_with("RS") && code.len() == 6,
            "self-host checker target code must be an RS diagnostic code: {code}"
        );
        assert!(
            seen.insert(*code),
            "self-host checker target code must not be duplicated: {code}"
        );
    }
}

/// Oracle: the set of target diagnostic codes the real analyzer reports.
fn checker_oracle_codes(file: &str, source: &str) -> Vec<String> {
    let mut codes: Vec<String> = analyze_source(file, source)
        .into_iter()
        .filter(|d| d.severity == Severity::Error && is_target_code(&d.code))
        .map(|d| d.code)
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SelfhostDiagnosticRecord {
    code: String,
    line: usize,
    column: usize,
    length: usize,
}

fn checker_oracle_records(
    file: &str,
    source: &str,
    target_code: &str,
) -> Vec<SelfhostDiagnosticRecord> {
    let mut records = analyze_source(file, source)
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error && diagnostic.code == target_code
        })
        .map(|diagnostic| SelfhostDiagnosticRecord {
            code: diagnostic.code,
            line: diagnostic.span.line,
            column: diagnostic.span.column,
            length: diagnostic.span.length,
        })
        .collect::<Vec<_>>();
    records.sort();
    records
}

fn diagnostic_records_for_code(
    records: Vec<SelfhostDiagnosticRecord>,
    target_code: &str,
) -> Vec<SelfhostDiagnosticRecord> {
    records
        .into_iter()
        .filter(|record| record.code == target_code)
        .collect()
}

fn compile_checker() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("check.rss", "checker")
}

#[test]
fn checker_module_loading_is_transitive_and_unique() {
    let sources = tool_sources("check.rss").expect("checker modules should load");
    let paths = sources
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(paths.len(), sources.len(), "checker modules must load once");
    for (path, source) in &sources {
        for import in selfhost_imports(path, source) {
            let imported_path = format!("selfhost/{import}");
            assert!(
                paths.contains(imported_path.as_str()),
                "{path} import {import} was not loaded"
            );
        }
    }
    for expected in [
        "selfhost/checker/support.rss",
        "selfhost/checker/output.rss",
        "selfhost/checker/type_model.rss",
        "selfhost/checker/diagnostics/syntax_declarations.rss",
        "selfhost/checker/diagnostics/effects_calls.rss",
        "selfhost/scan.rss",
        "selfhost/semantics/check_types.rss",
        "selfhost/semantics/check_bindings.rss",
        "selfhost/semantics/check_calls.rss",
    ] {
        assert!(paths.contains(expected), "checker did not load {expected}");
    }
}

/// Run the rss checker; parse the target codes it reports (`CLEAN` => none).
fn run_checker(exe: &RegVmExecutable, source: &str) -> Result<Vec<String>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss checker failed to run: {e:?}"))?;
    parse_checker_output(&output.stdout)
}

fn parse_checker_output(stdout: &str) -> Result<Vec<String>, String> {
    let mut codes = Vec::new();
    let mut clean_count = 0usize;
    for line in stdout.lines() {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if code == "CLEAN" {
            clean_count += 1;
        } else if is_target_code(code) {
            codes.push(code.to_string());
        } else {
            return Err(format!(
                "rss checker emitted an unknown diagnostic line: {line:?}"
            ));
        }
    }
    if clean_count > 1 {
        return Err("rss checker emitted duplicate CLEAN verdicts".to_string());
    }
    if clean_count == 1 && !codes.is_empty() {
        return Err("rss checker emitted CLEAN together with diagnostics".to_string());
    }
    if clean_count == 0 && codes.is_empty() {
        return Err("rss checker emitted no verdict".to_string());
    }
    codes.sort();
    codes.dedup();
    Ok(codes)
}

fn parse_checker_records(stdout: &str) -> Result<Vec<SelfhostDiagnosticRecord>, String> {
    let mut records = Vec::new();
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.as_slice() == ["CLEAN"] {
        return Ok(Vec::new());
    }
    if lines.contains(&"CLEAN") {
        return Err("rss checker emitted CLEAN together with structured diagnostics".to_string());
    }
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [code, line, column, length] = fields.as_slice() else {
            return Err(format!("malformed structured diagnostic: {line:?}"));
        };
        if !is_target_code(code) {
            return Err(format!("unknown structured diagnostic code: {code:?}"));
        }
        let parse_number = |name: &str, value: &str| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid diagnostic {name}: {value:?}"))
        };
        records.push(SelfhostDiagnosticRecord {
            code: (*code).to_string(),
            line: parse_number("line", line)?,
            column: parse_number("column", column)?,
            length: parse_number("length", length)?,
        });
    }
    if records.is_empty() {
        return Err("rss checker emitted no structured diagnostics".to_string());
    }
    records.sort();
    Ok(records)
}

type CheckerWorkerResponse = Result<String, String>;
type CheckerWorkerRequest = (String, std::sync::mpsc::Sender<CheckerWorkerResponse>);

struct CheckerWorkerPool {
    workers: Vec<std::sync::mpsc::Sender<CheckerWorkerRequest>>,
    next: std::sync::atomic::AtomicUsize,
}

impl CheckerWorkerPool {
    fn start() -> Self {
        let worker_count = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, 4);
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (request_tx, request_rx) = std::sync::mpsc::channel::<CheckerWorkerRequest>();
            std::thread::Builder::new()
                .name(format!("selfhost-checker-{index}"))
                .spawn(move || {
                    let checker = compile_checker();
                    for (source, response_tx) in request_rx {
                        let response = match &checker {
                            Ok(exe) => exe
                                .eval_main_with_args([source, "records".to_string()])
                                .map(|output| output.stdout)
                                .map_err(|e| {
                                    format!("rss structured checker failed to run: {e:?}")
                                }),
                            Err(error) => Err(error.clone()),
                        };
                        let _ = response_tx.send(response);
                    }
                })
                .expect("self-host checker worker should start");
            workers.push(request_tx);
        }
        Self {
            workers,
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn send(&self, request: CheckerWorkerRequest) -> Result<(), String> {
        let index =
            self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.workers.len();
        self.workers[index]
            .send(request)
            .map_err(|_| "self-host checker worker stopped".to_string())
    }
}

/// Compile a small pool of self-hosted checkers and keep every executable on
/// its owning worker thread. `RegVmExecutable` owns an `Rc<RegUnit>`, so it
/// cannot live in a process-global `Sync` cache or cross thread boundaries.
fn run_cached_checker_records(source: &str) -> Result<Vec<SelfhostDiagnosticRecord>, String> {
    static WORKERS: std::sync::OnceLock<CheckerWorkerPool> = std::sync::OnceLock::new();
    let workers = WORKERS.get_or_init(CheckerWorkerPool::start);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    workers.send((source.to_string(), response_tx))?;
    let stdout = response_rx
        .recv()
        .map_err(|_| "self-host checker worker returned no result".to_string())??;
    parse_checker_records(&stdout)
}

#[test]
fn checker_output_parser_rejects_unknown_lines() {
    assert_eq!(
        parse_checker_output("RS0005\nRS0207\n").unwrap(),
        vec!["RS0005".to_string(), "RS0207".to_string()]
    );
    assert!(parse_checker_output("debug\n").is_err());
    assert!(parse_checker_output("CLEAN\nRS0005\n").is_err());
    assert!(parse_checker_output("").is_err());
    assert!(parse_checker_output("  \n\t\n").is_err());
    assert!(parse_checker_output("CLEAN\nCLEAN\n").is_err());
}

#[test]
fn checker_record_parser_is_strict_and_preserves_duplicates() {
    let records = parse_checker_records("RS0005\t2\t1\t2\nRS0005\t2\t1\t2\n")
        .expect("valid records should parse");
    assert_eq!(
        records.len(),
        2,
        "structured parity must retain occurrences"
    );
    assert_eq!(parse_checker_records("CLEAN\n").unwrap(), Vec::new());
    assert!(parse_checker_records("").is_err());
    assert!(parse_checker_records("CLEAN\nRS0005\t2\t1\t2\n").is_err());
    assert!(parse_checker_records("RS0005\t2\t1\n").is_err());
    assert!(parse_checker_records("RS0005\ttwo\t1\t2\n").is_err());
    assert!(parse_checker_records("RS9999\t2\t1\t2\n").is_err());
}

#[test]
fn checker_preserves_raw_diagnostic_family_order() {
    let source = "fn broken(value)\n    effects(mystery)\n{\n    return Unit\n}\n";
    let checker = compile_checker().expect("checker should compile");
    let output = checker
        .eval_main_with_args([source.to_string()])
        .expect("checker should run");
    let jit_output = checker
        .eval_main_with_args_jit([source.to_string()])
        .expect("checker should run under the JIT");

    assert_eq!(output.stdout, "RS0002\nRS0003\nRS0004\n");
    assert_eq!(jit_output.stdout, output.stdout);
}

#[test]
fn checker_structured_clean_verdict() {
    let source = "fn clean(value: Int) -> Int {\n    return value\n}\n";
    let actual = run_cached_checker_records(source).expect("rss checker should emit records");
    assert!(
        actual.is_empty(),
        "clean source emitted records: {actual:?}"
    );
}

#[test]
fn checker_rs0005_structured_multiset_parity() {
    let source = r#"struct Response {
    status: Int
    status: String
}

struct Response {
    body: String
}

fn render() -> Unit {
    return Unit
}

fn render() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0005.rss", source, "RS0005");
    assert!(
        oracle.len() > 1,
        "fixture must exercise duplicate occurrences"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0005",
    );
    assert_eq!(oracle, actual, "RS0005 structured diagnostics diverged");
}

#[test]
fn checker_rs0002_structured_multiset_parity() {
    let source = r#"fn first() {
    return Unit
}

fn second() {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0002.rss", source, "RS0002");
    assert_eq!(oracle.len(), 2, "fixture must exercise both functions");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0002",
    );
    assert_eq!(oracle, actual, "RS0002 structured diagnostics diverged");
}

#[test]
fn checker_rs0003_structured_multiset_parity() {
    let source = r#"fn combine(first, second, typed: Int) -> Unit {
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0003.rss", source, "RS0003");
    assert_eq!(oracle.len(), 2, "fixture must exercise both parameters");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0003",
    );
    assert_eq!(oracle, actual, "RS0003 structured diagnostics diverged");
}

#[test]
fn checker_rs0004_structured_multiset_parity() {
    let source = r#"fn work() -> Unit
    effects(mystery, fresh)
{
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0004.rss", source, "RS0004");
    assert_eq!(oracle.len(), 2, "fixture must exercise both effects");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0004",
    );
    assert_eq!(oracle, actual, "RS0004 structured diagnostics diverged");
}

#[test]
fn checker_rs0004_retains_effect_is_not_unknown() {
    let source = r#"fn retain(image: read Image) -> Unit
    effects(retains(image))
{
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0004-retains.rss", source, "RS0004");
    assert!(
        oracle.is_empty(),
        "retains must not be an RS0004 effect item"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0004",
    );
    assert_eq!(oracle, actual, "retains effect classification diverged");
}

#[test]
fn checker_rs0006_structured_multiset_parity() {
    let source = "features: local\nfeatures: async\nfeatures: native\n";
    let oracle = checker_oracle_records("structured-rs0006.rss", source, "RS0006");
    assert_eq!(oracle.len(), 2, "fixture must exercise both extra headers");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0006",
    );
    assert_eq!(oracle, actual, "RS0006 structured diagnostics diverged");
}

#[test]
fn checker_rs0009_structured_multiset_parity() {
    let source = r#"features: local

resource File {
    fd: Int

    drop {
        Log.write(message: read "close")
    }
}

struct Image {
    value: Int
}

fn helper() -> Unit {
    return Unit
}

fn inspect(
    changed: mut Image,
    consumed: take Image,
    first: read String,
    second: read String,
    file: read File
) -> File
    effects(pure, retains(first), retains(second))
{
    with file as opened {
        helper()
    }
    local image = Image(value: 1)
    let shared = manage image
    return File(fd: 1)
}
"#;
    let oracle = checker_oracle_records("structured-rs0009.rss", source, "RS0009");
    assert_eq!(
        oracle.len(),
        8,
        "fixture must preserve every pure signature and body violation"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0009",
    );
    assert_eq!(oracle, actual, "RS0009 structured diagnostics diverged");
}

#[test]
fn checker_rs0007_structured_multiset_parity() {
    let source = r#"fn sample(count: Int, text: read String) -> Unit
    effects(retains(count), retains(missing))
{
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0007.rss", source, "RS0007");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both retains failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0007",
    );
    assert_eq!(oracle, actual, "RS0007 structured diagnostics diverged");
}

#[test]
fn checker_rs0010_structured_multiset_parity() {
    let source = "profile: managed\nprofile: managed\n";
    let oracle = checker_oracle_records("structured-rs0010.rss", source, "RS0010");
    assert_eq!(oracle.len(), 2, "fixture must exercise both profiles");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0010",
    );
    assert_eq!(oracle, actual, "RS0010 structured diagnostics diverged");
}

#[test]
fn checker_rs0012_structured_multiset_parity() {
    let source = r#"fn work() -> Unit
    effects(io, may_panic)
{
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0012.rss", source, "RS0012");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both removed effects"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0012",
    );
    assert_eq!(oracle, actual, "RS0012 structured diagnostics diverged");
}

#[test]
fn checker_rs0013_structured_multiset_parity() {
    let source = r#"struct Image {
    value: Int
}

struct Config {
    value: Int
}

struct ConfigError {
    code: Int
}

struct AppError {
    code: Int
}

fn load_image() -> Image {
    return Image(value: 1)
}

fn load_config() -> Result<Config, ConfigError> {
    return Ok(Config(value: 1))
}

fn load_app() -> Result<Config, AppError> {
    return Ok(Config(value: 1))
}

fn scalar() -> Int {
    let first = load_image()?
    let second = load_image()?
    return 0
}

fn bad_value() -> Result<Image, AppError> {
    let image = load_image()?
    return Ok(image)
}

fn bad_error() -> Result<Config, AppError> {
    let config = load_config()?
    return Ok(config)
}

fn valid() -> Result<Config, AppError> {
    let config = load_app()?
    return Ok(config)
}
"#;
    let oracle = checker_oracle_records("structured-rs0013.rss", source, "RS0013");
    assert_eq!(
        oracle.len(),
        6,
        "fixture must preserve duplicate-span return/value failures and error mismatch"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0013",
    );
    assert_eq!(oracle, actual, "RS0013 structured diagnostics diverged");
}

#[test]
fn checker_rs0014_structured_multiset_parity() {
    let source = r#"features: local

struct Pair {
    left: Int
    right: Int
}

fn build() -> Pair
    effects(noalloc)
{
    local first = Pair(left: 1, right: 2)
    local second = Pair(left: 3, right: 4)
    return manage first
}
"#;
    let oracle = checker_oracle_records("structured-rs0014.rss", source, "RS0014");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve constructor and manage allocation sites"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0014",
    );
    assert_eq!(oracle, actual, "RS0014 structured diagnostics diverged");
}

#[test]
fn checker_rs0018_structured_multiset_parity() {
    let source = r#"fn may_block(value: Int) -> Int {
    return value
}

fn safe(value: Int) -> Int
    effects(no_block)
{
    return value
}

fn promised(value: Int) -> Int
    effects(no_block)
{
    let first = may_block(value: value)
    let second = may_block(value: first)
    return safe(value: second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0018.rss", source, "RS0018");
    assert_eq!(oracle.len(), 2, "fixture must preserve both blocking calls");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0018",
    );
    assert_eq!(oracle, actual, "RS0018 structured diagnostics diverged");
}

#[test]
fn checker_rs0019_structured_multiset_parity() {
    let source = r#"fn may_panic(value: Int) -> Int {
    return value
}

fn safe(value: Int) -> Int
    effects(no_panic)
{
    return value
}

fn promised(value: Int) -> Int
    effects(no_panic)
{
    let first = may_panic(value: value)
    let second = may_panic(value: first)
    return safe(value: second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0019.rss", source, "RS0019");
    assert_eq!(oracle.len(), 2, "fixture must preserve both panic calls");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0019",
    );
    assert_eq!(oracle, actual, "RS0019 structured diagnostics diverged");
}

#[test]
fn checker_rs0020_structured_multiset_parity() {
    let source = r#"sum Choice {
    One
}

struct Boxed {
    value: Int
}

fn first(value: read Int) -> Int {
    return value
}

fn second(value: read Int) -> Int {
    return value
}

fn allowed(value: read Int) -> Int effects(noalloc) {
    return value
}

fn Host.bad(value: read Int) -> Int {
    return value
}

fn Host.allowed(value: read Int) -> Int effects(noalloc) {
    return value
}

fn exercise(value: read Int) -> Int effects(noalloc) {
    let a = first(value: read value)
    let b = second(value: read a)
    let c = Host.bad(value: read b)
    let d = allowed(value: read c)
    let e = Host.allowed(value: read d)
    let variant = One
    let boxed = Boxed(value: e)
    return boxed.value
}
"#;
    let oracle = checker_oracle_records("structured-rs0020.rss", source, "RS0020");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve simple and qualified calls while exempting noalloc/variant/constructor"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0020",
    );
    assert_eq!(oracle, actual, "RS0020 structured diagnostics diverged");
}

#[test]
fn checker_rs0021_structured_multiset_parity() {
    let source = r#"fn statement_bad(value: read Option<Int>) -> Int {
    match value {
        Some(item) => return item
    }
}

fn expression_bad(name: read String) -> String {
    return match name {
        "read" => { "value" }
    }
}

fn exhaustive(value: read Option<Int>) -> Int {
    return match value {
        Some(item) => { item }
        None => { 0 }
    }
}

fn bool_bad(value: read Bool) -> Int {
    return match value {
        true => { 1 }
    }
}

fn result_bad(value: read Result<Int, String>) -> Int {
    return match value {
        Ok(item) => { item }
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0021.rss", source, "RS0021");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must preserve Option, String, Bool, and Result mismatches while exempting exhaustive match"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0021",
    );
    assert_eq!(oracle, actual, "RS0021 structured diagnostics diverged");
}

#[test]
fn checker_rs0016_structured_multiset_parity() {
    let source = "features: mystery, other\n";
    let oracle = checker_oracle_records("structured-rs0016.rss", source, "RS0016");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both unknown features"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0016",
    );
    assert_eq!(oracle, actual, "RS0016 structured diagnostics diverged");
}

#[test]
fn checker_rs0017_structured_multiset_parity() {
    let source = "features: local, local, local\n";
    let oracle = checker_oracle_records("structured-rs0017.rss", source, "RS0017");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both duplicate features"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0017",
    );
    assert_eq!(oracle, actual, "RS0017 structured diagnostics diverged");
}

#[test]
fn checker_rs0011_structured_multiset_parity() {
    let source = r#"fn old(first: share String, second: share List<Int>) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0011.rss", source, "RS0011");
    assert_eq!(oracle.len(), 2, "fixture must exercise both share types");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0011",
    );
    assert_eq!(oracle, actual, "RS0011 structured diagnostics diverged");
}

#[test]
fn checker_rs0028_structured_multiset_parity() {
    let source = r#"fn wrong(self: read String, other: Int, self: read String) -> Unit {
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0028.rss", source, "RS0028");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both self parameters"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0028",
    );
    assert_eq!(oracle, actual, "RS0028 structured diagnostics diverged");
}

#[test]
fn checker_protocol_contract_structured_multiset_parity() {
    let source = r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct Buffer {}

fn Buffer.write(self: mut Buffer, message: read String) -> Unit {
    return Unit
}

fn Buffer.bad_write(self: mut Buffer, message: read String) -> Int {
    return 0
}

impl Writer for Buffer {
    write = Buffer.bad_write
}
"#;
    let oracle = checker_oracle_records("structured-protocol-contract.rss", source, "RS1301");
    assert_eq!(oracle.len(), 1, "fixture must exercise a mapping mismatch");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1301",
    );
    assert_eq!(oracle, actual, "protocol contract diagnostics diverged");
}

#[test]
fn checker_protocol_method_contract_structured_multiset_parity() {
    let source = r#"protocol Broken {
    fn missing_receiver() -> Unit

    fn has_body(self: read Self) -> Unit {
        return Unit
    }

    fn default_impl(self: read Self) -> Unit = _
}
"#;
    // A protocol declaration may use `= _`; only its method body is invalid.
    for (code, expected) in [("RS0015", 1), ("RS0028", 1)] {
        let oracle =
            checker_oracle_records("structured-protocol-method-contract.rss", source, code);
        assert_eq!(oracle.len(), expected, "fixture must exercise {code}");
        let actual = diagnostic_records_for_code(
            run_cached_checker_records(source).expect("rss checker should emit records"),
            code,
        );
        assert_eq!(
            oracle, actual,
            "protocol method {code} diagnostics diverged"
        );
    }
}

#[test]
fn checker_rs0029_structured_multiset_parity() {
    let source = r#"features: async

async fn fetch(value: read Int) -> Int {
    return value
}

fn exercise(value: read Int) -> Unit {
    let first = await fetch(value: read value)
    let second = await fetch(value: read first)
}

async fn valid(value: read Int) -> Int {
    return await fetch(value: read value)
}
"#;
    let oracle = checker_oracle_records("structured-rs0029.rss", source, "RS0029");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve two invalid awaits and exempt async functions"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0029",
    );
    assert_eq!(oracle, actual, "RS0029 structured diagnostics diverged");
}

#[test]
fn checker_rs0022_structured_multiset_parity() {
    let source = r#"features: async

async fn fetch(value: read Int) -> Int {
    return value
}

async fn exercise(value: read Int) -> Unit {
    let first = fetch(value: read value)
    fetch(value: read first)
    let consumed = await fetch(value: read value)
}
"#;
    let oracle = checker_oracle_records("structured-rs0022.rss", source, "RS0022");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve unconsumed calls and exempt await"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0022",
    );
    assert_eq!(oracle, actual, "RS0022 structured diagnostics diverged");
}

#[test]
fn checker_task_group_async_context_structured_multiset_parity() {
    let source = include_str!("../../tests/fixtures/pass/task-group-basic.rss");
    for code in ["RS0022", "RS0029"] {
        let oracle = checker_oracle_records("task-group-basic.rss", source, code);
        assert!(
            oracle.is_empty(),
            "task-group fixture must be valid for {code}"
        );
        let actual = diagnostic_records_for_code(
            run_cached_checker_records(source).expect("rss checker should emit records"),
            code,
        );
        assert_eq!(oracle, actual, "task-group {code} diagnostics diverged");
    }
}

#[test]
fn checker_rs0023_structured_multiset_parity() {
    let source = r#"struct BadHandle {
    input: Fd
    output: Fd
}

resource AllowedHandle {
    fd: Fd

    drop {
        OS.close(fd: fd)
    }
}

native fn allowed(fd: Fd) -> Fd

fn exposed(first: Fd, second: Fd) -> Fd {
    return first
}
"#;
    let oracle = checker_oracle_records("structured-rs0023.rss", source, "RS0023");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve fields, parameters, and return surface failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0023",
    );
    assert_eq!(oracle, actual, "RS0023 structured diagnostics diverged");
}

#[test]
fn checker_rs0024_structured_multiset_parity() {
    let source = r#"struct Holder<T> {
    first: Missing
    second: List<Other>
    callback: Fn(Arg, T) -> ReturnMissing
}

fn exercise<T>(
    first: read UnknownParam,
    second: read Map<String, NestedUnknown>,
    known: read T,
    holder: read Holder<T>
) -> Result<UnknownReturn, ErrorUnknown> {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0024.rss", source, "RS0024");
    assert_eq!(
        oracle.len(),
        8,
        "fixture must preserve every unknown root while exempting generic and declared types"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0024",
    );
    assert_eq!(oracle, actual, "RS0024 structured diagnostics diverged");
}

#[test]
fn checker_rs0027_structured_multiset_parity() {
    let source = r#"struct Box<T: MissingBoxProtocol> {
    value: T
}

fn combine<A: MissingLeft, B: MissingRight>(left: read A, right: read B) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0027.rss", source, "RS0027");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve type and function generic-bound failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0027",
    );
    assert_eq!(oracle, actual, "RS0027 structured diagnostics diverged");
}

#[test]
fn checker_rs0032_structured_multiset_parity() {
    let source = r#"features: local

struct Plain {
    value: Int
}

struct Hashable derives(Hash) {
    value: Int
}

struct Ordered derives(Ord) {
    value: Int
}

fn exercise(values: mut List<Plain>, ordered: mut List<Ordered>) -> Unit {
    let bad_set = Set.new<Plain>()
    let bad_map = Map.new<Plain, Int>()
    List.sort<Plain>(list: mut values)
    let good_set = Set.new<Hashable>()
    List.sort<Ordered>(list: mut ordered)
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0032.rss", source, "RS0032");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve Hashable/Ord failures and exempt derived implementations"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(oracle, actual, "RS0032 structured diagnostics diverged");
}

#[test]
fn checker_rs0032_map_receiver_protocol_parity() {
    let source = include_str!("../../tests/fixtures/pass/hashable-struct-map-key.rss");
    let oracle = checker_oracle_records("hashable-struct-map-key.rss", source, "RS0032");
    assert!(
        oracle.is_empty(),
        "valid Map receivers must remain protocol-clean"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(oracle, actual, "Map receiver RS0032 diagnostics diverged");
}

#[test]
fn checker_rs0032_map_field_receiver_protocol_parity() {
    let source = include_str!("../../tests/fixtures/pass/receiver-call-basic.rss");
    let oracle = checker_oracle_records("receiver-call-basic.rss", source, "RS0032");
    assert!(
        oracle.is_empty(),
        "valid Map field receivers must remain protocol-clean"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(
        oracle, actual,
        "Map field receiver RS0032 diagnostics diverged"
    );
}

#[test]
fn checker_rs0032_set_receiver_protocol_parity() {
    let source = include_str!("../../tests/fixtures/pass/hashable-struct-set.rss");
    let oracle = checker_oracle_records("hashable-struct-set.rss", source, "RS0032");
    assert!(
        oracle.is_empty(),
        "valid Set receivers must remain protocol-clean"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(oracle, actual, "Set receiver RS0032 diagnostics diverged");
}

#[test]
fn checker_rs0032_explicit_collection_method_type_args_are_exempt() {
    let source = r#"fn main() -> Unit {
    let mut seen = Set<String>.new()
    Set.insert<String>(set: mut seen, value: "entry")
    return Unit
}
"#;
    let oracle = checker_oracle_records("rs0032-explicit-method-type-args.rss", source, "RS0032");
    assert!(
        oracle.is_empty(),
        "fixture must remain protocol-clean in Rust"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(
        oracle, actual,
        "explicit collection method type args must not emit RS0032"
    );
}

#[test]
fn checker_rs0033_structured_multiset_parity() {
    let source = r#"fn first() -> Int {
    return 9223372036854775808
}

fn second() -> Int {
    return 999999999999999999999999999999
}
"#;
    let oracle = checker_oracle_records("structured-rs0033.rss", source, "RS0033");
    assert_eq!(oracle.len(), 2, "fixture must exercise both integers");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0033",
    );
    assert_eq!(oracle, actual, "RS0033 structured diagnostics diverged");
}

#[test]
fn checker_rs0034_structured_multiset_parity() {
    let source = r#"fn main() -> Unit {
    let first = Ok(1)
    let second = Err("error")
    let third = None
    let used = Ok(2)
    let annotated: Result<Int, String> = Ok(3)
    let determined = Some(4)
    Log.debug(value: read used)
}
"#;
    let oracle = checker_oracle_records("structured-rs0034.rss", source, "RS0034");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise bare Ok, Err, and None"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0034",
    );
    assert_eq!(oracle, actual, "RS0034 structured diagnostics diverged");
}

#[test]
fn checker_rs0205_structured_multiset_parity() {
    let source = r#"struct Pair {
    first: Int
    second: Int
}

sum Duo {
    Values(first: Int, second: Int)
}

class Left {}
class Right {}

fn target(a: Int, b: Int) -> Unit
fn Left.run(self: read Left, value: Int) -> Unit
fn Right.run(self: read Right, value: Int) -> Unit

fn exercise(left: read Left) -> Unit {
    target(a: 1, a: 2, a: 3, b: 4)
    target(a: 1, b: 2, b: 3)
    target(a: 1, b: 2)
    Pair(first: 1, first: 2, second: 3)
    Values(first: 1, second: 2, second: 3)
    left.run(value: 1, value: 2)
    missing(value: 1, value: 2)
}
"#;
    let oracle = checker_oracle_records("structured-rs0205.rss", source, "RS0205");
    assert_eq!(
        oracle.len(),
        6,
        "fixture must preserve resolved, ambiguous receiver, and unresolved-call duplicates"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0205",
    );
    assert_eq!(oracle, actual, "RS0205 structured diagnostics diverged");
}

#[test]
fn checker_rs0208_structured_multiset_parity() {
    let source = r#"class BuildError {
    code: Int
}

fn nested() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}

fn direct() -> String {
    return 42
}

fn fallthrough() -> String {
    42
}
"#;
    let oracle = checker_oracle_records("structured-rs0208.rss", source, "RS0208");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise nested payload, direct return, and fallthrough anchors"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0208",
    );
    assert_eq!(oracle, actual, "RS0208 structured diagnostics diverged");
}

#[test]
fn checker_rs0210_structured_multiset_parity() {
    let source = r#"fn apply(callback: noescape Fn(Int) -> Bool) -> Unit {
    return Unit
}

fn direct() -> Unit {
    if 1 == "one" {
        return Unit
    }
    return Unit
}

fn callback() -> Unit {
    apply(callback: |value| value == "text")
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0210.rss", source, "RS0210");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise ordinary and callback operator spans"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0210",
    );
    assert_eq!(oracle, actual, "RS0210 structured diagnostics diverged");
}

#[test]
fn checker_rs0209_structured_multiset_parity() {
    let source = r#"fn maybe() -> Option<Int> {
    return Some(1)
}

fn conditions(value: read String) -> Unit {
    if value {
        return Unit
    }
    for item in value {
        Log.write(message: read "item")
    }
    return Unit
}

fn patterns() -> Unit {
    let value = maybe()
    match value {
        Ok(result) => return Unit
        Err(error) => return Unit
    }
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0209.rss", source, "RS0209");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise condition, iterable, and both variant pattern occurrences"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0209",
    );
    assert_eq!(oracle, actual, "RS0209 structured diagnostics diverged");
}

#[test]
fn checker_rs0202_structured_multiset_parity() {
    let source = r#"sum Expr {
    Call(callee: String)
}

struct Item {
    name: String
}

struct Boxed {
    item: Item
}

fn Item.new() -> fresh Item {
    return Item(name: "item")
}

fn use_item(value: read Item) -> Unit {
    return Unit
}

fn Item.touch(self: mut Item, value: read String) -> Unit {
    return Unit
}

fn bad(expr: read Expr) -> Unit {
    let item = Item.new()
    use_item(value: item)
    let boxed = Boxed(item: read item)
    read item.touch(value: read "name")
    match expr {
        Call { callee } => return Unit
    }
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0202.rss", source, "RS0202");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise constructor, receiver, and match-effect spans; bare read is valid"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0202",
    );
    assert_eq!(oracle, actual, "RS0202 structured diagnostics diverged");
}

#[test]
fn checker_rs0207_structured_multiset_parity() {
    let source = r#"fn needs_text(value: read String) -> Unit {
    return Unit
}

fn bad() -> Unit {
    let value: String = 42
    needs_text(value: read 7)
    Log.write(message: read 9)
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0207.rss", source, "RS0207");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise annotated binding, same-file, and stdlib call argument anchors"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "RS0207 structured diagnostics diverged");
}

#[test]
fn checker_semantic_index_call_contract_parity() {
    let source = r#"struct Item {
    name: String
}

fn apply(left: mut Item, right: read Item, label: read String) -> Unit {
    return Unit
}

fn inspect(value: Item) -> Unit {
    return Unit
}

fn inspect_float(value: Float) -> Unit {
    return Unit
}

fn Item.touch(self: mut Item, value: read Item) -> Unit {
    return Unit
}

fn check(a: read Item, b: read Item) -> Unit {
    inspect(value: a)
    inspect_float(value: 1.5)
    apply(label: read 7, right: mut b, left: read a)
    read a.touch(value: mut b)
    return Unit
}
"#;
    for (code, expected_count) in [("RS0202", 4), ("RS0207", 1)] {
        let oracle = checker_oracle_records("semantic-index-call-contracts.rss", source, code);
        assert_eq!(
            oracle.len(),
            expected_count,
            "fixture no longer exercises the intended {code} contract cases"
        );
        let actual = diagnostic_records_for_code(
            run_cached_checker_records(source).expect("rss checker should emit records"),
            code,
        );
        assert_eq!(
            oracle, actual,
            "{code} SemanticIndex call-contract diagnostics diverged"
        );
    }
}

#[test]
fn checker_semantic_index_generic_call_type_parity() {
    let cases = [
        (
            "generic-list-argument.rss",
            r#"fn accept(values: read List<String>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let values = List.new<Int>()
    accept(values: read values)
    return Unit
}
"#,
        ),
        (
            "inferred-generic-argument.rss",
            r#"fn same<T>(left: read T, right: read T) -> Unit {
    return Unit
}

fn main() -> Unit {
    same(left: read "first", right: read 2)
    return Unit
}
"#,
        ),
        (
            "explicit-generic-argument.rss",
            r#"fn accept<T>(value: read T) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept<Int>(value: read "wrong")
    return Unit
}
"#,
        ),
        (
            "alias-call-argument.rss",
            r#"type Identifier = Int

fn accept(value: read Identifier) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept(value: read "wrong")
    return Unit
}
"#,
        ),
        (
            "generic-alias-call-argument.rss",
            r#"type Values<T> = List<T>

fn accept(values: read Values<String>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let values = List.new<Int>()
    accept(values: read values)
    return Unit
}
"#,
        ),
        (
            "nested-generic-return-argument.rss",
            r#"fn identity<T>(value: read T) -> T {
    return value
}

fn accept(value: read Int) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept(value: read identity(value: read "wrong"))
    return Unit
}
"#,
        ),
        (
            "generic-receiver-argument.rss",
            r#"struct Box<T> {
    value: T
}

fn Box.set<T>(self: mut Box<T>, value: read T) -> Unit {
    self.value = value
    return Unit
}

fn main() -> Unit {
    local box = Box<Int>(value: 1)
    mut box.set(value: read "wrong")
    return Unit
}
"#,
        ),
        (
            "function-value-argument.rss",
            r#"fn invoke(callback: noescape Fn(Int) -> Int) -> Int {
    return callback("wrong")
}
"#,
        ),
    ];

    for (file, source) in cases {
        let oracle = checker_oracle_records(file, source, "RS0207");
        assert_eq!(
            oracle.len(),
            1,
            "{file} must exercise one generic-aware call mismatch"
        );
        let actual = diagnostic_records_for_code(
            run_cached_checker_records(source).expect("rss checker should emit records"),
            "RS0207",
        );
        assert_eq!(oracle, actual, "{file} generic call parity diverged");
    }
}

#[test]
fn checker_semantic_index_concrete_single_letter_type_parity() {
    let source = r#"struct T {
    value: Int
}

fn accept(value: read T) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept(value: read "wrong")
    return Unit
}
"#;
    let oracle = checker_oracle_records("concrete-single-letter-type.rss", source, "RS0207");
    assert_eq!(
        oracle.len(),
        1,
        "concrete `T` must not be treated as generic"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(
        oracle, actual,
        "concrete single-letter type parity diverged"
    );
}

#[test]
fn checker_semantic_index_nested_and_receiver_alias_parity() {
    let valid_nested_alias = r#"type Identifier = Int

fn accept(values: read List<Identifier>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let values = List.new<Int>()
    accept(values: read values)
    return Unit
}
"#;
    let oracle = checker_oracle_records("nested-alias-valid.rss", valid_nested_alias, "RS0207");
    assert!(oracle.is_empty(), "nested aliases must be transparent");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(valid_nested_alias).expect("rss checker should run"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "nested alias parity diverged");

    let receiver_alias = r#"struct Box<T> {
    value: T
}

type IntBox = Box<Int>

fn Box.set<T>(self: mut Box<T>, value: read T) -> Unit {
    self.value = value
    return Unit
}

fn check(box: mut IntBox) -> Unit {
    mut box.set(value: read "wrong")
    return Unit
}

fn main() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("receiver-alias-generic.rss", receiver_alias, "RS0207");
    assert_eq!(
        oracle.len(),
        1,
        "receiver alias must constrain the method generic"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(receiver_alias).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "receiver alias generic parity diverged");
}

#[test]
fn checker_semantic_index_generated_generic_call_presence_parity() {
    let source = r#"fn main() -> Unit {
    local values = List.new<Int>()
    List.push(list: mut values, value: read "wrong")
    return Unit
}
"#;
    let oracle = checker_oracle_records("generated-generic-argument.rss", source, "RS0207");
    assert_eq!(
        oracle.len(),
        1,
        "fixture must exercise one generated generic signature mismatch"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(
        actual.len(),
        oracle.len(),
        "generated generic call presence parity diverged"
    );
}

#[test]
fn checker_semantic_index_generic_ownership_is_fail_closed() {
    let path = workspace_root().join("benchmarks/vm-jit/kernels/deepcopy_read_param.rss");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let oracle = checker_oracle_records(
        "benchmarks/vm-jit/kernels/deepcopy_read_param.rss",
        &source,
        "RS0207",
    );
    assert!(
        oracle.is_empty(),
        "fail-closed fixture must remain valid in the Rust checker"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(&source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert!(
        actual.is_empty(),
        "incomplete field inference must not become an RS0207 false positive: {actual:?}"
    );
}

#[test]
fn checker_rs0207_callback_fallback_survives_semantic_migration() {
    let source = r#"fn need(value: read String) -> Int {
    return 0
}

fn apply(callback: noescape Fn(Int) -> Int) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: |value| need(value: read value))
    return Unit
}
"#;
    let oracle = checker_oracle_records("rs0207-callback-fallback.rss", source, "RS0207");
    assert!(
        !oracle.is_empty(),
        "fixture must exercise contextual callback-body checking"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert!(
        !actual.is_empty(),
        "RS0207 contextual callback fallback disappeared during migration"
    );
}

#[test]
fn checker_rs0035_structured_multiset_parity() {
    let source = r#"#lower_name("dup_symbol")
fn first() -> Unit {
    return Unit
}

#lower_name("dup_symbol")
fn second() -> Unit {
    return Unit
}

#lower_name("has-a-dash")
fn invalid() -> Unit {
    return Unit
}

#lower_name("plain")
fn pinned_plain() -> Unit {
    return Unit
}

fn plain() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0035.rss", source, "RS0035");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve pin collision, invalid pin, and pinned/default collision"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0035",
    );
    assert_eq!(oracle, actual, "RS0035 structured diagnostics diverged");
}

#[test]
fn checker_rs0036_structured_multiset_parity() {
    let source = r#"features: async, native, local

async fn exercise() -> Result<Unit, ChannelError> {
    let first = Channel.message<List<Int>>(capacity: 4)?
    let second = Channel.message<Map<String, Int>>(capacity: 4)?
    let valid = Channel.message<String>(capacity: 4)?
    return Ok(Unit)
}

async fn generic<T>() -> Result<Unit, ChannelError> {
    let unresolved = Channel.message<T>(capacity: 4)?
    return Ok(Unit)
}
"#;
    let oracle = checker_oracle_records("structured-rs0036.rss", source, "RS0036");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve two non-transferable calls and exempt transferable/generic payloads"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0036",
    );
    assert_eq!(oracle, actual, "RS0036 structured diagnostics diverged");
}

#[test]
fn checker_rs0037_structured_multiset_parity() {
    let source = r#"sum Pairish {
    Pair(left: Int, right: Int)
    Empty
}

fn inspect(value: read Pairish) -> Int {
    match value {
        Pair(one) => return one
        Pair(one, two, three) => return one
        Pair(left, right) => return left + right
        Empty => return 0
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0037.rss", source, "RS0037");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve too-few and too-many positional bindings"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0037",
    );
    assert_eq!(oracle, actual, "RS0037 structured diagnostics diverged");
}

#[test]
fn checker_rs0101_structured_body_parity() {
    let source = r#"async fn fetch() -> Int {
    return 1
}

fn dangerous() -> Unit
    effects(unsafe)
{
    return Unit
}

fn exercise() -> Unit {
    local value = 1
    let managed = manage value
    let first = await fetch()
    spawn fetch()
    dangerous()
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-body.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        9,
        "fixture must preserve nested local, async, and unsafe feature uses"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 body diagnostics diverged");
}

#[test]
fn checker_rs0101_structured_qualified_call_parity() {
    let source = r#"async fn Worker.fetch<T>(value: read T) -> T {
    return value
}

fn Host.danger() -> Unit effects(unsafe) {
    return Unit
}

fn exercise() -> Unit {
    let value = await Worker.fetch<Int>(value: read 1)
    Host.danger()
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-qualified.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve qualified declarations, await/call, and unsafe call uses"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 qualified-call diagnostics diverged");
}

#[test]
fn checker_rs0201_structured_multiset_parity() {
    let source = r#"pub fn publish(first: Int, second: Int) -> Unit {
    return Unit
}

fn exercise() -> Unit {
    publish(1, 2)
    publish(first: 3, 4)
    publish(first: 5, second: 6)
}
"#;
    let oracle = checker_oracle_records("structured-rs0201.rss", source, "RS0201");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise each unnamed argument"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0201",
    );
    assert_eq!(oracle, actual, "RS0201 structured diagnostics diverged");
}

#[test]
fn checker_rs0201_accepts_default_read_name_puns() {
    let source = r#"pub fn forward(value: String, suffix: String) -> String {
    return String.concat(left: value, right: suffix)
}

fn exercise(value: String, suffix: String, fill: String) -> String {
    let joined = forward(value, suffix)
    return String.pad_left(value: joined, width: 12, fill)
}
"#;
    let oracle = checker_oracle_records("rs0201-default-read-pun.rss", source, "RS0201");
    assert!(
        oracle.is_empty(),
        "same-name default-read puns must be accepted"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0201",
    );
    assert_eq!(oracle, actual, "RS0201 default-read pun handling diverged");
}

#[test]
fn checker_rs0212_structured_multiset_parity() {
    let source = r#"resource Connection derives(Clone, Eq, Hash) {
    id: Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0212.rss", source, "RS0212");
    assert_eq!(oracle.len(), 3, "fixture must exercise all banned derives");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0212",
    );
    assert_eq!(oracle, actual, "RS0212 structured diagnostics diverged");
}

#[test]
fn checker_rs0211_structured_multiset_parity() {
    let source = r#"class Entity {
    value: Int
}

struct Bad derives(Eq, Hash) {
    score: Float
    target: handle Entity
}

struct DecodeBad derives(JsonDecode) {
    values: Map<Float, Int>
}

struct Good derives(Eq, Hash) {
    value: Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0211.rss", source, "RS0211");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve per-field/per-derive violations and valid scalar fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0211",
    );
    assert_eq!(oracle, actual, "RS0211 structured diagnostics diverged");
}

#[test]
fn checker_rs0701_structured_multiset_parity() {
    let source = r#"resource Connection {
    id: Int
}

struct Holder {
    primary: Connection
    backup: Connection
}
"#;
    let oracle = checker_oracle_records("structured-rs0701.rss", source, "RS0701");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both resource fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0701",
    );
    assert_eq!(oracle, actual, "RS0701 structured diagnostics diverged");
}

#[test]
fn checker_rs0704_structured_multiset_parity() {
    let source = r#"resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Archive {
    files: List<File>
    backups: Option<File>
}

resource Unbounded<T> {
    id: Int

    drop {
        OS.close()
    }
}

resource Direct<T: Resource> {
    item: T

    drop {
        OS.close()
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0704.rss", source, "RS0704");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise resource arguments and declaration constraints"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0704",
    );
    assert_eq!(oracle, actual, "RS0704 structured diagnostics diverged");
}

#[test]
fn checker_rs0706_structured_multiset_parity() {
    let source = r#"features: local

resource File {
    fd: Int
}

fn File.open_result(path: read Path) -> Result<File, IOError>
fn File.stat(file: read File) -> Unit

fn missing_first(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}

fn valid(path: read Path) -> Result<Unit, IOError> {
    with File.open_result(path: read path)? as file {
        File.stat(file: read file)
    }
    return Ok(Unit)
}

fn missing_second(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0706.rss", source, "RS0706");
    assert_eq!(oracle.len(), 2, "fixture must exercise both missing tries");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0706",
    );
    assert_eq!(oracle, actual, "RS0706 structured diagnostics diverged");
}

#[test]
fn checker_rs0805_structured_multiset_parity() {
    let source = r#"features: local

fn mismatch() -> Int {
    let mut count = 0
    local bump = fn() captures(read count) effects(pure) {
        count = count + 1
        return count
    }
    return bump()
}

fn missing() -> Int {
    let offset = 2
    local add = fn(value) captures() effects(pure) {
        return value + offset
    }
    return add(40)
}

fn unused() -> Int {
    let offset = 2
    local identity = fn(value) captures(read offset) effects(pure) {
        return value
    }
    return identity(40)
}

fn stronger_is_valid() -> Int {
    let offset = 2
    local add = fn(value) captures(take offset) effects(pure) {
        return value + offset
    }
    return add(40)
}
"#;
    let oracle = checker_oracle_records("structured-rs0805.rss", source, "RS0805");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve mismatch, missing, and unused capture diagnostics"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0805",
    );
    assert_eq!(oracle, actual, "RS0805 structured diagnostics diverged");
}

#[test]
fn checker_closure_capture_structured_multiset_parity() {
    let source = r#"features: local

struct Image { id: Int }
class Scheduler

fn Image.inspect(image: read Image) -> Unit
fn schedule(scheduler: mut Scheduler, callback: read Fn()) -> Unit
    effects(retains(callback))
fn apply(callback: noescape Fn()) -> Unit {
    callback()
}
fn consume(image: take Image) -> Unit

fn managed() -> Unit {
    local image = Image(id: 1)
    let callback = || {
        Image.inspect(image: read image)
        Image.inspect(image: read image)
    }
}

fn retained(scheduler: mut Scheduler) -> Unit {
    local image = Image(id: 2)
    schedule(scheduler: mut scheduler, callback: read || {
        Image.inspect(image: read image)
    })
}

fn consuming() -> Unit {
    local first = Image(id: 3)
    local second = Image(id: 4)
    apply(callback: || {
        consume(image: take first)
        consume(image: take second)
    })
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for (code, expected) in [("RS0801", 3), ("RS0804", 2)] {
        let oracle = checker_oracle_records("structured-closure-capture.rss", source, code);
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_closure_escape_structured_multiset_parity() {
    let source = r#"features: local

struct Callback

fn store(callback: read Callback) -> Unit

fn invalid_signature(callback: noescape Fn()) -> Unit
    effects(retains(callback))
{
    callback()
}

fn noescape_escapes(callback: noescape Fn()) -> Fn {
    let stored = callback
    store(callback: read callback)
    let wrapper = || {
        callback()
    }
    return callback
}

fn local_escapes() -> Callback {
    local callback = || {
        return Unit
    }
    let stored = callback
    store(callback: read callback)
    return callback
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for (code, expected) in [("RS0802", 6), ("RS0803", 4)] {
        let oracle = checker_oracle_records("structured-closure-escape.rss", source, code);
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_rs0902_structured_multiset_parity() {
    let source = r#"struct Value {
    id: Int
}

struct Holder {
    first: weak Value
    second: weak Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0902.rss", source, "RS0902");
    assert_eq!(oracle.len(), 2, "fixture must exercise both weak fields");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0902",
    );
    assert_eq!(oracle, actual, "RS0902 structured diagnostics diverged");
}

#[test]
fn checker_rs0903_structured_multiset_parity() {
    let source = r#"class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn User.log(user: read User) -> Unit
fn User.rename(user: mut User) -> Unit

fn invalid(session: read Session) -> Unit {
    User.log(user: read session.owner)
    User.rename(user: mut session.owner)
}

fn valid(session: read Session) -> Option<User> {
    return Weak.upgrade(value: read session.owner)
}
"#;
    let oracle = checker_oracle_records("structured-rs0903.rss", source, "RS0903");
    assert_eq!(oracle.len(), 2, "fixture must exercise read and mut uses");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0903",
    );
    assert_eq!(oracle, actual, "RS0903 structured diagnostics diverged");
}

#[test]
fn checker_rs0904_structured_multiset_parity() {
    let source = r#"class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn invalid(first: read User, second: read User) -> Unit {
    let a = Session(owner: read first)
    let b = Session(owner: read second)
}

fn valid(user: read User) -> Unit {
    let a = Session(owner: Weak.from(value: read user))
    let b = Session(owner: Weak.downgrade(value: read user))
}
"#;
    let oracle = checker_oracle_records("structured-rs0904.rss", source, "RS0904");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both bad initializers"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0904",
    );
    assert_eq!(oracle, actual, "RS0904 structured diagnostics diverged");
}

#[test]
fn checker_rs0901_structured_multiset_parity() {
    let source = r#"features: local

struct Rule {
    name: String
}

struct Config {
    first: handle List<Rule>
    second: handle List<Rule>
    owned: List<Rule>
}

fn consume(config: mut Config) -> Unit {
    List.consume(list: take config.first)
    List.consume(list: take config.owned)
    List.consume(list: take config.second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0901.rss", source, "RS0901");
    assert_eq!(oracle.len(), 2, "fixture must exercise both handle fields");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0901",
    );
    assert_eq!(oracle, actual, "RS0901 structured diagnostics diverged");
}

#[test]
fn checker_rs1003_structured_multiset_parity() {
    let source = "own struct First\nown struct Second\n";
    let oracle = checker_oracle_records("structured-rs1003.rss", source, "RS1003");
    assert_eq!(oracle.len(), 2, "fixture must exercise both own structs");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1003",
    );
    assert_eq!(oracle, actual, "RS1003 structured diagnostics diverged");
}

#[test]
fn checker_rs0306_structured_multiset_parity() {
    let source = r#"features: local

class Session

fn create() -> Unit {
    local first = Session()
    local second = Session()
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0306.rss", source, "RS0306");
    assert_eq!(oracle.len(), 2, "fixture must exercise both local bindings");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0306",
    );
    assert_eq!(oracle, actual, "RS0306 structured diagnostics diverged");
}

#[test]
fn checker_rs0307_structured_multiset_parity() {
    let source = r#"features: local

class User {
    id: Int
}

struct Frame {
    id: Int
}

fn invalid(user: read User) -> Unit {
    let managed = User(id: 1)
    let first = manage user
    let second = manage managed
}

fn valid() -> Unit {
    local frame = Frame(id: 1)
    let promoted = manage frame
}
"#;
    let oracle = checker_oracle_records("structured-rs0307.rss", source, "RS0307");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise parameter and let values"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0307",
    );
    assert_eq!(oracle, actual, "RS0307 structured diagnostics diverged");
}

#[test]
fn checker_rs0308_structured_multiset_parity() {
    let source = r#"features: local

struct Buffer {
    id: Int
}

fn Buffer.consume(buffer: take Buffer) -> Unit

fn invalid(buffer: read Buffer) -> Unit {
    let managed = Buffer(id: 1)
    Buffer.consume(buffer: take buffer)
    Buffer.consume(buffer: take managed)
}

fn valid(owned: take Buffer) -> Unit {
    local direct = Buffer(id: 1)
    Buffer.consume(buffer: take direct)
    Buffer.consume(buffer: take owned)
}
"#;
    let oracle = checker_oracle_records("structured-rs0308.rss", source, "RS0308");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise parameter and let values"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0308",
    );
    assert_eq!(oracle, actual, "RS0308 structured diagnostics diverged");
}

#[test]
fn checker_rs0301_structured_multiset_parity() {
    let source = r#"features: local

struct Rules {
    id: Int
}

struct Holder {
    rules: handle Rules
}

fn Holder.create() -> fresh Holder

fn invalid() -> Unit {
    let shared = Rules(id: 1)
    local first = read shared
    local second = read Some(shared)
    local holder = Holder.create()
    local third = read holder.rules
    local fourth = read Ok(holder.rules)
}

fn valid() -> Unit {
    local owned = Rules(id: 1)
    local copy = read owned
    local number = 1
}
"#;
    let oracle = checker_oracle_records("structured-rs0301.rss", source, "RS0301");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise managed values, wrappers, and handle fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0301",
    );
    assert_eq!(oracle, actual, "RS0301 structured diagnostics diverged");
}

#[test]
fn checker_place_conflicts_structured_multiset_parity() {
    let source = r#"features: local

struct Cache { value: Int }
struct Inner { cache: Cache }
struct State { inner: Inner }
struct Buffer { value: Int }
struct LocalVec { length: Int }
struct Workspace { id: Int }
struct Config { workspace: Workspace }
struct SplitState { cache: Cache buffer: Buffer }

fn use_state(state: read State, cache: mut Cache) -> Unit
fn use_inner(inner: mut Inner, cache: mut Cache) -> Unit
fn use_buffers(a: mut Buffer, b: mut Buffer) -> Unit
fn use_config(config: take Config, workspace: read Workspace) -> Unit
fn use_parts(cache: mut Cache, buffer: mut Buffer) -> Unit
fn make_state() -> fresh State
fn make_buffers() -> fresh LocalVec
fn make_config() -> fresh Config

fn whole_base() -> Unit {
    local state = make_state()
    use_state(state: read state, cache: mut state.inner.cache)
}

fn prefix() -> Unit {
    local state = make_state()
    use_inner(inner: mut state.inner, cache: mut state.inner.cache)
    use_inner(inner: mut state.inner, cache: mut state.inner.cache)
}

fn indexed() -> Unit {
    local buffers = make_buffers()
    use_buffers(a: mut buffers[0], b: mut buffers[1])
}

fn moved() -> Unit {
    local config = make_config()
    use_config(config: take config, workspace: read config.workspace)
}

fn managed_split(state: mut SplitState) -> Unit {
    use_parts(cache: mut state.cache, buffer: mut state.buffer)
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for code in ["RS0302", "RS0303", "RS0304", "RS0305", "RS0309"] {
        let oracle = checker_oracle_records("structured-place-conflicts.rss", source, code);
        let expected = if code == "RS0303" { 2 } else { 1 };
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_rs0401_structured_multiset_parity() {
    let source = r#"features: local

struct Frame {
    id: Int
}

fn consume(value: take Frame) -> Unit

fn invalid_direct() -> Unit {
    local value = Frame(id: 1)
    consume(value: take value)
    Log.write(message: read "moved")
    let first = value.id
    let second = value.id
}

fn invalid_field() -> Unit {
    local holder = Frame(id: 2)
    let moved = manage holder.id
    let later = holder.id
}

fn valid() -> Unit {
    local value = Frame(id: 3)
    consume(value: take value)
    local value = Frame(id: 4)
    let id = value.id
}
"#;
    let oracle = checker_oracle_records("structured-rs0401.rss", source, "RS0401");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise repeated direct uses, a field path, and a rebound name"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0401",
    );
    assert_eq!(oracle, actual, "RS0401 structured diagnostics diverged");
}

#[test]
fn checker_rs0501_structured_multiset_parity() {
    let source = r#"features: local

struct Item {
    id: Int
}

fn Cache.store(value: read Item) -> Unit
    effects(retains(value))

fn Cache.store_option(value: read Option<Item>) -> Unit
    effects(retains(value))

fn exercise() -> Unit {
    local first = Item(id: 1)
    local second = Item(id: 2)
    let managed = Item(id: 3)
    Cache.store(value: read first)
    Cache.store(value: read second)
    Cache.store_option(value: read Some(first))
    Cache.store(value: read managed)
}
"#;
    let oracle = checker_oracle_records("structured-rs0501.rss", source, "RS0501");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise direct, repeated, and wrapped local retention"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0501",
    );
    assert_eq!(oracle, actual, "RS0501 structured diagnostics diverged");
}

#[test]
fn checker_rs0601_structured_multiset_parity() {
    let source = r#"features: local

struct Boxed {
    value: Int
}

struct Holder {
    boxed: handle Boxed
}

fn Holder.create() -> fresh Holder

fn bad_direct(value: read Boxed) -> fresh Boxed {
    return value
}

fn bad_wrapper(value: read Boxed) -> Option<fresh Boxed> {
    return Some(value)
}

fn bad_field() -> fresh Boxed {
    local holder = Holder.create()
    return holder.boxed
}
"#;
    let oracle = checker_oracle_records("structured-rs0601.rss", source, "RS0601");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise identifier, wrapper, and field-expression spans"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0601",
    );
    assert_eq!(oracle, actual, "RS0601 structured diagnostics diverged");
}

#[test]
fn checker_rs0604_structured_multiset_parity() {
    let source = r#"features: local

struct Image {
    width: Int
}

fn Image.load(width: read Int) -> fresh Image
fn mutate(image: mut Image) -> Unit
fn consume(image: take Image) -> Unit

fn exercise() -> Unit {
    mutate(image: mut Image.load(width: read 1))
    consume(image: take Image.load(width: read 2))
    local image = Image.load(width: read 3)
    mutate(image: mut image)
}
"#;
    let oracle = checker_oracle_records("structured-rs0604.rss", source, "RS0604");
    assert_eq!(oracle.len(), 2, "fixture must exercise mut and take ranges");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0604",
    );
    assert_eq!(oracle, actual, "RS0604 structured diagnostics diverged");
}

#[test]
fn checker_rs0603_structured_multiset_parity() {
    let source = r#"class User {
    name: String
}

class BuildError {
    code: Int
}

fn direct() -> fresh User
fn nested() -> Result<fresh User, BuildError>

fn generic<T>() -> fresh T {
    return value
}
"#;
    let oracle = checker_oracle_records("structured-rs0603.rss", source, "RS0603");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise direct, nested, and generic fresh targets"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0603",
    );
    assert_eq!(oracle, actual, "RS0603 structured diagnostics diverged");
}

#[test]
fn checker_rs0311_structured_multiset_parity() {
    let source = r#"features: local

struct State {
    value: Int
}

fn invalid(state: mut State, borrowed: read State) -> Unit {
    let count = 0
    count = 1
    count = 2
    state = State(value: 3)
    borrowed = State(value: 4)
}

fn valid(value: mut Int) -> Unit {
    let mut count = 0
    count = 1
    value = 2
}
"#;
    let oracle = checker_oracle_records("structured-rs0311.rss", source, "RS0311");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise repeated local and parameter assignments"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0311",
    );
    assert_eq!(oracle, actual, "RS0311 structured diagnostics diverged");
}

#[test]
fn checker_rs0313_structured_multiset_parity() {
    let source = r#"fn exercise() -> Unit {
    let mut count: Int = 0
    let mut label: String = "start"
    let mut enabled: Bool = false
    count = "wrong"
    label = true
    enabled = 1
    count = 2
    label = "valid"
    enabled = false
}
"#;
    let oracle = checker_oracle_records("structured-rs0313.rss", source, "RS0313");
    assert_eq!(oracle.len(), 2, "fixture must exercise scalar mismatches");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0313",
    );
    assert_eq!(oracle, actual, "RS0313 structured diagnostics diverged");
}

#[test]
fn checker_rs0312_structured_multiset_parity() {
    let source = r#"fn exercise() -> Unit {
    let mut values = Map<String, Int>.new()
    let mut queue = Deque<Int>.new()
    let mut items = List<Int>.new()
    values["first"] = 1
    values["second"] = 2
    queue[0] = 3
    items[0] = 4
}
"#;
    let oracle = checker_oracle_records("structured-rs0312.rss", source, "RS0312");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise repeated Map and Deque index assignments"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0312",
    );
    assert_eq!(oracle, actual, "RS0312 structured diagnostics diverged");
}

#[test]
fn checker_rs1002_structured_multiset_parity() {
    let source = r#"fn convert(value: Int) -> Int {
    let first = value as String
    let second = value as Float
    return value
}
"#;
    let oracle = checker_oracle_records("structured-rs1002.rss", source, "RS1002");
    assert_eq!(oracle.len(), 2, "fixture must exercise both conversions");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1002",
    );
    assert_eq!(oracle, actual, "RS1002 structured diagnostics diverged");
}

#[test]
fn checker_rs1001_structured_multiset_parity() {
    let source = r#"struct Point {
    x: Int
}

fn invalid(right: read Point) -> Unit {
    let first = Point + right
    let second = "left" - "right"
}

fn valid(left: Int, right: Int) -> Unit {
    let sum = left + right
    let shifted = left << 1
}
"#;
    let oracle = checker_oracle_records("structured-rs1001.rss", source, "RS1001");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both overload attempts"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1001",
    );
    assert_eq!(oracle, actual, "RS1001 structured diagnostics diverged");
}

#[test]
fn checker_rs1004_structured_multiset_parity() {
    let source = r#"fn first(value: &Int) -> Int {
    return 0
}

fn second() -> &String {
    return ""
}
"#;
    let oracle = checker_oracle_records("structured-rs1004.rss", source, "RS1004");
    assert_eq!(oracle.len(), 2, "fixture must exercise both references");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1004",
    );
    assert_eq!(oracle, actual, "RS1004 structured diagnostics diverged");
}

#[test]
fn checker_rs0039_structured_generic_alias_cycle_parity() {
    let source = r#"type A<T> = B<List<T>>
type B<T> = A<List<T>>
"#;
    let oracle = checker_oracle_records("structured-rs0039.rss", source, "RS0039");
    assert_eq!(oracle.len(), 2, "both aliases participate in the cycle");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0039",
    );
    assert_eq!(oracle, actual, "RS0039 structured diagnostics diverged");
}

#[test]
fn checker_rs0207_structured_generic_alias_substitution_parity() {
    let source = r#"type Boxed<T> = List<T>

fn consume(values: read Boxed<String>) -> Unit {
    return Unit
}

fn bad(values: read List<Int>) -> Unit {
    consume(values: read values)
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-generic-alias-rs0207.rss", source, "RS0207");
    assert_eq!(oracle.len(), 1, "fixture must mismatch the substituted T");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "generic alias RS0207 diagnostics diverged");
}

#[test]
fn checker_nested_alias_arguments_match_rust_oracle() {
    let source = r#"type Scalar = Int
type Boxed<T> = List<T>

fn consume(values: read List<Int>) -> Unit {
    return Unit
}

fn main(values: read Boxed<Scalar>) -> Unit {
    consume(values: read values)
    return Unit
}
"#;
    let oracle = checker_oracle_records("nested-alias-args.rss", source, "RS0207");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "nested alias arguments diverged");
}

#[test]
fn checker_bounded_generic_alias_parameters_match_rust_oracle() {
    let source = r#"struct Item {
    id: Int
}

type Boxed<T: Struct> = List<T>

fn consume(items: read List<Item>) -> Unit {
    return Unit
}

fn main(items: read Boxed<Item>) -> Unit {
    consume(items: read items)
    return Unit
}
"#;
    let oracle = checker_oracle_records("bounded-generic-alias.rss", source, "RS0207");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "bounded generic alias diverged");
}

#[test]
fn type_helpers_detect_prefixed_and_late_generic_args() {
    let mut sources = tool_sources("types.rss").expect("selfhost types deps should load");
    sources.push((
        "selfhost/type_helpers_test.rss".to_string(),
        r#"
module selfhost.type_helpers_test

use selfhost.types.*

fn main() -> Unit {
    if str_is_unresolved_generic(s: read "owned T") {
        Log.write(message: read "owned")
    }
    if str_is_unresolved_generic(s: read "Triple<Int, Int, T>") {
        Log.write(message: read "third")
    }
    let mut substitutions = Map<String, String>.new()
    Map.insert(map: mut substitutions, key: "T", value: "String")
    if substitute_type_parameters(
        ty: "Result<List<T>, Int>",
        substitutions: substitutions,
        depth: 0
    ) == "Result<List<String>, Int>" {
        Log.write(message: read "substitute")
    }
    let mut genericNames = Set<String>.new()
    Set.insert(set: mut genericNames, value: "T")
    let mut inferred = Map<String, String>.new()
    collect_type_substitutions(
        pattern: "List<T>",
        actual: "List<Int>",
        genericNames: genericNames,
        substitutions: mut inferred,
        depth: 0
    )
    if Map.get_or_default(map: inferred, key: "T", default: "") == "Int" {
        Log.write(message: read "infer")
    }
}
"#
        .to_string(),
    ));
    let source_refs = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let exe = reg_vm_compile_sources(&source_refs).expect("type helper test should compile");
    let output = exe
        .eval_main_with_args(std::iter::empty::<String>())
        .expect("type helper test should run");
    assert_eq!(output.stdout.trim(), "owned\nthird\nsubstitute\ninfer");
}

/// Phase-3 proof: the rss checker agrees with the analyzer on a tiny sample
/// (no duplicates → both report no target codes).
#[test]
fn checker_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = checker_oracle_codes("samples/tiny.rss", &source);
    let exe = compile_checker().expect("rss checker should compile");
    let actual = run_checker(&exe, &source).expect("rss checker should run");
    assert_eq!(oracle, actual, "checker parity diverged on tiny sample");
}

/// Phase-3 POSITIVE smoke (non-ignored): the rss checker must REPORT RS0005 for a
/// duplicate declaration, matching the analyzer. The no-duplicate tiny sample
/// above would still pass if the rss checker degenerated to always printing
/// `CLEAN`; this closes that gap without the (ignored) full-corpus gate.
#[test]
fn checker_reports_rs0005_for_duplicate_declaration_smoke() {
    let source = "fn dup() -> Unit {\n    return Unit\n}\nfn dup() -> Unit {\n    return Unit\n}\n";
    let oracle = checker_oracle_codes("checker-duplicate.rss", source);
    assert!(
        oracle.contains(&"RS0005".to_string()),
        "oracle analyzer must report RS0005 for the duplicate declaration; got {oracle:?}"
    );
    let exe = compile_checker().expect("rss checker should compile");
    let actual = run_checker(&exe, source).expect("rss checker should run");
    assert_eq!(
        oracle, actual,
        "checker parity diverged on the duplicate-declaration smoke test"
    );
}

#[test]
fn checker_rs0015_edge_parity() {
    let root = workspace_root();
    let cases = [
        (
            "comparison-before-generic.rss",
            "features: local\n\
             fn f(limit: Int) -> Unit {\n\
                 let mut i = 0\n\
                 while i < limit {\n\
                     let values = List<Int>.new()\n\
                     i = i + 1\n\
                 }\n\
                 return Unit\n\
             }\n"
            .to_string(),
            false,
        ),
        (
            "native-function-body.rss",
            "features: native\n\
             pub native fn host() -> Unit {\n\
                 return Unit\n\
             }\n"
            .to_string(),
            true,
        ),
        (
            "hostile-malformed/unicode-bidi.rss",
            std::fs::read_to_string(
                root.join("crates/rsscript/tests/hostile-malformed/unicode-bidi.rss"),
            )
            .expect("unicode fixture should be readable"),
            true,
        ),
        (
            "hostile-malformed/unterminated-string.rss",
            std::fs::read_to_string(
                root.join("crates/rsscript/tests/hostile-malformed/unterminated-string.rss"),
            )
            .expect("unterminated-string fixture should be readable"),
            true,
        ),
        (
            "samples/ast/async_let.rss",
            std::fs::read_to_string(root.join("selfhost/samples/ast/async_let.rss"))
                .expect("async-let sample should be readable"),
            true,
        ),
    ];
    let exe = compile_checker().expect("rss checker should compile");
    for (file, source, expects_rs0015) in cases {
        let oracle = checker_oracle_codes(file, &source);
        let actual = run_checker(&exe, &source).expect("rss checker should run");
        assert_eq!(oracle, actual, "checker parity diverged for {file}");
        assert_eq!(
            oracle.contains(&"RS0015".to_string()),
            expects_rs0015,
            "unexpected Rust RS0015 result for {file}: {oracle:?}"
        );
    }
}

/// Phase-3 gate (ignored by default): the rss checker's target-code diagnostics
/// match the analyzer over the whole `.rss` corpus.
#[test]
#[ignore]
fn checker_parity_corpus() {
    let root = workspace_root();
    let all_files = collect_rss_files(&root).expect("corpus discovery should succeed");
    // Slow-test gate. A handful of ~4k-line self-hosted tools (check.rss ~220KB,
    // astdump.rss ~180KB, …) dominate the wall time: the checker's per-file cost is
    // super-linear and the reg-VM is an interpreter, so those few files take minutes
    // EACH and no fan-out can split a single file. By default we skip files above a
    // byte threshold for a ~1-min iteration gate; set RSS_SELFHOST_FULL=1 for the
    // exhaustive run. The skipped files are logged (no silent truncation).
    let full = std::env::var("RSS_SELFHOST_FULL").is_ok();
    // Tightest inner loop: RSS_SELFHOST_DEV=1 runs only tests/fixtures/ (all small,
    // where nearly every oracle-positive lives) in ~10s. Use it while iterating a code;
    // fall back to the full FAST gate (615 files) before commit.
    let dev = std::env::var("RSS_SELFHOST_DEV").is_ok();
    const FAST_MAX_BYTES: u64 = 40_000;
    let (files, skipped): (Vec<_>, Vec<_>) = if full {
        (all_files, Vec::new())
    } else if dev {
        all_files
            .into_iter()
            .partition(|f| f.to_string_lossy().contains("/tests/fixtures/"))
    } else {
        all_files.into_iter().partition(|f| {
            std::fs::metadata(f)
                .map(|m| m.len() <= FAST_MAX_BYTES)
                .unwrap_or(true)
        })
    };
    if !full {
        let mode = if dev { "DEV (fixtures only)" } else { "FAST" };
        eprintln!(
            "[gate] {mode} mode ({} files; {} skipped — RSS_SELFHOST_FULL=1 for all)",
            files.len(),
            skipped.len()
        );
        if !dev {
            for f in &skipped {
                eprintln!(
                    "[gate] skipped (large): {}",
                    f.strip_prefix(&root).unwrap_or(f).display()
                );
            }
        }
    }
    let total = files.len();
    // Each file is independent, so fan the corpus out across cores. `RegVmExecutable`
    // holds an `Rc` (not `Sync`), so we can't share one exe across threads — instead
    // each worker compiles its own checker (cheap vs. hundreds of file runs) and
    // processes one chunk. Cuts the wall time from ~30 min to a few minutes.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, total.max(1));
    // Work-stealing over a shared atomic cursor rather than static chunks: a few
    // files (the ~4k-line selfhost tools) are far slower than the rest, so static
    // chunking would leave one worker straggling while the others idle. Each worker
    // owns its own exe and pulls the next file index when free.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (mut ok, mut run_failures, mut mismatches) = (0usize, Vec::new(), Vec::new());
    let partials: Vec<(usize, Vec<String>, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (root, files, next) = (&root, &files, &next);
                scope.spawn(move || {
                    let exe = compile_checker().expect("rss checker should compile");
                    let mut ok = 0usize;
                    let mut run_failures: Vec<String> = Vec::new();
                    let mut mismatches: Vec<String> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= files.len() {
                            break;
                        }
                        let file = &files[i];
                        let rel = file
                            .strip_prefix(root)
                            .unwrap_or(file)
                            .display()
                            .to_string();
                        let source = match std::fs::read_to_string(file) {
                            Ok(source) => source,
                            Err(e) => {
                                run_failures.push(format!("{rel}: unreadable: {e}"));
                                continue;
                            }
                        };
                        let oracle = checker_oracle_codes(&rel, &source);
                        match run_checker(&exe, &source) {
                            Err(e) => run_failures.push(format!("{rel}: {e}")),
                            Ok(actual) => {
                                if actual == oracle {
                                    ok += 1;
                                } else {
                                    mismatches
                                        .push(format!("{rel}: oracle={oracle:?} rss={actual:?}"));
                                }
                            }
                        }
                    }
                    (ok, run_failures, mismatches)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (o, rf, mm) in partials {
        ok += o;
        run_failures.extend(rf);
        mismatches.extend(mm);
    }
    eprintln!(
        "\n=== checker_parity_corpus (codes {SELFHOST_CHECKER_TARGET_CODES:?}) ===\n  files: {total}\n  \
         ok: {ok}\n  run-failures: {}\n  code-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(20) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(100) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "checker parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}
