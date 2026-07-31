//! Spec §2.5 — review map: boundary/effect/unknown classification
#![allow(unused_imports, dead_code)]
use super::*;

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
fn review_map_json_carries_module_and_use_declarations() {
    let source = r#"
module rss.package.review

use rss.package.contract.PackageContract
use rss.review.ReviewMap

pub fn PackageReview.ready() -> Unit {
    return ()
}
"#;
    let map = review_map_sources(vec![("package-review.rss", source)]);
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");

    assert_eq!(json["modules"][0]["file"], "package-review.rss");
    assert_eq!(json["modules"][0]["module_path"], "rss.package.review");
    assert_eq!(
        json["modules"][0]["uses"][0]["path"],
        "rss.package.contract.PackageContract"
    );
    assert_eq!(
        json["modules"][0]["uses"][1]["path"],
        "rss.review.ReviewMap"
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
fn review_map_resolves_calls_across_source_sets() {
    let helper = r#"
fn helper() -> Unit {
    return Unit
}
"#;
    let entry = r#"
fn entry() -> Unit {
    helper()
    return Unit
}
"#;
    let map = review_map_sources(vec![("helper.rss", helper), ("entry.rss", entry)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.unknown.functions, 0);
    assert!(map.files.iter().any(|file| {
        file.file == "entry.rss"
            && file.regions.iter().any(|region| {
                region.function == "entry"
                    && region.classification == ReviewMapClassification::Foldable
            })
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
fn review_map_marks_callers_of_qualified_review_required_functions() {
    let source = r#"
struct Payload
struct Cache

fn Cache.remember(cache: read Cache, value: read Payload) -> Unit
    effects(retains(value))

fn wrapper(cache: read Cache, value: read Payload) -> Unit {
    Cache.remember(cache: read cache, value: read value)
}
"#;
    let map = review_map_sources(vec![("qualified-calls.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 0);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls must-review `Cache.remember`")
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
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 0);
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
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `pure`")
    }));
}

#[test]
fn review_map_marks_native_calls_as_review_required_boundaries() {
    let source = r#"
features: native

native fn Native.echo(message: read String) -> String
    effects(native)

fn caller(message: read String) -> String {
    return Native.echo(message: read message)
}
"#;
    let map = review_map_sources(vec![("native-call-map.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "caller"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "native call `Native.echo`")
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
fn review_map_records_explicit_fn_capture_contracts() {
    let source = r#"
features: local

fn run() -> Int {
    let offset = 2
    local add = fn(value) captures(read offset) effects(pure) {
        return value + offset
    }
    return add(40)
}
"#;
    let map = review_map_sources(vec![("explicit-fn-review.rss", source)]);
    let region = map
        .files
        .iter()
        .flat_map(|file| file.regions.iter())
        .find(|region| region.function == "run")
        .expect("run region should exist");

    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "explicit closure captures read `offset`"),
        "{region:?}"
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "explicit closure effects(pure)"),
        "{region:?}"
    );
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
fn review_map_json_records_receiver_call_canonical_expansion() {
    let source = r#"
features: local

struct Cache

fn Cache.put(self: mut Cache, key: read String) -> Unit {
    return Unit
}

fn update(cache: mut Cache) -> Unit {
    mut cache.put(key: read "k")
    return Unit
}
"#;
    let map = review_map_sources(vec![("receiver-map.rss", source)]);
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    let update = json["files"][0]["regions"]
        .as_array()
        .and_then(|regions| regions.iter().find(|region| region["function"] == "update"))
        .expect("update region should exist");
    let receiver_call = &update["receiver_calls"][0];
    assert_eq!(receiver_call["source"], "mut cache.put");
    assert_eq!(receiver_call["canonical_callee"], "Cache.put");
    assert_eq!(receiver_call["self_effect"], "mut");
    assert_eq!(receiver_call["resolution"], "user_function");
}
