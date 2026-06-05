use rsscript::{eval_source_main_with_args, vm_eval_source_main_with_args};

#[test]
fn vm_runs_pure_loop_sum_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + index
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-loop.rss", source, []);
}

#[test]
fn vm_runs_user_function_hot_loop_like_interpreter() {
    let source = r#"
fn mix(value: Int, salt: Int) -> Int {
    let doubled = value * 2
    return doubled + salt
}

fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + mix(value: index, salt: 1)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-function-loop.rss", source, []);
}

#[test]
fn vm_runs_args_parse_match_like_interpreter() {
    let source = r#"
fn bench_size(default: Int) -> Int {
    let raw = Args.get_or_default(index: 0, default: read String.from_int(value: default))
    match String.parse_int(value: read raw) {
        Some(value) => {
            return value
        }
        None => {
            return default
        }
    }
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: bench_size(default: 7)))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-args-match.rss", source, ["11"]);
}

#[test]
fn vm_runs_list_index_scan_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < List.len<Int>(list: read values) {
        total = total + List.get<Int>(list: read values, index: index)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-list-index.rss", source, []);
}

#[test]
fn vm_runs_map_insert_lookup_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local table = Map<Int, Int>.new()
    while index < 10 {
        let value = index * 3
        Map.insert<Int, Int>(map: mut table, key: read index, value: read value)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        match Map.get<Int, Int>(map: read table, key: read index) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-map-lookup.rss", source, []);
}

#[test]
fn vm_runs_string_key_map_like_interpreter() {
    let source = r#"
features: local

fn key_for(value: Int) -> String {
    return String.concat(left: read "key-", right: read String.from_int(value: value))
}

fn main() -> Unit {
    let mut index = 0
    local table = Map<String, Int>.new()
    while index < 10 {
        let key = key_for(value: index)
        Map.insert<String, Int>(map: mut table, key: read key, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        let key = key_for(value: index)
        match Map.get<String, Int>(map: read table, key: read key) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-map-string-key.rss", source, []);
}

#[test]
fn vm_runs_list_closure_pipeline_like_interpreter() {
    let source = r#"
features: local

struct Acc {
    total: Int
}

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    let mapped = List.map<Int, Int>(
        list: read values,
        mapper: |value| {
            return value * 2 + 1
        },
    )
    let filtered = List.filter<Int>(
        list: read mapped,
        predicate: |value| {
            let half = value / 2
            return half * 2 != value
        },
    )
    let acc = List.fold<Int, Acc>(
        list: read filtered,
        initial: read Acc(total: 0),
        folder: |state, value| {
            return Acc(total: state.total + value)
        },
    )

    Log.write(message: read String.from_int(value: acc.total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-list-closure-pipeline.rss", source, []);
}

#[test]
fn vm_runs_pipeline_chain_like_interpreter() {
    let source = r#"
features: local

struct Acc {
    total: Int
}

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    let pipeline = Pipeline.map<Int, Int>(
        pipeline: read Pipeline.filter<Int>(
            pipeline: read List.pipeline<Int>(list: read values),
            predicate: |value| {
                let half = value / 2
                return half * 2 == value
            },
        ),
        mapper: |value| {
            return value * 3 + 1
        },
    )
    let collected = Pipeline.collect<Int>(pipeline: read pipeline)
    let acc = List.fold<Int, Acc>(
        list: read collected,
        initial: read Acc(total: 0),
        folder: |state, value| {
            return Acc(total: state.total + value)
        },
    )

    Log.write(message: read String.from_int(value: acc.total))
    return Unit
}
"#;

    assert_vm_matches_interpreter("vm-pipeline-chain.rss", source, []);
}

fn assert_vm_matches_interpreter<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let interpreter =
        eval_source_main_with_args(file, source, args.iter().copied()).expect("eval should run");
    let vm =
        vm_eval_source_main_with_args(file, source, args.iter().copied()).expect("vm should run");

    assert_eq!(vm.value, interpreter.value);
    assert_eq!(vm.display_value, interpreter.display_value);
    assert_eq!(vm.native_value, interpreter.native_value);
    assert_eq!(vm.stdout, interpreter.stdout);
    assert_eq!(vm.stderr, interpreter.stderr);
}
