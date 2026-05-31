use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub fn list_new<T>() -> Vec<T> {
    Vec::new()
}

pub fn list_push<T: Clone>(list: &mut Vec<T>, value: &T) {
    list.push(value.clone());
}

pub fn list_len<T>(list: &[T]) -> i64 {
    list.len() as i64
}

pub fn list_get<T: Clone>(list: &[T], index: i64) -> T {
    list[index as usize].clone()
}

pub fn list_count_where<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> i64 {
    list.iter()
        .filter(|item| predicate((*item).clone()))
        .count() as i64
}

pub fn list_any<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().any(|item| predicate(item.clone()))
}

pub fn list_all<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().all(|item| predicate(item.clone()))
}

pub fn list_find<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> Option<T> {
    list.iter().find(|item| predicate((*item).clone())).cloned()
}

pub fn list_filter<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> Vec<T> {
    list.iter()
        .filter(|item| predicate((*item).clone()))
        .cloned()
        .collect()
}

pub fn list_map<T: Clone, U>(list: &[T], mapper: impl FnMut(T) -> U) -> Vec<U> {
    list.iter().cloned().map(mapper).collect()
}

pub fn list_fold<T: Clone, U: Clone>(
    list: &[T],
    initial: &U,
    mut folder: impl FnMut(U, T) -> U,
) -> U {
    let mut state = initial.clone();
    for item in list.iter().cloned() {
        state = folder(state, item);
    }
    state
}

pub fn list_try_fold<T: Clone, U: Clone, E>(
    list: &[T],
    initial: &U,
    mut folder: impl FnMut(U, T) -> Result<U, E>,
) -> Result<U, E> {
    let mut state = initial.clone();
    for item in list.iter().cloned() {
        state = folder(state, item)?;
    }
    Ok(state)
}

pub fn list_consume<T>(list: Vec<T>) {
    drop(list);
}

pub fn list_contains<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().any(|item| predicate(item.clone()))
}

pub fn list_is_empty<T>(list: &[T]) -> bool {
    list.is_empty()
}

pub fn list_reverse<T: Clone>(list: &[T]) -> Vec<T> {
    let mut result: Vec<T> = list.to_vec();
    result.reverse();
    result
}

pub fn list_sort<T: Clone>(list: &mut Vec<T>, mut compare: impl FnMut(T, T) -> i64) {
    list.sort_by(|a, b| {
        let result = compare(a.clone(), b.clone());
        if result < 0 {
            std::cmp::Ordering::Less
        } else if result > 0 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

pub fn map_new<K, V>() -> HashMap<K, V> {
    HashMap::new()
}

pub fn map_len<K, V>(map: &HashMap<K, V>) -> i64 {
    map.len() as i64
}

pub fn map_is_empty<K, V>(map: &HashMap<K, V>) -> bool {
    map.is_empty()
}

pub fn map_contains_key<K: Eq + Hash, V>(map: &HashMap<K, V>, key: &K) -> bool {
    map.contains_key(key)
}

pub fn map_get<K: Eq + Hash, V: Clone>(map: &HashMap<K, V>, key: &K) -> Option<V> {
    map.get(key).cloned()
}

pub fn map_insert<K: Eq + Hash + Clone, V: Clone>(map: &mut HashMap<K, V>, key: &K, value: &V) {
    map.insert(key.clone(), value.clone());
}

pub fn map_remove<K: Eq + Hash, V>(map: &mut HashMap<K, V>, key: &K) -> Option<V> {
    map.remove(key)
}

pub fn map_clear<K, V>(map: &mut HashMap<K, V>) {
    map.clear();
}

pub fn map_keys<K: Clone, V>(map: &HashMap<K, V>) -> Vec<K> {
    map.keys().cloned().collect()
}

pub fn map_values<K, V: Clone>(map: &HashMap<K, V>) -> Vec<V> {
    map.values().cloned().collect()
}

pub fn map_for_each<K: Clone, V: Clone>(map: &HashMap<K, V>, mut callback: impl FnMut(K, V)) {
    for (key, value) in map {
        callback(key.clone(), value.clone());
    }
}

pub fn set_new<T>() -> HashSet<T> {
    HashSet::new()
}

pub fn set_len<T>(set: &HashSet<T>) -> i64 {
    set.len() as i64
}

pub fn set_is_empty<T>(set: &HashSet<T>) -> bool {
    set.is_empty()
}

pub fn set_contains<T: Eq + Hash>(set: &HashSet<T>, value: &T) -> bool {
    set.contains(value)
}

pub fn set_insert<T: Eq + Hash + Clone>(set: &mut HashSet<T>, value: &T) -> bool {
    set.insert(value.clone())
}

pub fn set_remove<T: Eq + Hash>(set: &mut HashSet<T>, value: &T) -> bool {
    set.remove(value)
}

pub fn set_clear<T>(set: &mut HashSet<T>) {
    set.clear();
}

pub fn set_to_list<T: Clone>(set: &HashSet<T>) -> Vec<T> {
    set.iter().cloned().collect()
}

pub fn set_for_each<T: Clone>(set: &HashSet<T>, mut callback: impl FnMut(T)) {
    for value in set {
        callback(value.clone());
    }
}

pub fn buffer_new(size: i64) -> Vec<u8> {
    Vec::with_capacity(size.max(0) as usize)
}

pub fn buffer_clear(buffer: &mut Vec<u8>) {
    buffer.clear();
}

pub fn buffer_consume(buffer: Vec<u8>) {
    drop(buffer);
}

pub fn bytes_from_string(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

pub fn bytes_from_buffer(buffer: &[u8]) -> Vec<u8> {
    buffer.to_vec()
}

pub fn bytes_consume(bytes: Vec<u8>) {
    drop(bytes);
}

pub fn url_from_string(value: &str) -> String {
    value.to_string()
}
