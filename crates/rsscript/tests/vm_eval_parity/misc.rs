//! eval≡lowered parity: uncategorized
#![allow(unused_imports, dead_code)]
use super::*;

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
    common::assert_vm_eval_matches_backend(
        "parity-arithmetic.rss",
        "rsscript_parity_arithmetic",
        source,
    );
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
    common::assert_vm_eval_matches_backend("parity-struct.rss", "rsscript_parity_struct", source);
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
    common::assert_vm_eval_matches_backend(
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
    Log.write(message: read String.from_int(value: Option.unwrap_or_else<Int>(value: read maybe(found: true), default: || {
        return 14
    })))
    Log.write(message: read String.from_int(value: Option.unwrap_or_else<Int>(value: read maybe(found: false), default: || {
        return 15
    })))
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
    Log.write(message: read String.from_int(value: Result.unwrap_or_else<Int, String>(result: read checked(ok: true), fallback: |error| {
        return String.len(value: read error)
    })))
    Log.write(message: read String.from_int(value: Result.unwrap_or_else<Int, String>(result: read checked(ok: false), fallback: |error| {
        return String.len(value: read error)
    })))
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
    common::assert_vm_eval_matches_backend(
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
    common::assert_vm_eval_matches_backend("parity-while.rss", "rsscript_parity_while", source);
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
    common::assert_vm_eval_matches_backend("parity-sum.rss", "rsscript_parity_sum", source);
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
    common::assert_vm_eval_matches_backend("parity-logging.rss", "rsscript_parity_logging", source);
}

#[test]
fn parity_resource_drop_cleanup_intrinsics() {
    let source = r#"
features: local

resource FileHandle {
    fd: Int

    drop {
        Log.write(message: read "file-drop")
        OS.close(fd: fd)
    }
}

resource DbHandle {
    fd: Int

    drop {
        Log.write(message: read "db-drop")
        Db.close(fd: fd)
    }
}

fn main() -> Unit {
    with FileHandle(fd: 0) as file {
        Log.write(message: read "file-body")
        Log.write(message: read String.from_int(value: file.fd))
    }
    with DbHandle(fd: 0) as db {
        Log.write(message: read "db-body")
        Log.write(message: read String.from_int(value: db.fd))
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-resource-drop-cleanup.rss",
        "rsscript_parity_resource_drop_cleanup",
        source,
        &[],
        &[],
    );
}

#[test]
fn parity_resource_pool_stats_intrinsics() {
    let source = r#"
features: local

resource Session {
    value: Int
}

fn borrow_empty() -> Result<Unit, PoolError> {
    local empty = ResourcePool<Session>.lazy(
        create: || {
            return Session(value: 0)
        },
        max_size: 0,
    )
    with ResourcePool.try_borrow(pool: mut empty)? as session {
        Log.write(message: read String.from_int(value: session.value))
    }
    return Ok(Unit)
}

fn main() -> Unit {
    local pool = ResourcePool<Session>.new(
        create: || {
            return Session(value: 7)
        },
        max_size: 2,
    )
    let snapshot = ResourcePool.stats(pool: mut pool)
    Log.write(message: read String.from_int(value: PoolStats.capacity(stats: read snapshot)))
    Log.write(message: read String.from_int(value: PoolStats.created(stats: read snapshot)))
    Log.write(message: read String.from_int(value: PoolStats.available(stats: read snapshot)))
    Log.write(message: read String.from_int(value: PoolStats.in_use(stats: read snapshot)))

    with ResourcePool.borrow(pool: mut pool) as session {
        Log.write(message: read String.from_int(value: session.value))
        ResourcePool.discard(lease: mut session)
    }
    let discarded = ResourcePool.stats(pool: mut pool)
    Log.write(message: read String.from_int(value: PoolStats.created(stats: read discarded)))
    Log.write(message: read String.from_int(value: PoolStats.available(stats: read discarded)))

    match borrow_empty() {
        Ok(_) => {
            Log.write(message: read "unexpected-borrow")
        }
        Err(error) => {
            let message = PoolError.message(error: read error)
            if String.contains(value: read message, needle: read "") {
                Log.write(message: read "pool-error")
            }
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-resource-pool-stats.rss",
        "rsscript_parity_resource_pool_stats",
        source,
        &[],
        &[],
    );
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
    common::assert_vm_eval_matches_backend("parity-url.rss", "rsscript_parity_url", source);
}

#[test]
fn parity_encoding_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let encoded = Base64.encode(value: read "rsscript")
    Log.write(message: read encoded)

    match Base64.decode_string(text: read encoded) {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let bytes = String.to_bytes(value: read "hex")
    Log.write(message: read Base64.encode_bytes(value: read bytes))

    match Base64.decode(text: read "%%%") {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let hexed = Hex.encode_string(value: read "Az")
    Log.write(message: read hexed)
    Log.write(message: read Hex.encode(value: read bytes))

    match Hex.decode(text: read hexed) {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    match Hex.decode(text: read "not-hex") {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    match Hex.decode(text: read "1f8b08000000000002ff4b4c4a0600c241243503000000") {
        Ok(gzipped) => {
            match Gzip.decompress_bytes(value: read gzipped) {
                Ok(value) => {
                    Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
                    Log.write(message: read Hex.encode(value: read value))
                }
                Err(error) => Log.write(message: read DecodeError.message(error: read error))
            }
        }
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let bad_gzip = String.to_bytes(value: read "not gzip")
    match Gzip.decompress_bytes(value: read bad_gzip) {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let component = Url.encode_component(value: read "a b/é?x=1")
    Log.write(message: read component)

    match Url.decode_component(value: read component) {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    match Url.decode_component(value: read "%FF") {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-encoding.rss",
        "rsscript_parity_encoding",
        source,
    );
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

    common::assert_vm_eval_matches_backend_with_distinct_args(
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
    common::assert_vm_eval_matches_backend(
        "parity-request-response.rss",
        "rsscript_parity_request_response",
        source,
    );
}

#[test]
fn parity_borrowed_match_payload_used_by_value() {
    // Regression: matching a borrowed `read Option<T>` / `Result<T, E>` and using
    // the payload by value used to lower to `&T` and fail rustc E0308. The lowerer
    // now rebinds the payload to an owned value (`*x` for Copy, `x.clone()`
    // otherwise), so the AOT backend compiles and matches the interpreter.
    let source = r#"
features: local

fn int_or(o: read Option<Int>, fallback: Int) -> Int {
    match o {
        Some(s) => {
            return s
        }
        None => {
            return fallback
        }
    }
}

fn string_or(o: read Option<String>, fallback: read String) -> fresh String {
    match o {
        Some(s) => {
            return s
        }
        None => {
            return String.concat(left: read fallback, right: read "")
        }
    }
}

fn ok_or(r: read Result<Int, String>) -> fresh String {
    match r {
        Ok(value) => {
            return Int.to_string(value: read value)
        }
        Err(message) => {
            return message
        }
    }
}

fn main() -> Unit {
    Log.write(message: read Int.to_string(value: read int_or(o: read Some(7), fallback: 0)))
    Log.write(message: read Int.to_string(value: read int_or(o: read None, fallback: 9)))
    Log.write(message: read string_or(o: read Some("hi"), fallback: read "x"))
    Log.write(message: read string_or(o: read None, fallback: read "fallback"))
    Log.write(message: read ok_or(r: read Ok(3)))
    Log.write(message: read ok_or(r: read Err("boom")))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-borrowed-match-payload.rss",
        "rsscript_parity_borrowed_match_payload",
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
    common::assert_vm_eval_matches_backend(
        "parity-diff-patch.rss",
        "rsscript_parity_diff_patch",
        source,
    );
}

#[test]
fn parity_pipeline_intrinsics() {
    common::run_with_large_stack(|| {
        let source = r#"
features: local

fn main() -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4]
    let shifted = Pipeline.map<Int, Int>(
        pipeline: read Pipeline.filter<Int>(
            pipeline: read List.pipeline<Int>(list: read numbers),
            predicate: |item| {
                let half = item / 2
                return half * 2 == item
            },
        ),
        mapper: |item| {
            return item + 10
        },
    )
    let echoed = Pipeline.each<Int>(pipeline: read shifted, action: |item| {
        Log.write(message: read String.from_int(value: item))
        return Unit
    })
    let collected = Pipeline.collect<Int>(pipeline: read echoed)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read collected)))
    Log.write(message: read String.from_int(value: collected[0]))
    Log.write(message: read String.from_int(value: collected[1]))

    let ok_pipeline = Pipeline.try_map<Int, Int, String>(pipeline: read shifted, mapper: |item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(item + 1)
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read ok_pipeline) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: items[0]))
            Log.write(message: read String.from_int(value: items[1]))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let mapped = FalliblePipeline.map<Int, Int, String>(pipeline: read ok_pipeline, mapper: |item| {
        return item + 100
    })
    let filtered = FalliblePipeline.filter<Int, String>(pipeline: read mapped, predicate: |item| {
        return item > 113
    })
    let touched = FalliblePipeline.each<Int, String>(pipeline: read filtered, action: |item| {
        Log.write(message: read String.from_int(value: item))
        return Unit
    })
    let final_pipeline = FalliblePipeline.try_map<Int, Int, String>(pipeline: read touched, mapper: |item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(item + 1)
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read final_pipeline) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
            Log.write(message: read String.from_int(value: items[0]))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let failed = Pipeline.try_map<Int, Int, String>(pipeline: read List.pipeline<Int>(list: read numbers), mapper: |item| {
        if item == 3 {
            return Err(String.copy(value: read "stop"))
        }
        return Ok(item + 0)
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read failed) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    let still_failed = FalliblePipeline.map<Int, Int, String>(pipeline: read failed, mapper: |item| {
        Log.write(message: read "should-not-run")
        return item + 1
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read still_failed) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Unit
}
"#;
        common::assert_vm_eval_matches_backend(
            "parity-pipeline.rss",
            "rsscript_parity_pipeline",
            source,
        );
    });
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
    common::assert_vm_eval_matches_backend("parity-deque.rss", "rsscript_parity_deque", source);
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
    common::assert_vm_eval_matches_backend(
        "parity-duration.rss",
        "rsscript_parity_duration",
        source,
    );
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
    common::assert_vm_eval_matches_backend("parity-counter.rss", "rsscript_parity_counter", source);
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
    common::assert_vm_eval_matches_backend("parity-ord.rss", "rsscript_parity_ord", source);
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
    common::assert_vm_eval_matches_backend(
        "parity-with-try.rss",
        "rsscript_parity_with_try",
        source,
    );
}

#[test]
fn parity_try_short_circuits_inside_expressions() {
    let source = r#"
fn fail() -> Result<Int, String> {
    return Err("bad")
}

fn side_effect() -> Int {
    Log.write(message: read "should-not-run")
    return 1
}

fn combine(a: Int, b: Int) -> Int {
    return a + b
}

fn binary_case() -> Result<Int, String> {
    return Ok(fail()? + side_effect())
}

fn call_case() -> Result<Int, String> {
    return Ok(combine(a: fail()?, b: side_effect()))
}

fn list_case() -> Result<Unit, String> {
    let _ = [fail()?, side_effect()]
    return Ok(Unit)
}

fn print_result(label: read String, value: read Result<Unit, String>) -> Unit {
    match value {
        Ok(_) => {
            Log.write(message: read "unexpected-ok")
        }
        Err(message) => {
            Log.write(message: read label)
            Log.write(message: read message)
        }
    }
    return Unit
}

fn main() -> Unit {
    match binary_case() {
        Ok(_) => {
            Log.write(message: read "unexpected-binary")
        }
        Err(message) => {
            Log.write(message: read "binary")
            Log.write(message: read message)
        }
    }
    match call_case() {
        Ok(_) => {
            Log.write(message: read "unexpected-call")
        }
        Err(message) => {
            Log.write(message: read "call")
            Log.write(message: read message)
        }
    }
    print_result(label: read "list", value: read list_case())
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-try-short-circuit.rss",
        "rsscript_parity_try_short_circuit",
        source,
    );
}

#[test]
fn parity_try_on_option_short_circuits_on_none() {
    // `?` on an `Option` keeps `Some(x)` and early-returns `None`, matching `?`
    // on `Result`. Must behave identically on the VM and the compiled backend.
    let source = r#"
fn make(n: Int) -> Option<Int> {
    if n > 0 {
        return Some(n)
    }
    return None
}

fn doubled(n: Int) -> Option<Int> {
    let v = make(n: n)?
    return Some(v + v)
}

fn show(label: read String, value: read Option<Int>) -> Unit {
    match value {
        Some(x) => {
            Log.write(message: read String.concat(left: read label, right: read String.from_int(value: read x)))
        }
        None => {
            Log.write(message: read String.concat(left: read label, right: read "none"))
        }
    }
    return Unit
}

fn main() -> Unit {
    show(label: read "pos: ", value: read doubled(n: 21))
    show(label: read "zero: ", value: read doubled(n: 0))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-try-option.rss",
        "rsscript_parity_try_option",
        source,
    );
}

#[test]
fn parity_boolean_operators_short_circuit() {
    let source = r#"
fn side_effect() -> Bool {
    Log.write(message: read "should-not-run")
    return true
}

fn main() -> Unit {
    if false && side_effect() {
        Log.write(message: read "unexpected-and")
    }
    if true || side_effect() {
        Log.write(message: read "or-ok")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-bool-short-circuit.rss",
        "rsscript_parity_bool_short_circuit",
        source,
    );
}
