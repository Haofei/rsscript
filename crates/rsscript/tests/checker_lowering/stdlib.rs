//! Spec §4/§9 — stdlib facade calls lowering to runtime hooks
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_maps_common_no_require_core_helpers() {
    let source = r#"
fn run(path: read Path) -> Result<Int, FileError> {
    let mut items = List<Int>.new()
    List.push(list: mut items, value: read 1)
    List.set(list: mut items, index: 0, value: read 2)
    let copy = List.slice(list: read items, start: 0, len: 1)
    List.append(list: mut items, values: read copy)
    mut items.append(copy)
    List.pop(list: mut items)
    let part = List.slice(list: read items, start: 0, len: 2)
    if File.exists(path: read path) {
        let text = File.read_string(path: read path)?
        File.write_string_to_path(path: read path, text: read text)?
        File.append_string(path: read path, text: read "!")?
    }
    if Path.is_absolute(path: read path) {
        List.push(list: mut items, value: read 3)
    }
    return Ok(List.len(list: read part))
}
"#;
    let rust = lower_source_to_rust("core-common.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::list_set"));
    assert!(rust.contains("rsscript_runtime::list_append"));
    assert!(rust.contains("rsscript_runtime::list_append(&mut items, &(copy));"));
    assert!(rust.contains("rsscript_runtime::list_pop"));
    assert!(rust.contains("rsscript_runtime::list_slice"));
    assert!(rust.contains("rsscript_runtime::file_exists"));
    assert!(rust.contains("rsscript_runtime::file_read_string"));
    assert!(rust.contains("rsscript_runtime::file_write_string_to_path"));
    assert!(rust.contains("rsscript_runtime::file_append_string"));
    assert!(rust.contains("rsscript_runtime::path_is_absolute"));
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
    assert!(rust.contains("rsscript_runtime::cache_insert(&mut cache, &(\"/users\".to_string()), &(\"handled /users\".to_string()));"));
    assert!(rust.contains(
        "let body = rsscript_runtime::cache_lookup(&(cache), &(\"/users\".to_string()));"
    ));
}

#[test]
fn rust_lowering_maps_list_core_calls_to_runtime_hooks() {
    let source = r#"
struct CountBox {
    value: Int
}

fn main() -> Result<Unit, JsonError> {
    let list = List<Int>.new()
    List.push(list: mut list, value: read 10)
    let count = List.len(list: read list)
    let first = List.get(list: read list, index: 0)
    let matching = List.count_where(
        list: read list,
        predicate: |item| {
            return item == 10
        },
    )
    let any_match = List.any(
        list: read list,
        predicate: |item| {
            return item == 10
        },
    )
    let all_match = List.all(
        list: read list,
        predicate: |item| {
            return item >= 10
        },
    )
    let found = List.find(
        list: read list,
        predicate: |item| {
            return item == 10
        },
    )
    let filtered = List.filter(
        list: read list,
        predicate: |item| {
            return item == 10
        },
    )
    let mapped = List.map<Int, Int>(
        list: read list,
        mapper: |item| {
            return item + 1
        },
    )
    let receiver_mapped = list.map(|item| {
        return item + 2
    })
    let order = Ord.compare(self: read 1, other: read 2)
    List.sort<Int>(list: mut list)
    List.sort_with<Int>(
        list: mut list,
        compare: |left, right| {
            return right - left
        },
    )
    let folded = List.fold<Int, CountBox>(
        list: read list,
        initial: read CountBox(value: 0),
        folder: |state, item| {
            return CountBox(value: state.value + item)
        },
    )
    let try_folded = List.try_fold<Int, CountBox, JsonError>(
        list: read list,
        initial: read CountBox(value: 0),
        folder: |state, item| {
            return Ok(CountBox(value: state.value + item))
        },
    )?
    Assert.equal_int(left: count, right: 1)
    Assert.equal_int(left: first, right: 10)
    Assert.equal_int(left: matching, right: 1)
    Assert.equal_bool(left: any_match, right: true)
    Assert.equal_bool(left: all_match, right: true)
    match found {
        Some(value) => {
            Assert.equal_int(left: value, right: 10)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    Assert.equal_int(left: List.len(list: read filtered), right: 1)
    Assert.equal_int(left: List.get(list: read mapped, index: 0), right: 11)
    Assert.equal_int(left: List.get(list: read receiver_mapped, index: 0), right: 12)
    Assert.equal_int(left: folded.value, right: 10)
    Assert.equal_int(left: try_folded.value, right: 10)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("list.rss", source).expect("source should lower");

    assert!(rust.contains("let mut list = rsscript_runtime::list_new();"));
    assert!(rust.contains("rsscript_runtime::list_push(&mut list, &(10i64));"));
    assert!(rust.contains("let count = rsscript_runtime::list_len(&(list));"));
    assert!(rust.contains("let first = rsscript_runtime::list_get(&(list), 0i64);"));
    assert!(rust.contains("let matching = rsscript_runtime::list_count_where(&(list), |item| {"));
    assert!(rust.contains("let any_match = rsscript_runtime::list_any(&(list), |item| {"));
    assert!(rust.contains("let all_match = rsscript_runtime::list_all(&(list), |item| {"));
    assert!(rust.contains("let found = rsscript_runtime::list_find(&(list), |item| {"));
    assert!(rust.contains("let filtered = rsscript_runtime::list_filter(&(list), |item| {"));
    assert!(rust.contains("let mapped = rsscript_runtime::list_map(&(list), |item| {"));
    assert!(
        rust.contains("let receiver_mapped = rsscript_runtime::list_map(&list, |item: &i64| {"),
        "{rust}"
    );
    assert!(rust.contains("let order = rsscript_runtime::ord_compare(&(1i64), &(2i64));"));
    assert!(rust.contains("rsscript_runtime::list_sort(&mut list);"));
    assert!(rust.contains("rsscript_runtime::list_sort_with(&mut list, |left, right| {"));
    assert!(rust.contains("return item.clone() == 10i64;"), "{rust}");
    assert!(rust.contains("let folded = rsscript_runtime::list_fold(&(list), &(CountBox { value: 0i64 }), |state, item| {"));
    assert!(rust.contains("let try_folded = rsscript_runtime::list_try_fold(&(list), &(CountBox { value: 0i64 }), |state, item| {"));
}

#[test]
fn rust_lowering_maps_deque_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let queue = Deque<Int>.new()
    Deque.push_back<Int>(deque: mut queue, value: read 2)
    Deque.push_front<Int>(deque: mut queue, value: read 1)
    Deque.push_back<Int>(deque: mut queue, value: read 3)
    let len = Deque.len<Int>(deque: read queue)
    let empty = Deque.is_empty<Int>(deque: read queue)
    let values = Deque.to_list<Int>(deque: read queue)
    let front = Deque.pop_front<Int>(deque: mut queue)
    let back = Deque.pop_back<Int>(deque: mut queue)
    Assert.equal_int(left: len, right: 3)
    Assert.equal_bool(left: empty, right: false)
    Assert.equal_int(left: List.get<Int>(list: read values, index: 0), right: 1)
    match front {
        Some(value) => {
            Assert.equal_int(left: value, right: 1)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    match back {
        Some(value) => {
            Assert.equal_int(left: value, right: 3)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    Deque.clear<Int>(deque: mut queue)
    Assert.equal_bool(left: Deque.is_empty<Int>(deque: read queue), right: true)
}
"#;
    let rust = lower_source_to_rust("deque.rss", source).expect("source should lower");

    assert!(rust.contains("let mut queue = rsscript_runtime::deque_new();"));
    assert!(rust.contains("rsscript_runtime::deque_push_back(&mut queue, &(2i64));"));
    assert!(rust.contains("rsscript_runtime::deque_push_front(&mut queue, &(1i64));"));
    assert!(rust.contains("let len = rsscript_runtime::deque_len(&(queue));"));
    assert!(rust.contains("let empty = rsscript_runtime::deque_is_empty(&(queue));"));
    assert!(rust.contains("let values = rsscript_runtime::deque_to_list(&(queue));"));
    assert!(rust.contains("let front = rsscript_runtime::deque_pop_front(&mut queue);"));
    assert!(rust.contains("let back = rsscript_runtime::deque_pop_back(&mut queue);"));
    assert!(rust.contains("rsscript_runtime::deque_clear(&mut queue);"));
}

#[test]
fn rust_lowering_maps_pipeline_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

struct CountBox {
    value: Int
}

fn parse_positive(value: Int) -> Result<fresh CountBox, String> {
    if value < 0 {
        return Err("negative")
    }
    return Ok(CountBox(value: value))
}

fn main() -> Result<Unit, String> {
    let xs: List<Int> = [1, 2, 3]
    let ys = xs.pipeline().filter(|item| {
        return item > 1
    }).map(|item| {
        return item + 10
    }).collect()
    let pipeline = xs.pipeline()
    let parsed: FalliblePipeline<CountBox, String> = Pipeline.try_map<Int, CountBox, String>(
        pipeline: read pipeline,
        mapper: |item| {
            return parse_positive(value: item)
        },
    )
    let zs: List<Int> = parsed.filter(|item| {
        return item.value > 1
    }).map(|item| {
        return item.value + 1
    }).collect()?

    Assert.equal_int(left: List.get(list: read ys, index: 0), right: 12)
    Assert.equal_int(left: List.get(list: read zs, index: 0), right: 3)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("pipeline.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::pipeline_from_list(&xs)"));
    assert!(rust.contains("rsscript_runtime::pipeline_filter("));
    assert!(rust.contains("rsscript_runtime::pipeline_map("));
    assert!(rust.contains("rsscript_runtime::pipeline_collect("));
    assert!(rust.contains("let pipeline = rsscript_runtime::pipeline_from_list(&xs);"));
    assert!(rust.contains("let parsed = rsscript_runtime::pipeline_try_map(&(pipeline), |item| {"));
    assert!(rust.contains("rsscript_runtime::fallible_pipeline_filter("));
    assert!(rust.contains("rsscript_runtime::fallible_pipeline_map("));
    assert!(rust.contains("rsscript_runtime::fallible_pipeline_collect("));
}

#[test]
fn rust_lowered_pipeline_package_compiles() {
    let source = r#"
features: local

fn parse_positive(value: Int) -> Result<fresh Int, String> {
    if value < 0 {
        return Err("negative")
    }
    return Ok(value)
}

type Handler = owned Fn(Int) -> Int

fn make_handler() -> Handler {
    return |value| {
        return value + 1
    }
}

struct Holder<T> {
    value: T
}

fn make_generic_handler() -> Holder<owned Fn(Int) -> Int> {
    return Holder<owned Fn(Int) -> Int>(value: |value| value + 1)
}

fn main() -> Result<Unit, String> {
    let xs: List<Int> = [1, 2, 3]
    let ys = xs.pipeline().filter(|item| {
        return item > 1
    }).map(|item| {
        return item + 10
    }).collect()
    let zs: List<Int> = Pipeline.try_map<Int, Int, String>(
        pipeline: read xs.pipeline(),
        mapper: |item| {
            return parse_positive(value: item)
        },
    ).filter(|item| {
        return item > 1
    }).collect()?
    Assert.equal_int(left: List.get(list: read ys, index: 0), right: 12)
    Assert.equal_int(left: List.get(list: read zs, index: 0), right: 2)
    return Ok(Unit)
}
"#;
    let package = lower_source_to_rust_package(
        "pipeline-main.rss",
        source,
        "Pipeline Compile",
        &common::runtime_path(),
    )
    .expect("source should lower");
    let package_dir = unique_temp_dir("rsscript-pipeline-compile");
    write_generated_rust_package(&package_dir, &package).expect("package should write");

    let output = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("cargo check should run");

    assert!(
        output.status.success(),
        "cargo check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(&package_dir).expect("temp package should be removed");
}

#[test]
fn rust_lowering_maps_map_and_set_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let map = Map<String, Int>.new()
    Map.insert(map: mut map, key: read "one", value: read 1)
    let map_len = Map.len(map: read map)
    let has_one = Map.contains_key(map: read map, key: read "one")
    let first = Map.get(map: read map, key: read "one")
    let removed = Map.remove(map: mut map, key: read "one")
    let map_empty = Map.is_empty(map: read map)
    Map.clear(map: mut map)

    let set = Set<String>.new()
    let inserted = Set.insert(set: mut set, value: read "one")
    let has_set_one = Set.contains(set: read set, value: read "one")
    let set_len = Set.len(set: read set)
    let removed_set_one = Set.remove(set: mut set, value: read "one")
    let set_empty = Set.is_empty(set: read set)
    Set.clear(set: mut set)

    Assert.equal_int(left: map_len, right: 1)
    Assert.equal_bool(left: has_one, right: true)
    match first {
        Some(value) => {
            Assert.equal_int(left: value, right: 1)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    match removed {
        Some(value) => {
            Assert.equal_int(left: value, right: 1)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    Assert.equal_bool(left: map_empty, right: true)
    Assert.equal_bool(left: inserted, right: true)
    Assert.equal_bool(left: has_set_one, right: true)
    Assert.equal_int(left: set_len, right: 1)
    Assert.equal_bool(left: removed_set_one, right: true)
    Assert.equal_bool(left: set_empty, right: true)
    return Unit
}
"#;
    let rust = lower_source_to_rust("map-set.rss", source).expect("source should lower");

    assert!(rust.contains("let mut map = rsscript_runtime::map_new();"));
    assert!(
        rust.contains("rsscript_runtime::map_insert(&mut map, &(\"one\".to_string()), &(1i64));")
    );
    assert!(rust.contains("let map_len = rsscript_runtime::map_len(&(map));"));
    assert!(rust.contains(
        "let has_one = rsscript_runtime::map_contains_key(&(map), &(\"one\".to_string()));"
    ));
    assert!(
        rust.contains("let first = rsscript_runtime::map_get(&(map), &(\"one\".to_string()));")
    );
    assert!(
        rust.contains(
            "let removed = rsscript_runtime::map_remove(&mut map, &(\"one\".to_string()));"
        )
    );
    assert!(rust.contains("let map_empty = rsscript_runtime::map_is_empty(&(map));"));
    assert!(rust.contains("rsscript_runtime::map_clear(&mut map);"));
    assert!(rust.contains("let mut set = rsscript_runtime::set_new();"));
    assert!(rust.contains(
        "let inserted = rsscript_runtime::set_insert(&mut set, &(\"one\".to_string()));"
    ));
    assert!(rust.contains(
        "let has_set_one = rsscript_runtime::set_contains(&(set), &(\"one\".to_string()));"
    ));
    assert!(rust.contains("let set_len = rsscript_runtime::set_len(&(set));"));
    assert!(rust.contains(
        "let removed_set_one = rsscript_runtime::set_remove(&mut set, &(\"one\".to_string()));"
    ));
    assert!(rust.contains("let set_empty = rsscript_runtime::set_is_empty(&(set));"));
    assert!(rust.contains("rsscript_runtime::set_clear(&mut set);"));
}

#[test]
fn rust_lowering_maps_sorted_collections_to_btree_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let map = SortedMap<Int, String>.new()
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "two")
    SortedMap.insert<Int, String>(map: mut map, key: read 1, value: read "one")
    let map_len = SortedMap.len<Int, String>(map: read map)
    let has_two = SortedMap.contains_key<Int, String>(map: read map, key: read 2)
    let first = SortedMap.get<Int, String>(map: read map, key: read 1)
    let keys = SortedMap.keys<Int, String>(map: read map)
    let values = SortedMap.values<Int, String>(map: read map)
    let removed = SortedMap.remove<Int, String>(map: mut map, key: read 2)

    let set = SortedSet<Int>.new()
    let inserted_two = SortedSet.insert<Int>(set: mut set, value: read 2)
    let inserted_one = SortedSet.insert<Int>(set: mut set, value: read 1)
    let set_len = SortedSet.len<Int>(set: read set)
    let has_one = SortedSet.contains<Int>(set: read set, value: read 1)
    let ordered = SortedSet.to_list<Int>(set: read set)
    let removed_one = SortedSet.remove<Int>(set: mut set, value: read 1)

    Assert.equal_int(left: map_len, right: 2)
    Assert.equal_bool(left: has_two, right: true)
    Assert.equal_int(left: List.get<Int>(list: read keys, index: 0), right: 1)
    Assert.equal(left: read List.get<String>(list: read values, index: 0), right: read "one")
    match first {
        Some(value) => {
            Assert.equal(left: read value, right: read "one")
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    match removed {
        Some(value) => {
            Assert.equal(left: read value, right: read "two")
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }

    Assert.equal_bool(left: inserted_two, right: true)
    Assert.equal_bool(left: inserted_one, right: true)
    Assert.equal_int(left: set_len, right: 2)
    Assert.equal_bool(left: has_one, right: true)
    Assert.equal_int(left: List.get<Int>(list: read ordered, index: 0), right: 1)
    Assert.equal_bool(left: removed_one, right: true)
    SortedMap.clear<Int, String>(map: mut map)
    SortedSet.clear<Int>(set: mut set)
    Assert.equal_bool(left: SortedMap.is_empty<Int, String>(map: read map), right: true)
    Assert.equal_bool(left: SortedSet.is_empty<Int>(set: read set), right: true)
}
"#;
    let rust = lower_source_to_rust("sorted-collections.rss", source).expect("source should lower");

    assert!(rust.contains("let mut map = rsscript_runtime::sorted_map_new();"));
    assert!(rust.contains(
        "rsscript_runtime::sorted_map_insert(&mut map, &(2i64), &(\"two\".to_string()));"
    ));
    assert!(rust.contains("let map_len = rsscript_runtime::sorted_map_len(&(map));"));
    assert!(
        rust.contains("let has_two = rsscript_runtime::sorted_map_contains_key(&(map), &(2i64));")
    );
    assert!(rust.contains("let first = rsscript_runtime::sorted_map_get(&(map), &(1i64));"));
    assert!(rust.contains("let keys = rsscript_runtime::sorted_map_keys(&(map));"));
    assert!(rust.contains("let values = rsscript_runtime::sorted_map_values(&(map));"));
    assert!(rust.contains("let removed = rsscript_runtime::sorted_map_remove(&mut map, &(2i64));"));
    assert!(rust.contains("let mut set = rsscript_runtime::sorted_set_new();"));
    assert!(
        rust.contains("let inserted_two = rsscript_runtime::sorted_set_insert(&mut set, &(2i64));")
    );
    assert!(rust.contains("let ordered = rsscript_runtime::sorted_set_to_list(&(set));"));
    assert!(
        rust.contains("let removed_one = rsscript_runtime::sorted_set_remove(&mut set, &(1i64));")
    );
    assert!(rust.contains("rsscript_runtime::sorted_map_clear(&mut map);"));
    assert!(rust.contains("rsscript_runtime::sorted_set_clear(&mut set);"));
}

#[test]
fn rust_lowering_maps_persistent_map_to_runtime_hooks() {
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

    Assert.equal_bool(left: old_missing, right: false)
    Assert.equal_bool(left: has_one, right: true)
    Assert.equal_int(left: PersistentMap.len<String, Int>(map: read two), right: 2)
    Assert.equal_bool(left: PersistentMap.is_empty<String, Int>(map: read cleared), right: true)
    match value {
        Some(item) => {
            Assert.equal_int(left: item, right: 1)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    Assert.equal_bool(left: PersistentMap.contains_key<String, Int>(map: read removed, key: read "one"), right: false)
    Assert.equal_bool(left: PersistentMap.contains_key<String, Int>(map: read two, key: read "one"), right: true)
}
"#;
    let rust = lower_source_to_rust("persistent-map.rss", source).expect("source should lower");

    assert!(rust.contains("let empty = rsscript_runtime::persistent_map_new();"));
    assert!(rust.contains("let one = rsscript_runtime::persistent_map_insert(&(empty), &(\"one\".to_string()), &(1i64));"));
    assert!(rust.contains("let two = rsscript_runtime::persistent_map_insert(&(one), &(\"two\".to_string()), &(2i64));"));
    assert!(rust.contains("let old_missing = rsscript_runtime::persistent_map_contains_key(&(empty), &(\"one\".to_string()));"));
    assert!(rust.contains(
        "let value = rsscript_runtime::persistent_map_get(&(one), &(\"one\".to_string()));"
    ));
    assert!(rust.contains(
        "let removed = rsscript_runtime::persistent_map_remove(&(two), &(\"one\".to_string()));"
    ));
    assert!(rust.contains("let cleared = rsscript_runtime::persistent_map_clear(&(two));"));
    assert!(!rust.contains("imbl::HashMap"));
}

#[test]
fn rust_lowering_maps_file_core_calls_to_runtime_hooks() {
    let source = r#"
fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    with File.open_read(path: read input)? as reader {
        with File.open_write(path: read output)? as writer {
            let bytes = File.read_all(file: mut reader)?
            let text = File.read_all_string(file: mut reader)?
            File.write(file: mut writer, data: read bytes)?
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), rsscript_runtime::FileError>"));
    assert!(rust.contains("let mut reader = rsscript_runtime::file_open_read(input)?;"));
    assert!(rust.contains("let mut writer = rsscript_runtime::file_open_write(output)?;"));
    assert!(rust.contains("let bytes = rsscript_runtime::file_read_all(&mut reader)?;"));
    assert!(rust.contains("let text = rsscript_runtime::file_read_all_string(&mut reader)?;"));
    assert!(rust.contains("rsscript_runtime::file_write(&mut writer, &(bytes))?;"));
}

#[test]
fn rust_lowering_maps_json_core_calls_to_runtime_hooks() {
    let source = r#"
struct IntBox {
    value: Int
}

fn read_name(text: read String) -> Result<String, JsonError> {
    let value = Json.parse(text: read text)?
    let path = Path.from_string(value: read "profile.json")
    let value_from_file = Json.parse_file(path: read path)?
    let literal_value = Json.value(value: read {"name": "rss", "tags": ["agent", "json"]})
    let quoted = Json.quote_string(value: read "RSScript \"review\"")
    let serialized = Json.to_string(value: read value)
    let literal_serialized = Json.to_string(value: read literal_value)
    let raw_message = Json.to_string_at_or(text: read text, path: read "profile", fallback: read "{}")
    let age_at = Json.int_at_or(text: read text, path: read "profile.age", fallback: 0)
    let active_at = Json.bool_at_or(text: read text, path: read "profile.active", fallback: false)
    let parsed_from_method = text.json_parse()?
    let raw_message_from_method = text.json_string_at_or("profile", "{}")
    let age_from_method = text.json_int_at_or("profile.age", 0)
    let active_from_method = text.json_bool_at_or("profile.active", false)
    let profile_or_root = value.at_or("profile", value)
    let count = Json.array_len(value: read value)?
    let first = value.array_get(0)?
    let strings = Json.array_strings(value: read value)?
    let ints_value = Json.parse(text: read "[1, 2]")?
    let ints = Json.array_ints(value: read ints_value)?
    let bools_value = Json.parse(text: read "[true, false]")?
    let bools = Json.array_bools(value: read bools_value)?
    let has_profile = Json.array_contains_string(value: read value, item: read "profile")?
    let has_profile_prefix = Json.array_contains_substring(value: read value, text: read "prof")?
    let has_profile_named_prefix = Json.array_contains_prefix(value: read value, prefix: read "pro")?
    let matching = Json.array_count_where(
        value: read value,
        predicate: |item| {
            let text = Json.as_string(value: read item)?
            return Ok(String.starts_with(value: read text, prefix: read "pro"))
        },
    )?
    let folded = Json.array_fold<IntBox>(
        value: read value,
        initial: read IntBox(value: 0),
        folder: |state, item| {
            let text = Json.as_string(value: read item)?
            if String.starts_with(value: read text, prefix: read "pro") {
                return Ok(IntBox(value: state.value + 1))
            }

            return Ok(IntBox(value: state.value))
        },
    )?
    let profile = value.field("profile")?
    let profile_again = Json.value_at(value: read value, path: read "profile")?
    let profile_name_value = profile.field("name")?
    let profile_name = profile_name_value.as_string()?
    let profile_name_at = Json.string_at(text: read text, path: read "profile.name")?
    let missing_name = Json.string_at_or(text: read text, path: read "profile.missing", fallback: read "unknown")
    let active = Json.field_bool(value: read profile, name: read "active")?
    let age = Json.field_int(value: read profile, name: read "age")?
    let age_value = Json.field(value: read profile, name: read "age")?
    let active_value = Json.field(value: read profile, name: read "active")?
    let age_again = Json.as_int(value: read age_value)?
    let active_again = Json.as_bool(value: read active_value)?
    let profile_is_object = Json.is_object(value: read profile)
    let profile_object_len = Json.object_len(value: read profile)?
    let profile_object_keys = Json.object_keys(value: read profile)?
    let profiles_is_array = Json.is_array(value: read value)
    let name_is_null = Json.is_null(value: read profile_name_value)
    return profile_name
}
"#;
    let rust = lower_source_to_rust("json.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::JsonError>"));
    assert!(rust.contains("let value = rsscript_runtime::json_parse(text)?;"));
    assert!(rust.contains("let value_from_file = rsscript_runtime::json_parse_file(&(path))?;"));
    assert!(rust.contains(
        "let literal_value = rsscript_runtime::json_value(&rsscript_runtime::json_object"
    ));
    assert!(rust.contains("let quoted = rsscript_runtime::json_quote_string(&(\"RSScript \\\"review\\\"\".to_string()));"));
    assert!(rust.contains("let serialized = rsscript_runtime::json_to_string(&(value));"));
    assert!(
        rust.contains(
            "let literal_serialized = rsscript_runtime::json_to_string(&(literal_value));"
        )
    );
    assert!(rust.contains("let raw_message = rsscript_runtime::json_to_string_at_or"));
    assert!(rust.contains("let age_at = rsscript_runtime::json_int_at_or"));
    assert!(rust.contains("let active_at = rsscript_runtime::json_bool_at_or"));
    assert!(
        rust.contains("let parsed_from_method = rsscript_runtime::json_parse(text)?;"),
        "{rust}"
    );
    assert!(
        rust.contains(
            "let raw_message_from_method = rsscript_runtime::json_string_at_or(text, &(\"profile\".to_string()), &(\"{}\".to_string()));"
        ),
        "{rust}"
    );
    assert!(
        rust.contains(
            "let age_from_method = rsscript_runtime::json_int_at_or(text, &(\"profile.age\".to_string()), 0i64);"
        ),
        "{rust}"
    );
    assert!(rust.contains(
        "let active_from_method = rsscript_runtime::json_bool_at_or(text, &(\"profile.active\".to_string()), false);"
    ));
    assert!(rust.contains(
        "let profile_or_root = rsscript_runtime::json_at_or(&value, &(\"profile\".to_string()), &(value));"
    ));
    assert!(rust.contains(
        "let profile = rsscript_runtime::json_field(&value, &(\"profile\".to_string()))?;"
    ));
    assert!(rust.contains("let profile_again = rsscript_runtime::json_value_at(&(value), &(\"profile\".to_string()))?;"));
    assert!(rust.contains("let count = rsscript_runtime::json_array_len(&(value))?;"));
    assert!(rust.contains("let first = rsscript_runtime::json_array_get(&value, 0i64)?;"));
    assert!(rust.contains("let strings = rsscript_runtime::json_array_strings(&(value))?;"));
    assert!(rust.contains("let ints = rsscript_runtime::json_array_ints(&(ints_value))?;"));
    assert!(rust.contains("let bools = rsscript_runtime::json_array_bools(&(bools_value))?;"));
    assert!(rust.contains("let has_profile = rsscript_runtime::json_array_contains_string(&(value), &(\"profile\".to_string()))?;"));
    assert!(rust.contains("let has_profile_prefix = rsscript_runtime::json_array_contains_substring(&(value), &(\"prof\".to_string()))?;"));
    assert!(rust.contains("let has_profile_named_prefix = rsscript_runtime::json_array_contains_prefix(&(value), &(\"pro\".to_string()))?;"));
    assert!(
        rust.contains("let matching = rsscript_runtime::json_array_count_where(&(value), |item: &rsscript_runtime::JsonValue| {")
    );
    assert!(rust.contains("let folded = rsscript_runtime::json_array_fold(&(value), &(IntBox { value: 0i64 }), |state, item: &rsscript_runtime::JsonValue| {"));
    assert!(rust.contains("let text = rsscript_runtime::json_as_string(item)?;"));
    assert!(rust.contains(
        "return Ok(rsscript_runtime::string_starts_with(text, &(\"pro\".to_string())));"
    ));
    assert!(rust.contains(
        "let active = rsscript_runtime::json_field_bool(&(profile), &(\"active\".to_string()))?;"
    ));
    assert!(
        rust.contains("let profile_name = rsscript_runtime::json_as_string(&profile_name_value)?;")
    );
    assert!(rust.contains("let profile_name_at = rsscript_runtime::json_string_at(text, &(\"profile.name\".to_string()))?;"));
    assert!(rust.contains("let missing_name = rsscript_runtime::json_string_at_or(text, &(\"profile.missing\".to_string()), &(\"unknown\".to_string()));"));
    assert!(rust.contains(
        "let age = rsscript_runtime::json_field_int(&(profile), &(\"age\".to_string()))?;"
    ));
    assert!(rust.contains("let age_again = rsscript_runtime::json_as_int(&(age_value))?;"));
    assert!(rust.contains("let active_again = rsscript_runtime::json_as_bool(&(active_value))?;"));
    assert!(rust.contains("let profile_is_object = rsscript_runtime::json_is_object(&(profile));"));
    assert!(
        rust.contains("let profile_object_len = rsscript_runtime::json_object_len(&(profile))?;")
    );
    assert!(
        rust.contains("let profile_object_keys = rsscript_runtime::json_object_keys(&(profile))?;")
    );
    assert!(rust.contains("let profiles_is_array = rsscript_runtime::json_is_array(&(value));"));
    assert!(
        rust.contains("let name_is_null = rsscript_runtime::json_is_null(&(profile_name_value));")
    );
    assert!(rust.contains("return Ok(profile_name);"));
}

#[test]
fn rust_lowering_maps_csv_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn read_name(path: read Path) -> Result<String, CsvError> {
    local buffer = RowBuffer.new(size: 4096)
    with Csv.open_read(path: read path)? as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
        let row = Csv.parse_row(buffer: read buffer)?
        return Row.field_string(row: read row, index: 0)
    }
}
"#;
    let rust = lower_source_to_rust("csv.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::CsvError>"));
    assert!(rust.contains("let mut buffer = rsscript_runtime::row_buffer_new(4096i64);"));
    assert!(rust.contains("let mut file = rsscript_runtime::csv_open_read(path)?;"));
    assert!(rust.contains("rsscript_runtime::csv_read_into(&mut file, &mut buffer)?;"));
    assert!(rust.contains("let row = rsscript_runtime::csv_parse_row(&(buffer))?;"));
    assert!(rust.contains("return rsscript_runtime::row_field_string(&(row), 0i64);"));
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
    assert!(rust.contains("let mut image = rsscript_runtime::image_load(input)?;"));
    assert!(rust.contains("rsscript_runtime::image_resize(&mut image, 320i64, 240i64);"));
    assert!(rust.contains("rsscript_runtime::image_normalize(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_sharpen(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_inspect(&(image));"));
    assert!(rust.contains("rsscript_runtime::image_save(&(image), output)?;"));
}

#[test]
fn rust_lowering_maps_p0_core_std_alignment_helpers() {
    let source = r#"
struct MyError {
    message: String
}

fn fallback(error: read MyError) -> Int {
    return 7
}

fn main() -> Result<Unit, MyError> {
    let list = List<Int>.new()
    List.push(list: mut list, value: read 1)
    List.push(list: mut list, value: read 2)
    let contains_two = List.contains_value(list: read list, value: read 2)
    let removed_item = List.remove_at(list: mut list, index: 0)

    let map = Map<String, Int>.new()
    let old_missing = Map.insert_old(map: mut map, key: read "one", value: read 1)
    let old_present = Map.insert_old(map: mut map, key: read "one", value: read 2)
    let present = Map.get_or_default(map: read map, key: read "one", default: read 0)
    let missing = Map.get_or_default(map: read map, key: read "missing", default: read 3)

    let left = Set<String>.new()
    let right = Set<String>.new()
    Set.insert(set: mut left, value: read "a")
    Set.insert(set: mut right, value: read "a")
    Set.insert(set: mut right, value: read "b")
    let unioned = Set.union(left: read left, right: read right)
    let common = Set.intersection(left: read left, right: read right)
    let only_right = Set.difference(left: read right, right: read left)
    let subset = Set.is_subset(left: read left, right: read right)

    let sliced = String.slice(value: read "abcdef", start: 1, len: 3)
    let index = String.index_of(value: read "abcdef", needle: read "cd")
    let repeated = String.repeat(value: read "ab", count: 2)
    let parsed = String.parse_int(value: read "42")

    let bytes = Bytes.from_string(value: read "ab")
    let more = Bytes.from_string(value: read "cd")
    let joined = Bytes.concat(left: read bytes, right: read more)
    let bytes_len = Bytes.len(value: read joined)
    let bytes_empty = Bytes.is_empty(value: read joined)
    let bytes_slice = Bytes.slice(value: read joined, start: 1, len: 2)
    let bytes_text = Bytes.to_string(value: read bytes_slice)

    let buffer = Buffer.new(size: 16)
    let buffer_len = Buffer.len(buffer: read buffer)
    let buffer_empty = Buffer.is_empty(buffer: read buffer)

    let maybe = Option.or<Int>(value: read None, fallback: read Some(5))
    let filtered = Option.filter<Int>(
        value: read maybe,
        predicate: |value| {
            return value > 3
        },
    )
    let err_value = Result.err<Int, MyError>(value: read Err(MyError(message: "bad")))
    let recovered = Result.unwrap_or_else<Int, MyError>(
        result: read Err(MyError(message: "bad")),
        fallback: |error| {
            return fallback(error: read error)
        },
    )

    Assert.equal_bool(left: contains_two, right: true)
    Assert.equal_int(left: present, right: 2)
    Assert.equal_int(left: missing, right: 3)
    Assert.equal_bool(left: subset, right: true)
    Assert.equal_int(left: bytes_len, right: 4)
    Assert.equal_bool(left: bytes_empty, right: false)
    Assert.equal_int(left: buffer_len, right: 0)
    Assert.equal_bool(left: buffer_empty, right: true)
    Assert.equal_int(left: recovered, right: 7)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("p0-core-helpers.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "let contains_two = rsscript_runtime::list_contains_value(&(list), &(2i64));"
        )
    );
    assert!(rust.contains("rsscript_runtime::list_remove_at(&mut list, 0i64);"));
    assert!(
        rust.contains("let old_missing = rsscript_runtime::map_insert_old(&mut map, &(\"one\".to_string()), &(1i64));")
    );
    assert!(
        rust.contains("let present = rsscript_runtime::map_get_or_default(&(map), &(\"one\".to_string()), &(0i64));")
    );
    assert!(rust.contains("let unioned = rsscript_runtime::set_union(&(left), &(right));"));
    assert!(rust.contains("let common = rsscript_runtime::set_intersection(&(left), &(right));"));
    assert!(rust.contains("let only_right = rsscript_runtime::set_difference(&(right), &(left));"));
    assert!(rust.contains("let subset = rsscript_runtime::set_is_subset(&(left), &(right));"));
    assert!(rust.contains(
        "let sliced = rsscript_runtime::string_slice(&(\"abcdef\".to_string()), 1i64, 3i64);"
    ));
    assert!(rust.contains("let index = rsscript_runtime::string_index_of(&(\"abcdef\".to_string()), &(\"cd\".to_string()));"));
    assert!(
        rust.contains(
            "let repeated = rsscript_runtime::string_repeat(&(\"ab\".to_string()), 2i64);"
        )
    );
    assert!(
        rust.contains("let parsed = rsscript_runtime::string_parse_int(&(\"42\".to_string()));")
    );
    assert!(rust.contains("let joined = rsscript_runtime::bytes_concat(&(bytes), &(more));"));
    assert!(rust.contains("let bytes_len = rsscript_runtime::bytes_len(&(joined));"));
    assert!(rust.contains("let bytes_empty = rsscript_runtime::bytes_is_empty(&(joined));"));
    assert!(
        rust.contains("let bytes_slice = rsscript_runtime::bytes_slice(&(joined), 1i64, 2i64);")
    );
    assert!(rust.contains("let buffer_len = rsscript_runtime::buffer_len(&(buffer));"));
    assert!(rust.contains("let buffer_empty = rsscript_runtime::buffer_is_empty(&(buffer));"));
    assert!(rust.contains("let maybe = rsscript_runtime::option_or(&(None), &(Some(5i64)));"));
    assert!(rust.contains("let filtered = rsscript_runtime::option_filter(&(maybe), |value| {"));
    assert!(rust.contains("let err_value = rsscript_runtime::result_err(&(Err(MyError { message: \"bad\".to_string() })));"));
    assert!(rust.contains("let recovered = rsscript_runtime::result_unwrap_or_else(&(Err(MyError { message: \"bad\".to_string() })), |error| {"));
}

#[test]
fn rust_lowering_maps_p1_application_core_helpers() {
    let source = r#"
fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read "/tmp/rsscript-p1")
    let nested = Path.join(base: read root, child: read "child")
    let normalized = Path.normalize(path: read Path.join(base: read nested, child: read ".."))
    let text_path = Path.with_extension(path: read Path.join(base: read root, child: read "out"), extension: read "txt")
    let starts = Path.starts_with(path: read text_path, base: read root)

    Directory.create_all(path: read root)?
    if !Directory.exists(path: read nested) {
        Directory.create(path: read nested)?
    }
    let listed = Directory.list_paths(path: read root)?

    let data = Bytes.from_string(value: read "hello")
    File.write_bytes(path: read text_path, data: read data)?
    File.append_bytes(path: read text_path, data: read Bytes.from_string(value: read "!"))?
    File.append_string(path: read text_path, text: read "\n")?
    File.remove(path: read text_path)?

    let args = Args.all()
    let arg_count = Args.count()
    let first_arg = Args.get(index: 0)
    let fallback = Args.get_or_default(index: 1000, default: read "missing")

    Log.error(message: read "p1")

    Assert.equal_bool(left: starts, right: true)
    return Ok(Unit)
}

fn process_main() -> Result<Unit, String> {
    let proc = Process.run(command: read "printf", args: read List<String>.new())?
    let proc_timeout = Process.run_timeout(command: read "printf", args: read List<String>.new(), timeout_ms: 1000)?
    Assert.equal_int(left: proc.status, right: 0)
    Assert.equal_int(left: proc_timeout.status, right: 0)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("p1-core-helpers.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::path_normalize("));
    assert!(rust.contains("rsscript_runtime::path_with_extension("));
    assert!(rust.contains("rsscript_runtime::path_starts_with("));
    assert!(rust.contains("rsscript_runtime::directory_create(&(nested))?;"));
    assert!(rust.contains("let listed = rsscript_runtime::directory_list_paths(&(root))?;"));
    assert!(rust.contains("rsscript_runtime::file_append_bytes("));
    assert!(rust.contains("rsscript_runtime::file_remove(&(text_path))?;"));
    assert!(rust.contains("rsscript_runtime::args_all();"));
    assert!(rust.contains("rsscript_runtime::args_get(0i64);"));
    assert!(rust.contains("let proc = rsscript_runtime::process_run(&(\"printf\".to_string()), &(rsscript_runtime::list_new()))?;"));
    assert!(rust.contains("let proc_timeout = rsscript_runtime::process_run_timeout(&(\"printf\".to_string()), &(rsscript_runtime::list_new()), 1000i64)?;"));
    assert!(rust.contains("rsscript_runtime::log_error(&(\"p1\".to_string()));"));
}

#[test]
fn rust_lowering_maps_config_reload_to_runtime_hooks() {
    let source = r#"
features: local

fn load_config(path: read Path) -> Result<fresh ConfigValue, ConfigError> {
    return Config.load(path: read path)
}

fn reload_config(path: read Path, store: mut ConfigStore) -> Result<Unit, ConfigError> {
    let next = load_config(path: read path)?
    ConfigStore.replace(store: mut store, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("config.rss", source).expect("source should lower");

    assert!(
        rust.contains("-> Result<rsscript_runtime::ConfigValue, rsscript_runtime::ConfigError>")
    );
    assert!(rust.contains("return rsscript_runtime::config_load(path);"));
    assert!(rust.contains("store: &mut rsscript_runtime::ConfigStore"));
    assert!(rust.contains("rsscript_runtime::config_store_replace(store, &(next));"));
}

#[test]
fn rust_lowering_maps_rules_config_reload_to_runtime_hooks() {
    let source = r#"
fn load_rules_config(path: read Path) -> Result<fresh Config, ConfigError> {
    let rules = RuleLoader.load_rules(path: read path)?
    return Ok(Config.new(name: read "rules", rules: read rules))
}

fn reload_rules_config(path: read Path, global: mut GlobalConfig) -> Result<Unit, ConfigError> {
    let next = load_rules_config(path: read path)?
    GlobalConfig.replace(global: mut global, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("rules-config.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<rsscript_runtime::Config, rsscript_runtime::ConfigError>"));
    assert!(rust.contains("let rules = rsscript_runtime::rule_loader_load_rules(path)?;"));
    assert!(
        rust.contains(
            "return Ok(rsscript_runtime::config_new(&(\"rules\".to_string()), &(rules)));"
        )
    );
    assert!(rust.contains("global: &mut rsscript_runtime::GlobalConfig"));
    assert!(rust.contains("rsscript_runtime::global_config_replace(global, &(next));"));
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

    assert!(rust.contains("let mut counter = rsscript_runtime::counter_new(1i64);"));
    assert!(rust.contains("rsscript_runtime::counter_add(&mut counter, 2i64);"));
    assert!(rust.contains("let value = rsscript_runtime::counter_value(&(counter));"));
    assert!(rust.contains("if value == 3i64 {"));
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
fn rust_lowering_does_not_decode_multiline_string_backslashes() {
    let source = "fn prompt() -> String {\n    return \"\"\"literal \\n text\"\"\"\n}\n";
    let rust =
        lower_source_to_rust("multiline-string-raw.rss", source).expect("source should lower");

    assert!(rust.contains("return \"literal \\\\n text\".to_string();"));
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

    assert!(rust.contains("rsscript_runtime::log_write(&(\"hello RSScript\".to_string()));"));
}

#[test]
fn rust_lowering_maps_assert_equal_to_runtime_hook() {
    let source = r#"
fn main() -> Unit {
    Assert.equal(left: read "rss", right: read "rss")
    Assert.equal_int(left: 42, right: 42)
    Assert.equal_bool(left: true, right: true)
    return Unit
}
"#;
    let rust = lower_source_to_rust("assert.rss", source).expect("source should lower");

    assert!(rust.contains(
        "rsscript_runtime::assert_equal(&(\"rss\".to_string()), &(\"rss\".to_string()));"
    ));
    assert!(rust.contains("rsscript_runtime::assert_equal_int(42i64, 42i64);"));
    assert!(rust.contains("rsscript_runtime::assert_equal_bool(true, true);"));
}

#[test]
fn rust_lowering_maps_string_concat_to_rust_std_expression() {
    let source = r#"
features: local

fn main() -> Unit {
    let message = String.concat(left: read "hello ", right: read "world")
    let suffixed = message.concat("!")
    let literal_joined = "agent HTTP status ".concat(String.from_int(value: 500))
    let literal_joined_len = "prefix".concat("suffix").len()
    let copied = String.copy(value: read message)
    let count = String.from_int(value: 42)
    let count_method = count.len().to_string()
    let ok = String.from_bool(value: true)
    let length = String.len(value: read message)
    let starts = String.starts_with(value: read message, prefix: read "hello")
    let ends = String.ends_with(value: read message, suffix: read "world")
    let contains = String.contains(value: read message, needle: read "lo wo")
    let lines = String.lines(value: read "one\ntwo")
    let chars = String.chars(value: read "a1 \n")
    let first_char = List.get(list: read chars, index: 0)
    let second_char = List.get(list: read chars, index: 1)
    let third_char = List.get(list: read chars, index: 2)
    let fourth_char = List.get(list: read chars, index: 3)
    let char_alpha = Char.is_alpha(value: read first_char)
    let char_digit = Char.is_digit(value: read second_char)
    let char_space = Char.is_whitespace(value: read third_char)
    let char_alnum = Char.is_alphanumeric(value: read second_char)
    let char_code = Char.to_code(value: read first_char)
    let char_from_code = Char.from_code(value: 97)
    let char_compare = Char.compare(left: read first_char, right: read second_char)
    let char_text = Char.to_string(value: read fourth_char)
    Assert.equal_bool(left: char_alpha, right: true)
    Assert.equal_bool(left: char_digit, right: true)
    Assert.equal_bool(left: char_space, right: true)
    Assert.equal_bool(left: char_alnum, right: true)
    Assert.equal_int(left: char_code, right: 97)
    let compare_less = char_compare < 0
    match char_from_code {
        Some(value) => {
            let same_char = Char.compare(left: read value, right: read first_char)
            Assert.equal_int(left: same_char, right: 0)
        }
        None => {
            Assert.equal_bool(left: false, right: true)
        }
    }
    Assert.equal_bool(left: compare_less, right: true)
    Assert.equal(left: read char_text, right: read "\n")
    let joined = String.join(parts: read lines, separator: read ",")
    let root = "target/"
    let path_message = String.join(parts: read ["root=", root, " ok"], separator: read "")
    let receiver_joined = ["root=", root, " ok"].join("")
    let stripped = String.strip_prefix(value: read "pub fn Api.run()", prefix: read "pub fn ")
    let before = String.before(value: read "Api.run() -> Unit", delimiter: read "(")
    let after = String.after(value: read "Api.run() -> Unit", delimiter: read "-> ")
    let empty = String.is_empty(value: read "")
    let trimmed = String.trim(value: read "  review  ")
    let lower = String.to_lowercase(value: read "Review")
    let upper = String.to_uppercase(value: read "review")
    let replaced = String.replace(value: read "review map", from: read "map", to: read "plan")
    let split = String.split(value: read "review,map", delimiter: read ",")
    local builder = StringBuilder.new()
    StringBuilder.push(builder: mut builder, value: read "hello")
    StringBuilder.push(builder: mut builder, value: read " builder")
    let built = StringBuilder.finish(builder: take builder)
    Log.write(message: read message)
    Log.write(message: read suffixed)
    Log.write(message: read literal_joined)
    Log.write(message: read copied)
    Log.write(message: read count)
    Log.write(message: read count_method)
    Log.write(message: read ok)
    Log.write(message: read path_message)
    return Unit
}
"#;
    let rust = lower_source_to_rust("string.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let message = format!(\"{}{}\", &(\"hello \".to_string()), &(\"world\".to_string()));"
    ));
    assert!(rust.contains(
        "let suffixed = rsscript_runtime::string_concat(&message, &(\"!\".to_string()));"
    ));
    assert!(
        rust.contains(
            "let literal_joined = rsscript_runtime::string_concat(&\"agent HTTP status \".to_string(), &(rsscript_runtime::string_from_int(500i64)));"
        ),
        "{rust}"
    );
    assert!(rust.contains(
        "let literal_joined_len = rsscript_runtime::string_len(&rsscript_runtime::string_concat(&\"prefix\".to_string(), &(\"suffix\".to_string())));"
    ));
    assert!(rust.contains("let copied = rsscript_runtime::string_copy(&(message));"));
    assert!(rust.contains("let count = rsscript_runtime::string_from_int(42i64);"));
    assert!(rust.contains("let count_method = rsscript_runtime::string_from_int("));
    assert!(rust.contains("rsscript_runtime::string_len(&count)"));
    assert!(
        rust.contains("let path_message = rsscript_runtime::string_join(&vec![\"root=\".to_string(), root.clone(), \" ok\".to_string()], &(\"\".to_string()));"),
        "String.join should materialize read String bindings inside List<String> literals, got:\n{rust}"
    );
    assert!(
        rust.contains(
            "let receiver_joined = rsscript_runtime::list_join(&vec![\"root=\".to_string(), root.clone(), \" ok\".to_string()], &(\"\".to_string()));"
        ),
        "List.join receiver calls should borrow the separator, got:\n{rust}"
    );
    assert!(rust.contains("let ok = rsscript_runtime::string_from_bool(true);"));
    assert!(rust.contains("let length = rsscript_runtime::string_len(&(message));"));
    assert!(rust.contains(
        "let starts = rsscript_runtime::string_starts_with(&(message), &(\"hello\".to_string()));"
    ));
    assert!(rust.contains(
        "let ends = rsscript_runtime::string_ends_with(&(message), &(\"world\".to_string()));"
    ));
    assert!(rust.contains(
        "let contains = rsscript_runtime::string_contains(&(message), &(\"lo wo\".to_string()));"
    ));
    assert!(
        rust.contains("let lines = rsscript_runtime::string_lines(&(\"one\\ntwo\".to_string()));")
    );
    assert!(
        rust.contains("let chars = rsscript_runtime::string_chars(&(\"a1 \\n\".to_string()));")
    );
    assert!(rust.contains("let char_alpha = rsscript_runtime::char_is_alpha(&(first_char));"));
    assert!(rust.contains("let char_digit = rsscript_runtime::char_is_digit(&(second_char));"));
    assert!(rust.contains("let char_space = rsscript_runtime::char_is_whitespace(&(third_char));"));
    assert!(
        rust.contains("let char_alnum = rsscript_runtime::char_is_alphanumeric(&(second_char));")
    );
    assert!(rust.contains("let char_code = rsscript_runtime::char_to_code(&(first_char));"));
    assert!(rust.contains("let char_from_code = rsscript_runtime::char_from_code(97i64);"));
    assert!(rust.contains(
        "let char_compare = rsscript_runtime::char_compare(&(first_char), &(second_char));"
    ));
    assert!(rust.contains("let char_text = rsscript_runtime::char_to_string(&(fourth_char));"));
    assert!(
        rust.contains(
            "let joined = rsscript_runtime::string_join(&(lines), &(\",\".to_string()));"
        )
    );
    assert!(rust.contains("let stripped = rsscript_runtime::string_strip_prefix(&(\"pub fn Api.run()\".to_string()), &(\"pub fn \".to_string()));"));
    assert!(rust.contains("let before = rsscript_runtime::string_before(&(\"Api.run() -> Unit\".to_string()), &(\"(\".to_string()));"));
    assert!(rust.contains("let after = rsscript_runtime::string_after(&(\"Api.run() -> Unit\".to_string()), &(\"-> \".to_string()));"));
    assert!(rust.contains("let empty = rsscript_runtime::string_is_empty(&(\"\".to_string()));"));
    assert!(
        rust.contains(
            "let trimmed = rsscript_runtime::string_trim(&(\"  review  \".to_string()));"
        )
    );
    assert!(
        rust.contains(
            "let lower = rsscript_runtime::string_to_lowercase(&(\"Review\".to_string()));"
        )
    );
    assert!(
        rust.contains(
            "let upper = rsscript_runtime::string_to_uppercase(&(\"review\".to_string()));"
        )
    );
    assert!(rust.contains("let replaced = rsscript_runtime::string_replace(&(\"review map\".to_string()), &(\"map\".to_string()), &(\"plan\".to_string()));"));
    assert!(rust.contains("let split = rsscript_runtime::string_split(&(\"review,map\".to_string()), &(\",\".to_string()));"));
    assert!(rust.contains("let mut builder = rsscript_runtime::string_builder_new();"));
    assert!(rust.contains(
        "rsscript_runtime::string_builder_push(&mut builder, &(\"hello\".to_string()));"
    ));
    assert!(
        rust.contains("let built = rsscript_runtime::string_builder_finish(builder);"),
        "StringBuilder.finish should take builder by value, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_process_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Result<Unit, String> {
    let args = List<String>.new()
    List.push(list: mut args, value: read "hello")
    let stdout = Process.run_stdout(command: read "printf", args: read args)?
    Log.write(message: read stdout)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("process.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::process_run_stdout"));
}

#[test]
fn rust_lowering_maps_process_request_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Result<Unit, String> {
    let env: List<ProcessEnv> = [
        ProcessEnv(name: "RSS_TEST", value: "1"),
    ]
    let request = ProcessRequest(
        command: "printf",
        args: ["hello"],
        cwd: Some(Path.from_string(value: read "/tmp")),
        stdin: None,
        env: env,
        timeout_ms: 1000,
        merge_stderr: true,
        output_cap_bytes: 4096,
    )
    let output = Process.run_request(request: read request)?
    Assert.equal_int(left: output.status, right: 0)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("process-request.rss", source).expect("source should lower");

    assert!(
        rust.contains("rsscript_runtime::ProcessRequest"),
        "process request should lower to runtime request type, got:\n{rust}"
    );
    assert!(rust.contains("rsscript_runtime::ProcessEnv"));
    assert!(rust.contains("let output = rsscript_runtime::process_run_request(&(request))?;"));
}

#[test]
fn rust_lowering_maps_stdlib_facade_calls_to_runtime_hooks() {
    let source = r#"
features: native, local

fn inspect_file(path: read Path, bytes: read Bytes) -> Result<String, FileError> {
    let exists = Directory.exists(path: read path)
    let metadata = Directory.metadata(path: read path)?
    let length = metadata_len(metadata: read metadata)
    let byte_text = Bytes.to_string(value: read bytes)
    let text = Directory.read_string(path: read path)?
    Directory.write_string(path: read path, content: read text)?
    let file_hash = Hash.sha256_file(path: read path)?
    let byte_hash = Hash.sha256_bytes(value: read bytes)
    let current = Env.current_dir()?
    let run_root = Env.run_workspace_root()
    Env.set_current_dir(path: read current)?
    let configured = Env.get_or_default(name: read "RSS_MODE", default: read "dev")
    let configured_method = "RSS_MODE".env_or("dev")
    let now = Clock.now()
    let elapsed = Instant.elapsed(start: read now)
    let millis = Duration.as_ms(value: read elapsed)
    return Ok(file_hash)
}

fn metadata_len(metadata: read FileMetadata) -> Int {
    return metadata.len
}

fn compile_match(pattern: read String, value: read String) -> Result<Bool, RegexError> {
    let regex = Regex.compile(pattern: read pattern)?
    let found = Regex.find(regex: read regex, value: read value)
    let captures = Regex.captures(regex: read regex, value: read value)
    let replaced = Regex.replace_all(regex: read regex, value: read value, replacement: read "x")
    let parts = Regex.split(regex: read regex, value: read value)
    return Ok(Regex.is_match(regex: read regex, value: read value))
}

fn temp_path(parent: read Path) -> Result<fresh Path, FileError> {
    with TempDir.new_in(parent: read parent)? as dir {
        return Ok(TempDir.path(dir: read dir))
    }
}

fn fetch_status(url: read Url) -> Result<Int, HttpError> {
    let response = Http.get(url: read url)?
    let ok = HttpResponse.is_success(response: read response)
    let body = HttpResponse.text(response: read response)
    return Ok(HttpResponse.status(response: read response))
}
"#;
    let rust = lower_source_to_rust("stdlib-facades.rss", source).expect("source should lower");

    assert!(rust.contains("metadata: &rsscript_runtime::FileMetadata"));
    assert!(rust.contains("let exists = rsscript_runtime::directory_exists(path);"));
    assert!(rust.contains("let metadata = rsscript_runtime::directory_metadata(path)?;"));
    assert!(rust.contains("rsscript_runtime::directory_write_string(path, &(text))?;"));
    assert!(rust.contains("let file_hash = rsscript_runtime::hash_sha256_file(path)?;"));
    assert!(rust.contains("let byte_hash = rsscript_runtime::hash_sha256_bytes(bytes);"));
    assert!(rust.contains("let byte_text = rsscript_runtime::bytes_to_string(bytes);"));
    assert!(rust.contains("let current = rsscript_runtime::env_current_dir()?;"));
    assert!(rust.contains("let run_root = rsscript_runtime::env_run_workspace_root();"));
    assert!(rust.contains("rsscript_runtime::env_set_current_dir(&(current))?;"));
    assert!(rust.contains("let configured = rsscript_runtime::env_get_or_default"));
    assert!(rust.contains(
        "let configured_method = rsscript_runtime::env_get_or_default(&\"RSS_MODE\".to_string(), &(\"dev\".to_string()));"
    ));
    assert!(rust.contains("let now = rsscript_runtime::clock_now();"));
    assert!(rust.contains("let elapsed = rsscript_runtime::instant_elapsed(&(now));"));
    assert!(rust.contains("let millis = rsscript_runtime::duration_as_ms(&(elapsed));"));
    assert!(rust.contains("-> Result<bool, rsscript_runtime::RegexError>"));
    assert!(rust.contains("let regex = rsscript_runtime::regex_compile(pattern)?;"));
    assert!(rust.contains("let found = rsscript_runtime::regex_find(&(regex), value);"));
    assert!(rust.contains("let captures = rsscript_runtime::regex_captures(&(regex), value);"));
    assert!(rust.contains("let replaced = rsscript_runtime::regex_replace_all"));
    assert!(rust.contains("let parts = rsscript_runtime::regex_split(&(regex), value);"));
    assert!(rust.contains("return Ok(rsscript_runtime::regex_is_match(&(regex), value));"));
    assert!(rust.contains("let mut dir = rsscript_runtime::tempdir_new_in(parent)?;"));
    assert!(rust.contains("return Ok(rsscript_runtime::tempdir_path(&(dir)));"));
    assert!(rust.contains("-> Result<i64, rsscript_runtime::HttpError>"));
    assert!(rust.contains("let response = rsscript_runtime::http_get(url)?;"));
    assert!(rust.contains("let ok = rsscript_runtime::http_response_is_success(&(response));"));
    assert!(rust.contains("let body = rsscript_runtime::http_response_text(&(response));"));
    assert!(rust.contains("return Ok(rsscript_runtime::http_response_status(&(response)));"));
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

    assert!(rust.contains("let value = 20i64 + 22i64;"));
}

#[test]
fn rust_lowering_maps_builtin_operators_to_rust_expressions() {
    let source = r#"
fn main() -> Unit {
    let negative = -1
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

    assert!(rust.contains("let difference = 44i64 - 2i64;"));
    assert!(rust.contains("let negative = -1i64;"));
    assert!(rust.contains("let product = 6i64 * 7i64;"));
    assert!(rust.contains("let quotient = product / 2i64;"));
    assert!(rust.contains("let equal = product == 42i64;"));
    assert!(rust.contains("let different = quotient != 0i64;"));
    assert!(rust.contains("let less = quotient < product;"));
    assert!(rust.contains("let less_equal = quotient <= product;"));
    assert!(rust.contains("let greater = product > quotient;"));
    assert!(rust.contains("let greater_equal = product >= quotient;"));
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
    // Provenance: the remapped error names the enclosing RSScript source symbol
    // and its lowered Rust symbol.
    assert!(
        remapped
            .diagnostic
            .causes
            .iter()
            .any(|cause| cause.contains("RSScript symbol: make_session")
                && cause.contains("lowered: make_session")),
        "expected source/lowered symbol provenance: {:?}",
        remapped.diagnostic.causes
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
fn rustc_diagnostics_do_not_map_to_nearest_previous_source_marker() {
    let source_map = vec![rsscript::RustSourceMapEntry {
        kind: "function".to_string(),
        source: rsscript::Span {
            file: "bad.rss".to_string(),
            line: 1,
            column: 1,
            length: 10,
        },
        generated: rsscript::Span {
            file: "src/lib.rs".to_string(),
            line: 10,
            column: 1,
            length: 2,
        },
        ..Default::default()
    }];
    let rustc_json = r#"{"message":"cannot find value","code":{"code":"E0425","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":200,"line_end":200,"column_start":1,"column_end":2,"is_primary":true}]}"#;

    let remapped = remap_rustc_diagnostic_json(&source_map, rustc_json)
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
fn assertion_runtime_diagnostics_parse_to_rsscript_diagnostics() {
    let stderr = r#"thread 'main' panicked at runtime/src/lib.rs:1:1:
RSSCRIPT_RUNTIME_DIAGNOSTIC:{"code":"RS1201","severity":"error","summary":"RSScript runtime error: assertion failed: left `left` did not equal right `right`","file":"<runtime>","line":1,"column":1,"length":1,"label":"assertion failed: left `left` did not equal right `right`","kind":"assertion_failed"}
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"#;

    let diagnostics = parse_runtime_diagnostics(stderr);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "RS1201");
    assert_eq!(diagnostics[0].span.file, "<runtime>");
    assert!(diagnostics[0].summary.contains("assertion failed"));
    assert!(
        diagnostics[0]
            .causes
            .iter()
            .any(|cause| cause == "runtime error kind: assertion_failed")
    );
}

#[test]
fn rust_lowering_maps_protocol_static_dispatch_to_rust_traits() {
    let source = r#"
features: local

protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(
    self: mut BufferWriter,
    message: read String,
) -> Unit
    effects(retains(message))
{
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_line<W: Writer>(
    writer: mut W,
    message: read String,
) -> Unit {
    Writer.write(self: mut writer, message: read message)
}
"#;
    let rust = lower_source_to_rust("protocol-lower.rss", source).expect("source should lower");

    assert!(rust.contains("pub trait Writer"));
    assert!(rust.contains("impl Writer for BufferWriter"));
    assert!(rust.contains("fn write_line<W: Writer>(writer: &mut W, message: &String)"));
    assert!(rust.contains("Writer::write(writer, message);"));
    assert!(rust.contains("BufferWriter_write(self, message);"));
    assert!(!rust.contains("fn Writer_write"));
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
fn review_reports_protocol_impl_mapping_changes() {
    let old_source = r#"
protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

fn BufferWriter.audit_write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#;
    let new_source = r#"
protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

fn BufferWriter.audit_write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.audit_write
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let finding = findings
        .iter()
        .find(|finding| finding.code == "RSR016")
        .expect("protocol impl mapping change should be reported");

    assert_eq!(finding.risk, ReviewRisk::Api);
    assert_eq!(
        finding.before.as_deref(),
        Some("impl Writer for BufferWriter { write = BufferWriter.write }")
    );
    assert_eq!(
        finding.after.as_deref(),
        Some("impl Writer for BufferWriter { write = BufferWriter.audit_write }")
    );
    assert!(format_review_human(&findings).contains("RSR016[api]:"));
}

#[test]
fn review_map_marks_noescape_callback_calls_review_required_not_unknown() {
    let source = r#"
features: local

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}
"#;
    let map = review_map_sources(vec![("callback.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "apply")
        .expect("expected apply region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "noescape callback call `callback`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_local_closure_direct_calls_review_required_not_unknown() {
    let source = r#"
features: local

fn run() -> Unit {
    local callback = || {
        return Unit
    }

    callback()
    return Unit
}
"#;
    let map = review_map_sources(vec![("local-callback.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "run")
        .expect("expected run region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        !region
            .reasons
            .iter()
            .any(|reason| reason.contains("unresolved call")),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_pass_fixture_unknown_rate_stays_low() {
    let owned_sources = common::fixture_paths("tests/fixtures/pass")
        .into_iter()
        .map(|path| {
            let file = path.display().to_string();
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            (file, source)
        })
        .collect::<Vec<_>>();
    let sources = owned_sources
        .iter()
        .map(|(file, source)| (file.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let map = review_map_sources(sources);

    assert!(map.summary.total_functions >= 60);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn review_map_complex_supported_script_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/complex-supported-review-map.rss");
    let source = common::read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 13);
    assert!(map.summary.total_lines >= 70);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert_eq!(map.summary.review_required.functions, 10);
    assert_eq!(map.summary.foldable.functions, 3);
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn review_map_realistic_supported_corpus_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/realistic-supported-review-corpus.rss");
    let source = common::read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 20);
    assert!(map.summary.total_lines >= 120);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert!(map.summary.review_required.functions >= 10, "{map:?}");
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn review_map_app_benchmark_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/app-review-benchmark.rss");
    let source = common::read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 41);
    assert!(map.summary.total_lines >= 300, "{map:?}");
    assert_eq!(map.files[0].risk, ReviewMapFileRisk::High);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert!(map.summary.review_required.functions >= 31, "{map:?}");
    assert!(map.summary.foldable.functions <= 10, "{map:?}");

    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
    assert_eq!(json["summary"]["must_review"]["functions"], 31);
    assert_eq!(json["summary"]["low_semantic_risk"]["functions"], 10);
}

#[test]
fn checker_validates_protocol_impl_mappings_against_contracts() {
    let source = r#"
protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(
    self: mut BufferWriter,
    message: read String,
) -> Unit
    effects(retains(message))
{
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#;
    let diagnostics = analyze_source_with_core("protocol-impl.rss", source);
    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn rust_lowering_builtin_value_clone_uses_rust_clone() {
    // Regression: `.clone()` on a builtin value type whose clone the checker
    // resolves (List/Bytes/Buffer) typechecks via the `Clone` protocol but used to
    // lower to a dangling `List_clone`-style call (rustc E0425). It must lower to
    // Rust's `.clone()`, which these types support.
    let source = r#"
features: local

fn dup_list(xs: read List<Int>) -> fresh List<Int> {
    return xs.clone()
}

fn dup_bytes(b: read Bytes) -> fresh Bytes {
    return b.clone()
}
"#;
    let rust = lower_source_to_rust("builtin-clone.rss", source).expect("source should lower");
    assert!(
        rust.contains(".clone()"),
        "builtin `.clone()` should lower to Rust's clone, got:\n{rust}"
    );
    assert!(
        !rust.contains("List_clone") && !rust.contains("Bytes_clone"),
        "builtin `.clone()` must not emit a dangling `Type_clone` call, got:\n{rust}"
    );
}
