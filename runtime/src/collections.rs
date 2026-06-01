use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub fn ord_compare<T: Ord>(left: &T, right: &T) -> i64 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

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

pub fn list_sort<T: Ord>(list: &mut [T]) {
    list.sort();
}

pub fn list_sort_with<T: Clone>(list: &mut [T], mut compare: impl FnMut(T, T) -> i64) {
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

pub fn list_first<T: Clone>(list: &[T]) -> Option<T> {
    list.first().cloned()
}

pub fn list_flat_map<T: Clone, U>(list: &[T], mut mapper: impl FnMut(T) -> Vec<U>) -> Vec<U> {
    let mut result = Vec::new();
    for item in list.iter().cloned() {
        result.extend(mapper(item));
    }
    result
}

pub fn list_group_by<T: Clone, K: Eq + Hash>(
    list: &[T],
    mut key: impl FnMut(T) -> K,
) -> HashMap<K, Vec<T>> {
    let mut groups = HashMap::new();
    for item in list.iter().cloned() {
        let group_key = key(item.clone());
        groups.entry(group_key).or_insert_with(Vec::new).push(item);
    }
    groups
}

pub fn list_join(list: &[String], separator: &str) -> String {
    list.join(separator)
}

pub fn list_last<T: Clone>(list: &[T]) -> Option<T> {
    list.last().cloned()
}

pub fn list_partition<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> Vec<Vec<T>> {
    let mut matches = Vec::new();
    let mut rest = Vec::new();
    for item in list.iter().cloned() {
        if predicate(item.clone()) {
            matches.push(item);
        } else {
            rest.push(item);
        }
    }
    vec![matches, rest]
}

pub fn list_skip<T: Clone>(list: &[T], count: i64) -> Vec<T> {
    list.iter().skip(count.max(0) as usize).cloned().collect()
}

pub fn list_sort_by<T: Clone, K>(
    list: &[T],
    mut key: impl FnMut(T) -> K,
    mut compare: impl FnMut(K, K) -> i64,
) -> Vec<T> {
    let mut result = list.to_vec();
    result.sort_by(|a, b| {
        let ordering = compare(key(a.clone()), key(b.clone()));
        if ordering < 0 {
            std::cmp::Ordering::Less
        } else if ordering > 0 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    result
}

pub fn list_take<T: Clone>(list: &[T], count: i64) -> Vec<T> {
    list.iter().take(count.max(0) as usize).cloned().collect()
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

pub fn map_filter<K: Eq + Hash + Clone, V: Clone>(
    map: &HashMap<K, V>,
    mut predicate: impl FnMut(K, V) -> bool,
) -> HashMap<K, V> {
    map.iter()
        .filter_map(|(key, value)| {
            let key = key.clone();
            let value = value.clone();
            predicate(key.clone(), value.clone()).then_some((key, value))
        })
        .collect()
}

pub fn map_fold<K: Clone, V: Clone, U: Clone>(
    map: &HashMap<K, V>,
    initial: &U,
    mut folder: impl FnMut(U, K, V) -> U,
) -> U {
    let mut state = initial.clone();
    for (key, value) in map {
        state = folder(state, key.clone(), value.clone());
    }
    state
}

pub fn map_map_values<K: Eq + Hash + Clone, V: Clone, U>(
    map: &HashMap<K, V>,
    mut mapper: impl FnMut(V) -> U,
) -> HashMap<K, U> {
    map.iter()
        .map(|(key, value)| (key.clone(), mapper(value.clone())))
        .collect()
}

pub fn map_merge<K: Eq + Hash + Clone, V: Clone>(
    left: &HashMap<K, V>,
    right: &HashMap<K, V>,
    mut resolver: impl FnMut(V, V) -> V,
) -> HashMap<K, V> {
    let mut merged = left.clone();
    for (key, value) in right {
        if let Some(existing) = merged.get(key).cloned() {
            merged.insert(key.clone(), resolver(existing, value.clone()));
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

pub fn map_try_fold<K: Clone, V: Clone, U: Clone, E>(
    map: &HashMap<K, V>,
    initial: &U,
    mut folder: impl FnMut(U, K, V) -> Result<U, E>,
) -> Result<U, E> {
    let mut state = initial.clone();
    for (key, value) in map {
        state = folder(state, key.clone(), value.clone())?;
    }
    Ok(state)
}

pub fn option_and_then<T: Clone, U>(
    value: &Option<T>,
    mapper: impl FnMut(T) -> Option<U>,
) -> Option<U> {
    value.as_ref().cloned().and_then(mapper)
}

pub fn option_is_none<T>(value: &Option<T>) -> bool {
    value.is_none()
}

pub fn option_is_some<T>(value: &Option<T>) -> bool {
    value.is_some()
}

pub fn option_map<T: Clone, U>(value: &Option<T>, mapper: impl FnMut(T) -> U) -> Option<U> {
    value.as_ref().cloned().map(mapper)
}

pub fn option_ok_or<T: Clone, E: Clone>(value: &Option<T>, error: &E) -> Result<T, E> {
    value.as_ref().cloned().ok_or_else(|| error.clone())
}

pub fn option_unwrap_or<T: Clone>(value: &Option<T>, default: &T) -> T {
    value.as_ref().cloned().unwrap_or_else(|| default.clone())
}

pub fn option_unwrap_or_else<T: Clone>(value: &Option<T>, default: impl FnOnce() -> T) -> T {
    value.as_ref().cloned().unwrap_or_else(default)
}

pub fn result_and_then<T: Clone, E: Clone, U>(
    value: &Result<T, E>,
    mut mapper: impl FnMut(T) -> Result<U, E>,
) -> Result<U, E> {
    match value {
        Ok(ok) => mapper(ok.clone()),
        Err(error) => Err(error.clone()),
    }
}

pub fn result_err_message<T, E: ToString>(value: &Result<T, E>) -> Option<String> {
    match value {
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    }
}

pub fn result_is_err<T, E>(value: &Result<T, E>) -> bool {
    value.is_err()
}

pub fn result_is_ok<T, E>(value: &Result<T, E>) -> bool {
    value.is_ok()
}

pub fn result_map<T: Clone, E: Clone, U>(
    value: &Result<T, E>,
    mut mapper: impl FnMut(T) -> U,
) -> Result<U, E> {
    match value {
        Ok(ok) => Ok(mapper(ok.clone())),
        Err(error) => Err(error.clone()),
    }
}

pub fn result_map_error<T: Clone, E: Clone, F>(
    value: &Result<T, E>,
    mut mapper: impl FnMut(E) -> F,
) -> Result<T, F> {
    match value {
        Ok(ok) => Ok(ok.clone()),
        Err(error) => Err(mapper(error.clone())),
    }
}

pub fn result_ok<T: Clone, E>(value: &Result<T, E>) -> Option<T> {
    match value {
        Ok(ok) => Some(ok.clone()),
        Err(_) => None,
    }
}

pub fn result_unwrap_or<T: Clone, E>(value: &Result<T, E>, default: &T) -> T {
    match value {
        Ok(ok) => ok.clone(),
        Err(_) => default.clone(),
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
