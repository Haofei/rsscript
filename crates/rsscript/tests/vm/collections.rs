//! Spec §6 — register-VM execution: collections (List/Map/Set/Deque/Buffer)
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_task_group_async_let_like_backend() {
    // `async let` spawns concurrent tasks; `await` joins them. The scheduler must
    // run the spawned tasks and hand their results back to the joining task.
    let source = r#"

async fn fetch_user() -> Result<String, String> {
    return Ok("user")
}

async fn fetch_profile() -> Result<String, String> {
    return Ok("profile")
}

fn main() -> Result<Unit, String> {
    task_group {
        async let user = fetch_user()
        async let profile = fetch_profile()

        let u = await user?
        let p = await profile?
        Output.write(message: read u)
        Output.write(message: read p)
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-task-group.rss", source, []);
}

#[test]
fn reg_vm_propagates_all_list_mutators_through_mut_param_like_backend() {
    // A `mut List` parameter aliases the caller's list (lowered to `&mut` by the
    // backend), so every mutator — not just `push` — must propagate. This caught
    // the divergence where `set`/`append`/`sort` cloned-and-wrote-back and so
    // silently dropped mutations made through a `mut` param.
    let source = r#"
fn mutate(xs: mut List<Int>) -> Unit {
    List.push<Int>(list: mut xs, value: read 4)
    List.set<Int>(list: mut xs, index: 0, value: read 10)
    List.append<Int>(list: mut xs, values: read List<Int>.new())
    List.sort<Int>(list: mut xs)
    return Unit
}

fn main() -> Unit {
    let mut a = List<Int>.new()
    List.push<Int>(list: mut a, value: read 3)
    List.push<Int>(list: mut a, value: read 1)
    mutate(xs: mut a)
    Output.write(message: read String.from_int(value: List.get<Int>(list: read a, index: 0)))
    Output.write(message: read String.from_int(value: List.get<Int>(list: read a, index: 2)))
    Output.write(message: read String.from_int(value: List.len<Int>(list: read a)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-mut-list-param.rss", source, []);
}

#[test]
fn reg_vm_isolates_by_value_list_params_like_backend() {
    // A non-`mut` parameter is by value/`&` in the backend (with an inserted
    // `.clone()`), so the callee owns an isolated copy. Stashing it in a struct,
    // handing it back, and mutating it must not write into the caller's original
    // list — which only holds once non-`mut` params are deep-copied on entry.
    let source = r#"
struct Box {
    items: List<Int>
}

fn keep(xs: read List<Int>) -> Box {
    return Box(items: read xs)
}

fn main() -> Unit {
    let mut a = List<Int>.new()
    List.push<Int>(list: mut a, value: read 1)
    let boxed = keep(xs: read a)
    let mut b = boxed.items
    List.push<Int>(list: mut b, value: read 9)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read a)))
    Output.write(message: read String.from_int(value: List.len<Int>(list: read b)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-by-value-list-param.rss", source, []);
}

#[test]
fn reg_vm_runs_common_math_string_char_and_list_helpers_like_backend() {
    let source = r#"

fn main() -> Unit {
    Output.write(message: read String.from_int(value: Math.pow(base: 3, exponent: 4)))

    Output.write(message: read String.trim_start(value: read "  left"))
    Output.write(message: read String.trim_end(value: read "right  "))
    Output.write(message: read String.pad_left(value: read "7", width: 3, fill: read "0"))
    Output.write(message: read String.pad_right(value: read "x", width: 3, fill: read "."))
    Output.write(message: read String.pad_left(value: read "x", width: 2, fill: read "é"))
    Output.write(message: read String.pad_right(value: read "x", width: 2, fill: read "é"))
    Output.write(message: read String.reverse(value: read "abc"))
    Output.write(message: read String.replace_first(value: read "one one", from: read "one", to: read "two"))
    Output.write(message: read String.from_int(value: String.count(value: read "banana", needle: read "an")))
    match String.char_at(value: read "abc", index: 1) {
        Some(value) => Output.write(message: read Char.to_string(value: read value))
        None => Output.write(message: read "missing-char")
    }
    match String.char_at(value: read "abc", index: 9) {
        Some(value) => Output.write(message: read Char.to_string(value: read value))
        None => Output.write(message: read "missing-char")
    }

    match String.char_at(value: read "a", index: 0) {
        Some(value) => {
            if Char.is_lower(value: read value) {
                Output.write(message: read "lower")
            }
        }
        None => Output.write(message: read "missing-lower")
    }
    match String.char_at(value: read "Z", index: 0) {
        Some(value) => {
            if Char.is_upper(value: read value) {
                Output.write(message: read "upper")
            }
        }
        None => Output.write(message: read "missing-upper")
    }
    match String.char_at(value: read "Q", index: 0) {
        Some(value) => Output.write(message: read Char.to_string(value: read Char.to_lower(value: read value)))
        None => Output.write(message: read "missing-to-lower")
    }
    match String.char_at(value: read "q", index: 0) {
        Some(value) => Output.write(message: read Char.to_string(value: read Char.to_upper(value: read value)))
        None => Output.write(message: read "missing-to-upper")
    }

    let values = [3, 1, 3, 2]
    Output.write(message: read String.from_int(value: List.sum(list: read values)))
    match List.min(list: read values) {
        Some(value) => Output.write(message: read String.from_int(value: value))
        None => Output.write(message: read "min-none")
    }
    match List.max(list: read values) {
        Some(value) => Output.write(message: read String.from_int(value: value))
        None => Output.write(message: read "max-none")
    }
    let deduped = List.dedup<Int>(list: read values)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read deduped)))
    Output.write(message: read String.from_int(value: deduped[0]))
    Output.write(message: read String.from_int(value: deduped[1]))
    Output.write(message: read String.from_int(value: deduped[2]))

    let nested = [[1, 2], [3], List<Int>.new()]
    let flat = List.flatten<Int>(list: read nested)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read flat)))
    Output.write(message: read String.from_int(value: flat[0]))
    Output.write(message: read String.from_int(value: flat[2]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-common-helper-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_persistent_map_intrinsics_like_interpreter() {
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

    if old_missing {
        Output.write(message: read "bad-empty")
    }
    if has_one {
        Output.write(message: read "has-one")
    }
    Output.write(message: read String.from_int(value: PersistentMap.len<String, Int>(map: read two)))
    if PersistentMap.is_empty<String, Int>(map: read cleared) {
        Output.write(message: read "cleared")
    }
    match value {
        Some(item) => {
            Output.write(message: read String.from_int(value: item))
        }
        None => {
            Output.write(message: read "missing")
        }
    }
    if PersistentMap.contains_key<String, Int>(map: read removed, key: read "one") {
        Output.write(message: read "bad-removed")
    }
    if PersistentMap.contains_key<String, Int>(map: read two, key: read "one") {
        Output.write(message: read "original-kept")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-persistent-map-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_deque_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local deque = Deque<Int>.new()
    if Deque.is_empty<Int>(deque: read deque) {
        Output.write(message: read "empty")
    }
    Deque.push_back<Int>(deque: mut deque, value: read 2)
    Deque.push_front<Int>(deque: mut deque, value: read 1)
    Deque.push_back<Int>(deque: mut deque, value: read 3)
    Output.write(message: read String.from_int(value: Deque.len<Int>(deque: read deque)))
    let values = Deque.to_list<Int>(deque: read deque)
    Output.write(message: read String.from_int(value: values[0]))
    Output.write(message: read String.from_int(value: values[1]))
    Output.write(message: read String.from_int(value: values[2]))
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "front-none")
        }
    }
    match Deque.pop_back<Int>(deque: mut deque) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "back-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "front-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "front-none")
        }
    }
    Deque.push_back<Int>(deque: mut deque, value: read 4)
    Deque.clear<Int>(deque: mut deque)
    if Deque.is_empty<Int>(deque: read deque) {
        Output.write(message: read "cleared")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-deque-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_set_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local set = Set<String>.new()
    if Set.is_empty<String>(set: read set) {
        Output.write(message: read "empty")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Output.write(message: read "insert-a")
    }
    if Set.insert<String>(set: mut set, value: read "b") {
        Output.write(message: read "insert-b")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Output.write(message: read "duplicate")
    } else {
        Output.write(message: read "duplicate-no")
    }
    if Set.contains<String>(set: read set, value: read "b") {
        Output.write(message: read "has-b")
    }
    Output.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    if Set.remove<String>(set: mut set, value: read "a") {
        Output.write(message: read "removed-a")
    }
    if Set.remove<String>(set: mut set, value: read "z") {
        Output.write(message: read "removed-z")
    } else {
        Output.write(message: read "removed-z-no")
    }
    if Set.contains<String>(set: read set, value: read "b") {
        Output.write(message: read "for-each-b")
    }

    local right = Set<String>.new()
    Set.insert<String>(set: mut right, value: read "b")
    Set.insert<String>(set: mut right, value: read "c")
    let union = Set.union<String>(left: read set, right: read right)
    let intersection = Set.intersection<String>(left: read set, right: read right)
    let difference = Set.difference<String>(left: read right, right: read set)
    if Set.contains<String>(set: read union, value: read "c") {
        Output.write(message: read "union-c")
    }
    Output.write(message: read String.from_int(value: Set.len<String>(set: read intersection)))
    if Set.contains<String>(set: read difference, value: read "c") {
        Output.write(message: read "diff-c")
    }
    if Set.is_subset<String>(left: read intersection, right: read union) {
        Output.write(message: read "subset")
    }
    let list = Set.to_list<String>(set: read union)
    Output.write(message: read String.from_int(value: List.len<String>(list: read list)))
    if List.contains_value<String>(list: read list, value: read "b") {
        Output.write(message: read "list-b")
    }
    if List.contains_value<String>(list: read list, value: read "c") {
        Output.write(message: read "list-c")
    }
    Set.clear<String>(set: mut set)
    Output.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-set-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_set_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local set = SortedSet<Int>.new()
    if SortedSet.is_empty<Int>(set: read set) {
        Output.write(message: read "empty")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 3) {
        Output.write(message: read "insert-3")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 1) {
        Output.write(message: read "insert-1")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Output.write(message: read "insert-2")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Output.write(message: read "duplicate")
    } else {
        Output.write(message: read "duplicate-no")
    }
    if SortedSet.contains<Int>(set: read set, value: read 1) {
        Output.write(message: read "has-1")
    }
    Output.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    let values = SortedSet.to_list<Int>(set: read set)
    Output.write(message: read String.from_int(value: values[0]))
    Output.write(message: read String.from_int(value: values[1]))
    Output.write(message: read String.from_int(value: values[2]))
    if SortedSet.remove<Int>(set: mut set, value: read 2) {
        Output.write(message: read "removed-2")
    }
    if SortedSet.remove<Int>(set: mut set, value: read 9) {
        Output.write(message: read "removed-9")
    } else {
        Output.write(message: read "removed-9-no")
    }
    SortedSet.clear<Int>(set: mut set)
    Output.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-set-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_map_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local map = SortedMap<Int, String>.new()
    if SortedMap.is_empty<Int, String>(map: read map) {
        Output.write(message: read "empty")
    }
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "two")
    SortedMap.insert<Int, String>(map: mut map, key: read 1, value: read "one")
    SortedMap.insert<Int, String>(map: mut map, key: read 3, value: read "three")
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "TWO")
    Output.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    if SortedMap.contains_key<Int, String>(map: read map, key: read 2) {
        Output.write(message: read "has-2")
    }
    match SortedMap.get<Int, String>(map: read map, key: read 2) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing")
        }
    }
    let keys = SortedMap.keys<Int, String>(map: read map)
    Output.write(message: read String.from_int(value: keys[0]))
    Output.write(message: read String.from_int(value: keys[1]))
    Output.write(message: read String.from_int(value: keys[2]))
    let values = SortedMap.values<Int, String>(map: read map)
    Output.write(message: read values[0])
    Output.write(message: read values[1])
    Output.write(message: read values[2])
    match SortedMap.remove<Int, String>(map: mut map, key: read 2) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "remove-none")
        }
    }
    match SortedMap.remove<Int, String>(map: mut map, key: read 9) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "remove-none")
        }
    }
    SortedMap.clear<Int, String>(map: mut map)
    Output.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-map-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_map_order_and_fresh_lists_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local strings = SortedMap<String, Int>.new()
    SortedMap.insert<String, Int>(map: mut strings, key: read "b", value: read 2)
    let first_keys = SortedMap.keys<String, Int>(map: read strings)
    let first_values = SortedMap.values<String, Int>(map: read strings)
    SortedMap.insert<String, Int>(map: mut strings, key: read "a", value: read 1)
    let sorted_keys = SortedMap.keys<String, Int>(map: read strings)
    let sorted_values = SortedMap.values<String, Int>(map: read strings)
    Output.write(message: read first_keys[0])
    Output.write(message: read String.from_int(value: first_values[0]))
    Output.write(message: read String.from_int(value: List.len<String>(list: read first_keys)))
    Output.write(message: read sorted_keys[0])
    Output.write(message: read sorted_keys[1])
    Output.write(message: read String.from_int(value: sorted_values[0]))
    Output.write(message: read String.from_int(value: sorted_values[1]))

    local bools = SortedMap<Bool, String>.new()
    SortedMap.insert<Bool, String>(map: mut bools, key: read true, value: read "true")
    SortedMap.insert<Bool, String>(map: mut bools, key: read false, value: read "false")
    let bool_values = SortedMap.values<Bool, String>(map: read bools)
    Output.write(message: read bool_values[0])
    Output.write(message: read bool_values[1])
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-map-order-fresh.rss", source, []);
}

#[test]
fn reg_vm_runs_buffer_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    local buffer = Buffer.new(size: 16)
    if Buffer.is_empty(buffer: read buffer) {
        Output.write(message: read "buffer-empty")
    }
    Output.write(message: read String.from_int(value: Buffer.len(buffer: read buffer)))
    let view = Buffer.view(buffer: read buffer, start: 0, len: 10)
    if BufferView.is_empty(value: read view) {
        Output.write(message: read "view-empty")
    }
    Output.write(message: read String.from_int(value: BufferView.len(value: read view)))
    let slice = BufferView.slice(value: read view, start: 1, len: 2)
    Output.write(message: read String.from_int(value: Bytes.len(value: read BufferView.to_bytes(value: read slice))))
    Output.write(message: read String.from_int(value: Bytes.len(value: read Bytes.from_buffer(buffer: read buffer))))
    Buffer.clear(buffer: mut buffer)
    Buffer.consume(buffer: take buffer)
    Output.write(message: read "consumed")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-buffer-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_map_read_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let empty: Map<String, Int> = Map.new<String, Int>()
    if Map.is_empty<String, Int>(map: read empty) {
        Output.write(message: read "empty")
    }

    let table: Map<String, Int> = {"a" => 1, "b" => 2}
    let keys = Map.keys<String, Int>(map: read table)
    Assert.equal_int(left: List.len<String>(list: read keys), right: 2)
    Assert.equal_bool(left: List.contains_value<String>(list: read keys, value: read "a"), right: true)
    Assert.equal_bool(left: List.contains_value<String>(list: read keys, value: read "b"), right: true)

    let values = Map.values<String, Int>(map: read table)
    Assert.equal_int(left: List.len<Int>(list: read values), right: 2)
    Assert.equal_bool(left: List.contains_value<Int>(list: read values, value: read 1), right: true)
    Assert.equal_bool(left: List.contains_value<Int>(list: read values, value: read 2), right: true)
    Output.write(message: read "ok")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-read-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_list_non_closure_read_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let numbers: List<Int> = [3, 1, 2, 5, 4]
    if List.contains_value<Int>(list: read numbers, value: read 5) {
        Output.write(message: read "contains")
    }
    match List.last<Int>(list: read numbers) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "last-none")
        }
    }
    let reversed = List.reverse<Int>(list: read numbers)
    Output.write(message: read String.from_int(value: reversed[0]))
    let skipped = List.skip<Int>(list: read numbers, count: 2)
    Output.write(message: read String.from_int(value: skipped[0]))
    let negative_skip = List.skip<Int>(list: read numbers, count: 0 - 2)
    Output.write(message: read String.from_int(value: negative_skip[0]))
    let taken = List.take<Int>(list: read numbers, count: 3)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read taken)))
    let negative_take = List.take<Int>(list: read numbers, count: 0 - 3)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read negative_take)))
    let sliced = List.slice<Int>(list: read numbers, start: 1, len: 3)
    Output.write(message: read String.from_int(value: sliced[0]))
    Output.write(message: read String.from_int(value: sliced[2]))
    let negative_slice = List.slice<Int>(list: read numbers, start: 0 - 2, len: 2)
    Output.write(message: read String.from_int(value: negative_slice[0]))
    let beyond = List.slice<Int>(list: read numbers, start: 99, len: 2)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read beyond)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-read-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_list_index_scan_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-index.rss", source, []);
}

#[test]
fn reg_vm_runs_map_literal_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let table: Map<String, Int> = {"one" => 1, "two" => 2, "three" => 3}
    let mut total = 0
    match Map.get<String, Int>(map: read table, key: read "one") {
        Some(value) => {
            total = total + value
        }
        None => {
            total = total - 10
        }
    }
    match Map.get<String, Int>(map: read table, key: read "three") {
        Some(value) => {
            total = total + value
        }
        None => {
            total = total - 10
        }
    }
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-literal.rss", source, []);
}

#[test]
fn reg_vm_runs_list_closure_intrinsics_like_interpreter() {
    let source = r#"

fn is_even(value: Int) -> Bool {
    let half = value / 2
    return half * 2 == value
}

fn main() -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4, 5]
    Output.write(message: read String.from_int(value: List.count_where<Int>(list: read numbers, predicate: |item| {
        return item > 3
    })))
    Output.write(message: read String.from_bool(value: List.any<Int>(list: read numbers, predicate: |item| {
        return item == 5
    })))
    Output.write(message: read String.from_bool(value: List.all<Int>(list: read numbers, predicate: |item| {
        return item > 0
    })))
    Output.write(message: read String.from_bool(value: List.contains<Int>(list: read numbers, predicate: |item| {
        return item == 3
    })))

    match List.find<Int>(list: read numbers, predicate: |item| {
        return item > 3
    }) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "find-none")
        }
    }

    let grouped = List.group_by<Int, String>(list: read numbers, key: |item| {
        if is_even(value: item) {
            return String.copy(value: read "even")
        }
        return String.copy(value: read "odd")
    })
    match Map.get(map: read grouped, key: read "even") {
        Some(items) => {
            Output.write(message: read String.from_int(value: List.len(list: read items)))
            Output.write(message: read String.from_int(value: items[0]))
        }
        None => {
            Output.write(message: read "even-missing")
        }
    }
    match Map.get(map: read grouped, key: read "odd") {
        Some(items) => {
            Output.write(message: read String.from_int(value: List.len(list: read items)))
            Output.write(message: read String.from_int(value: items[2]))
        }
        None => {
            Output.write(message: read "odd-missing")
        }
    }

    match List.try_fold<Int, Int, String>(list: read numbers, initial: read 0, folder: |state, item| {
        if item > 3 {
            return Err(String.copy(value: read "too-large"))
        }
        return Ok(state + item)
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    match List.try_fold<Int, Int, String>(list: read [2, 4], initial: read 0, folder: |state, item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(state + item)
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let filtered = List.filter<Int>(list: read numbers, predicate: |item| {
        return is_even(value: item)
    })
    let flattened = List.flat_map<Int, Int>(list: read filtered, mapper: |item| {
        let values: List<Int> = [item, item + 10]
        return values
    })
    Output.write(message: read String.from_int(value: List.len<Int>(list: read flattened)))
    Output.write(message: read String.from_int(value: flattened[1]))
    Output.write(message: read String.from_int(value: flattened[3]))

    let parts = List.partition<Int>(list: read numbers, predicate: |item| {
        return item > 3
    })
    Output.write(message: read String.from_int(value: List.len<Int>(list: read parts[0])))
    Output.write(message: read String.from_int(value: parts[0][0]))
    Output.write(message: read String.from_int(value: List.len<Int>(list: read parts[1])))
    Output.write(message: read String.from_int(value: parts[1][2]))

    let zip_left = [1, 2, 3]
    let zip_right = [4, 5]
    let zipped = List.zip<Int>(left: read zip_left, right: read zip_right)
    Output.write(message: read String.from_int(value: List.len(list: read zipped)))
    Output.write(message: read String.from_int(value: zipped[0][0]))
    Output.write(message: read String.from_int(value: zipped[0][1]))
    Output.write(message: read String.from_int(value: zipped[1][1]))

    let enumerate_values = [7, 8]
    let indexed = List.enumerate(list: read enumerate_values)
    Output.write(message: read String.from_int(value: indexed[0][0]))
    Output.write(message: read String.from_int(value: indexed[0][1]))
    Output.write(message: read String.from_int(value: indexed[1][0]))
    Output.write(message: read String.from_int(value: indexed[1][1]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-closure-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_list_closure_pipeline_like_interpreter() {
    let source = r#"

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
    let total = List.fold<Int, Int>(
        list: read filtered,
        initial: read 0,
        folder: |state, value| {
            return state + value
        },
    )

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-closure-pipeline.rss", source, []);
}

#[test]
fn reg_vm_runs_map_insert_lookup_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-lookup.rss", source, []);
}

#[test]
fn reg_vm_runs_string_key_map_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-string-key.rss", source, []);
}

#[test]
fn vm_runs_list_index_scan_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-list-index.rss", source, []);
}

#[test]
fn vm_runs_map_insert_lookup_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-map-lookup.rss", source, []);
}

#[test]
fn vm_runs_string_key_map_like_interpreter() {
    let source = r#"

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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-map-string-key.rss", source, []);
}

#[test]
fn vm_runs_list_closure_pipeline_like_interpreter() {
    let source = r#"

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

    Output.write(message: read String.from_int(value: acc.total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-list-closure-pipeline.rss", source, []);
}

#[test]
fn reg_vm_runs_list_mutator_and_json_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 3)
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.set<Int>(list: mut values, index: 1, value: read 10)
    let suffix: List<Int> = [4, 5]
    List.append<Int>(list: mut values, values: read suffix)
    match List.pop<Int>(list: mut values) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "pop-none")
        }
    }
    match List.remove_at<Int>(list: mut values, index: 1) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "remove-none")
        }
    }
    List.sort<Int>(list: mut values)
    Output.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    Output.write(message: read String.from_int(value: values[0]))
    Output.write(message: read String.from_int(value: values[2]))
    List.clear<Int>(list: mut values)
    if List.is_empty<Int>(list: read values) {
        Output.write(message: read "cleared")
    }

    let words: List<String> = ["b", "a"]
    let json_strings = List.to_json_strings(list: read words)
    Output.write(message: read Json.to_string(value: read json_strings))
    let json_values: List<JsonValue> = [Json.value(value: read {"n": 1}), Json.value(value: read {"n": 2})]
    let json_array = List.to_json_values(list: read json_values)
    Output.write(message: read Json.to_string(value: read json_array))

    local taken = List.take<Int>(list: read [1, 2], count: 1)
    List.consume<Int>(list: take taken)
    Output.write(message: read "consumed")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-mutator-json.rss", source, []);
}

#[test]
fn reg_vm_runs_map_mutator_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut table = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut table, key: read "one", value: read 1)
    Map.insert<String, Int>(map: mut table, key: read "two", value: read 2)
    match Map.insert_old<String, Int>(map: mut table, key: read "one", value: read 10) {
        Some(old) => {
            Output.write(message: read String.from_int(value: old))
        }
        None => {
            Output.write(message: read "insert-none")
        }
    }
    match Map.get<String, Int>(map: read table, key: read "one") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "missing")
        }
    }
    match Map.remove<String, Int>(map: mut table, key: read "two") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "remove-none")
        }
    }
    Output.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    Map.clear<String, Int>(map: mut table)
    Output.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-mutators.rss", source, []);
}

#[test]
fn reg_vm_runs_map_closure_intrinsics_like_interpreter() {
    let source = r#"
struct Acc {
    total: Int
}

fn main() -> Unit {
    let mut left = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut left, key: read "a", value: read 1)
    Map.insert<String, Int>(map: mut left, key: read "b", value: read 2)

    let mapped = Map.map_values<String, Int, Int>(map: read left, mapper: |value| {
        return value + 10
    })
    match Map.get<String, Int>(map: read mapped, key: read "a") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "mapped-missing")
        }
    }

    let filtered = Map.filter<String, Int>(map: read mapped, predicate: |key, value| {
        return key == "b" && value > 10
    })
    Output.write(message: read String.from_int(value: Map.len<String, Int>(map: read filtered)))
    match Map.get<String, Int>(map: read filtered, key: read "b") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "filtered-missing")
        }
    }

    let mut single = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut single, key: read "only", value: read 8)
    Map.for_each<String, Int>(map: read single, callback: |key, value| {
        Output.write(message: read key)
        Output.write(message: read String.from_int(value: value))
        return Unit
    })

    let folded = Map.fold<String, Int, Acc>(map: read left, initial: read Acc(total: 0), folder: |state, key, value| {
        if key == "a" {
            return Acc(total: state.total + value)
        }
        return Acc(total: state.total + value + 10)
    })
    Output.write(message: read String.from_int(value: folded.total))

    match Map.try_fold<String, Int, Acc, String>(map: read left, initial: read Acc(total: 0), folder: |state, key, value| {
        if key == "b" {
            return Err(String.copy(value: read "stop-b"))
        }
        return Ok(Acc(total: state.total + value))
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value.total))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let mut right = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut right, key: read "b", value: read 20)
    Map.insert<String, Int>(map: mut right, key: read "c", value: read 30)
    let merged = Map.merge<String, Int>(left: read left, right: read right, resolver: |left_value, right_value| {
        return left_value + right_value
    })
    match Map.get<String, Int>(map: read merged, key: read "b") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "merge-b-missing")
        }
    }
    match Map.get<String, Int>(map: read merged, key: read "c") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "merge-c-missing")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-closure-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_task_group_drains_unawaited_async_let_like_backend() {
    // Structured concurrency: leaving a `task_group` must drain background tasks
    // spawned by `async let` even when they are never explicitly awaited (here
    // `async let _ = background()`). The backend drains at the scope boundary, so
    // "background completed" must print before "after group".
    let source = r#"

async fn background() -> Result<Unit, String> {
    Output.write(message: read "background completed")
    return Ok(Unit)
}

fn main() -> Result<Unit, String> {
    task_group {
        async let _ = background()
    }
    Output.write(message: read "after group")
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-task-group-drain.rss", source, []);
}
