use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::Hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssPipeline<T> {
    pub items: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssFalliblePipeline<T, E> {
    pub result: Result<Vec<T>, E>,
}

pub fn clone_value<T: Clone>(value: &T) -> T {
    value.clone()
}

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

pub fn deque_new<T>() -> VecDeque<T> {
    VecDeque::new()
}

pub fn deque_push_back<T: Clone>(deque: &mut VecDeque<T>, value: &T) {
    deque.push_back(value.clone());
}

pub fn deque_push_front<T: Clone>(deque: &mut VecDeque<T>, value: &T) {
    deque.push_front(value.clone());
}

pub fn deque_pop_back<T>(deque: &mut VecDeque<T>) -> Option<T> {
    deque.pop_back()
}

pub fn deque_pop_front<T>(deque: &mut VecDeque<T>) -> Option<T> {
    deque.pop_front()
}

pub fn deque_clear<T>(deque: &mut VecDeque<T>) {
    deque.clear();
}

pub fn deque_len<T>(deque: &VecDeque<T>) -> i64 {
    deque.len() as i64
}

pub fn deque_is_empty<T>(deque: &VecDeque<T>) -> bool {
    deque.is_empty()
}

pub fn deque_to_list<T: Clone>(deque: &VecDeque<T>) -> Vec<T> {
    deque.iter().cloned().collect()
}

pub fn list_push<T: Clone>(list: &mut Vec<T>, value: &T) {
    list.push(value.clone());
}

pub fn list_set<T: Clone>(list: &mut [T], index: i64, value: &T) {
    list[index as usize] = value.clone();
}

pub fn list_pop<T>(list: &mut Vec<T>) -> Option<T> {
    list.pop()
}

pub fn list_clear<T>(list: &mut Vec<T>) {
    list.clear();
}

pub fn list_append<T: Clone>(list: &mut Vec<T>, values: &[T]) {
    list.extend_from_slice(values);
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

pub fn pipeline_from_list<T: Clone>(list: &[T]) -> RssPipeline<T> {
    RssPipeline {
        items: list.to_vec(),
    }
}

pub fn pipeline_map<T: Clone, U>(
    pipeline: &RssPipeline<T>,
    mapper: impl FnMut(T) -> U,
) -> RssPipeline<U> {
    RssPipeline {
        items: pipeline.items.iter().cloned().map(mapper).collect(),
    }
}

pub fn pipeline_filter<T: Clone>(
    pipeline: &RssPipeline<T>,
    mut predicate: impl FnMut(T) -> bool,
) -> RssPipeline<T> {
    RssPipeline {
        items: pipeline
            .items
            .iter()
            .filter(|item| predicate((*item).clone()))
            .cloned()
            .collect(),
    }
}

pub fn pipeline_each<T: Clone>(
    pipeline: &RssPipeline<T>,
    mut action: impl FnMut(T),
) -> RssPipeline<T> {
    for item in pipeline.items.iter().cloned() {
        action(item);
    }
    RssPipeline {
        items: pipeline.items.clone(),
    }
}

pub fn pipeline_try_map<T: Clone, U, E>(
    pipeline: &RssPipeline<T>,
    mut mapper: impl FnMut(T) -> Result<U, E>,
) -> RssFalliblePipeline<U, E> {
    let mut mapped = Vec::new();
    for item in pipeline.items.iter().cloned() {
        match mapper(item) {
            Ok(value) => mapped.push(value),
            Err(error) => return RssFalliblePipeline { result: Err(error) },
        }
    }
    RssFalliblePipeline { result: Ok(mapped) }
}

pub fn pipeline_collect<T: Clone>(pipeline: &RssPipeline<T>) -> Vec<T> {
    pipeline.items.clone()
}

pub fn fallible_pipeline_map<T: Clone, U, E: Clone>(
    pipeline: &RssFalliblePipeline<T, E>,
    mapper: impl FnMut(T) -> U,
) -> RssFalliblePipeline<U, E> {
    RssFalliblePipeline {
        result: pipeline
            .result
            .as_ref()
            .map(|items| items.iter().cloned().map(mapper).collect::<Vec<_>>())
            .map_err(Clone::clone),
    }
}

pub fn fallible_pipeline_filter<T: Clone, E: Clone>(
    pipeline: &RssFalliblePipeline<T, E>,
    mut predicate: impl FnMut(T) -> bool,
) -> RssFalliblePipeline<T, E> {
    RssFalliblePipeline {
        result: pipeline
            .result
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .filter(|item| predicate((*item).clone()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .map_err(Clone::clone),
    }
}

pub fn fallible_pipeline_each<T: Clone, E: Clone>(
    pipeline: &RssFalliblePipeline<T, E>,
    mut action: impl FnMut(T),
) -> RssFalliblePipeline<T, E> {
    if let Ok(items) = &pipeline.result {
        for item in items.iter().cloned() {
            action(item);
        }
    }
    RssFalliblePipeline {
        result: pipeline.result.clone(),
    }
}

pub fn fallible_pipeline_try_map<T: Clone, U, E: Clone>(
    pipeline: &RssFalliblePipeline<T, E>,
    mut mapper: impl FnMut(T) -> Result<U, E>,
) -> RssFalliblePipeline<U, E> {
    let items = match &pipeline.result {
        Ok(items) => items,
        Err(error) => {
            return RssFalliblePipeline {
                result: Err(error.clone()),
            };
        }
    };
    let mut mapped = Vec::new();
    for item in items.iter().cloned() {
        match mapper(item) {
            Ok(value) => mapped.push(value),
            Err(error) => return RssFalliblePipeline { result: Err(error) },
        }
    }
    RssFalliblePipeline { result: Ok(mapped) }
}

pub fn fallible_pipeline_collect<T: Clone, E: Clone>(
    pipeline: &RssFalliblePipeline<T, E>,
) -> Result<Vec<T>, E> {
    pipeline.result.clone()
}

pub fn list_consume<T>(list: Vec<T>) {
    drop(list);
}

pub fn list_contains<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().any(|item| predicate(item.clone()))
}

pub fn list_contains_value<T: PartialEq>(list: &[T], value: &T) -> bool {
    list.contains(value)
}

pub fn list_is_empty<T>(list: &[T]) -> bool {
    list.is_empty()
}

pub fn list_sum(list: &[i64]) -> i64 {
    list.iter()
        .copied()
        .try_fold(0_i64, i64::checked_add)
        .unwrap_or_else(|| {
            crate::error::panic_runtime_error(crate::error::integer_overflow_error(
                "List.sum overflow exceeds the Int range".to_string(),
            ))
        })
}

pub fn list_min(list: &[i64]) -> Option<i64> {
    list.iter().copied().min()
}

pub fn list_max(list: &[i64]) -> Option<i64> {
    list.iter().copied().max()
}

pub fn list_flatten<T: Clone>(list: &[Vec<T>]) -> Vec<T> {
    list.iter()
        .flat_map(|items| items.iter().cloned())
        .collect()
}

pub fn list_dedup<T: Clone + PartialEq>(list: &[T]) -> Vec<T> {
    let mut result = Vec::new();
    for item in list {
        if !result.contains(item) {
            result.push(item.clone());
        }
    }
    result
}

pub fn list_zip<T: Clone>(left: &[T], right: &[T]) -> Vec<Vec<T>> {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| vec![left.clone(), right.clone()])
        .collect()
}

pub fn list_enumerate(list: &[i64]) -> Vec<Vec<i64>> {
    list.iter()
        .enumerate()
        .map(|(index, value)| vec![index as i64, *value])
        .collect()
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

pub fn list_remove_at<T>(list: &mut Vec<T>, index: i64) -> Option<T> {
    let index = usize::try_from(index).ok()?;
    (index < list.len()).then(|| list.remove(index))
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

pub fn list_slice<T: Clone>(list: &[T], start: i64, len: i64) -> Vec<T> {
    let start = start.max(0) as usize;
    if start >= list.len() {
        return Vec::new();
    }
    let end = start.saturating_add(len.max(0) as usize).min(list.len());
    list[start..end].to_vec()
}

pub fn map_new<K, V>() -> HashMap<K, V> {
    HashMap::new()
}

pub type RssPersistentMap<K, V> = imbl::HashMap<K, V>;

pub fn persistent_map_new<K, V>() -> RssPersistentMap<K, V> {
    RssPersistentMap::new()
}

pub fn persistent_map_len<K, V>(map: &RssPersistentMap<K, V>) -> i64 {
    map.len() as i64
}

pub fn persistent_map_is_empty<K, V>(map: &RssPersistentMap<K, V>) -> bool {
    map.is_empty()
}

pub fn persistent_map_contains_key<K: Eq + Hash, V>(map: &RssPersistentMap<K, V>, key: &K) -> bool {
    map.contains_key(key)
}

pub fn persistent_map_get<K: Eq + Hash, V: Clone>(
    map: &RssPersistentMap<K, V>,
    key: &K,
) -> Option<V> {
    map.get(key).cloned()
}

pub fn persistent_map_insert<K: Eq + Hash + Clone, V: Clone>(
    map: &RssPersistentMap<K, V>,
    key: &K,
    value: &V,
) -> RssPersistentMap<K, V> {
    map.update(key.clone(), value.clone())
}

pub fn persistent_map_remove<K: Eq + Hash + Clone, V: Clone>(
    map: &RssPersistentMap<K, V>,
    key: &K,
) -> RssPersistentMap<K, V> {
    map.without(key)
}

pub fn persistent_map_clear<K, V>(_map: &RssPersistentMap<K, V>) -> RssPersistentMap<K, V> {
    RssPersistentMap::new()
}

pub fn sorted_map_new<K, V>() -> BTreeMap<K, V> {
    BTreeMap::new()
}

pub fn sorted_map_len<K, V>(map: &BTreeMap<K, V>) -> i64 {
    map.len() as i64
}

pub fn sorted_map_is_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}

pub fn sorted_map_contains_key<K: Ord, V>(map: &BTreeMap<K, V>, key: &K) -> bool {
    map.contains_key(key)
}

pub fn sorted_map_get<K: Ord, V: Clone>(map: &BTreeMap<K, V>, key: &K) -> Option<V> {
    map.get(key).cloned()
}

pub fn sorted_map_insert<K: Ord + Clone, V: Clone>(map: &mut BTreeMap<K, V>, key: &K, value: &V) {
    map.insert(key.clone(), value.clone());
}

pub fn sorted_map_remove<K: Ord, V>(map: &mut BTreeMap<K, V>, key: &K) -> Option<V> {
    map.remove(key)
}

pub fn sorted_map_clear<K, V>(map: &mut BTreeMap<K, V>) {
    map.clear();
}

pub fn sorted_map_keys<K: Clone, V>(map: &BTreeMap<K, V>) -> Vec<K> {
    map.keys().cloned().collect()
}

pub fn sorted_map_values<K, V: Clone>(map: &BTreeMap<K, V>) -> Vec<V> {
    map.values().cloned().collect()
}

pub fn map_from_entries<K: Eq + Hash, V>(entries: Vec<(K, V)>) -> HashMap<K, V> {
    entries.into_iter().collect()
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

pub fn map_get_or_default<K: Eq + Hash, V: Clone>(map: &HashMap<K, V>, key: &K, default: &V) -> V {
    map.get(key).cloned().unwrap_or_else(|| default.clone())
}

pub fn map_insert<K: Eq + Hash + Clone, V: Clone>(map: &mut HashMap<K, V>, key: &K, value: &V) {
    map.insert(key.clone(), value.clone());
}

pub fn map_insert_old<K: Eq + Hash + Clone, V: Clone>(
    map: &mut HashMap<K, V>,
    key: &K,
    value: &V,
) -> Option<V> {
    map.insert(key.clone(), value.clone())
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

pub fn option_or<T: Clone>(value: &Option<T>, fallback: &Option<T>) -> Option<T> {
    value.clone().or_else(|| fallback.clone())
}

pub fn option_filter<T: Clone>(
    value: &Option<T>,
    mut predicate: impl FnMut(T) -> bool,
) -> Option<T> {
    value
        .as_ref()
        .cloned()
        .and_then(|item| predicate(item.clone()).then_some(item))
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

pub fn result_err<T, E: Clone>(value: &Result<T, E>) -> Option<E> {
    match value {
        Ok(_) => None,
        Err(error) => Some(error.clone()),
    }
}

pub fn result_unwrap_or<T: Clone, E>(value: &Result<T, E>, default: &T) -> T {
    match value {
        Ok(ok) => ok.clone(),
        Err(_) => default.clone(),
    }
}

pub fn result_unwrap_or_else<T: Clone, E: Clone>(
    value: &Result<T, E>,
    mut fallback: impl FnMut(E) -> T,
) -> T {
    match value {
        Ok(ok) => ok.clone(),
        Err(error) => fallback(error.clone()),
    }
}

pub fn set_new<T>() -> HashSet<T> {
    HashSet::new()
}

pub fn sorted_set_new<T>() -> BTreeSet<T> {
    BTreeSet::new()
}

pub fn sorted_set_len<T>(set: &BTreeSet<T>) -> i64 {
    set.len() as i64
}

pub fn sorted_set_is_empty<T>(set: &BTreeSet<T>) -> bool {
    set.is_empty()
}

pub fn sorted_set_contains<T: Ord>(set: &BTreeSet<T>, value: &T) -> bool {
    set.contains(value)
}

pub fn sorted_set_insert<T: Ord + Clone>(set: &mut BTreeSet<T>, value: &T) -> bool {
    set.insert(value.clone())
}

pub fn sorted_set_remove<T: Ord>(set: &mut BTreeSet<T>, value: &T) -> bool {
    set.remove(value)
}

pub fn sorted_set_clear<T>(set: &mut BTreeSet<T>) {
    set.clear();
}

pub fn sorted_set_to_list<T: Clone>(set: &BTreeSet<T>) -> Vec<T> {
    set.iter().cloned().collect()
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

pub fn set_union<T: Eq + Hash + Clone>(left: &HashSet<T>, right: &HashSet<T>) -> HashSet<T> {
    left.union(right).cloned().collect()
}

pub fn set_intersection<T: Eq + Hash + Clone>(left: &HashSet<T>, right: &HashSet<T>) -> HashSet<T> {
    left.intersection(right).cloned().collect()
}

pub fn set_difference<T: Eq + Hash + Clone>(left: &HashSet<T>, right: &HashSet<T>) -> HashSet<T> {
    left.difference(right).cloned().collect()
}

pub fn set_is_subset<T: Eq + Hash>(left: &HashSet<T>, right: &HashSet<T>) -> bool {
    left.is_subset(right)
}

pub fn buffer_new(size: i64) -> Vec<u8> {
    Vec::with_capacity(size.max(0) as usize)
}

pub fn buffer_len(buffer: &[u8]) -> i64 {
    buffer.len() as i64
}

pub fn buffer_is_empty(buffer: &[u8]) -> bool {
    buffer.is_empty()
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

pub fn bytes_from_uints(values: &[i64]) -> Vec<u8> {
    values.iter().map(|value| *value as u8).collect()
}

pub fn bytes_to_uints(value: &[u8]) -> Vec<i64> {
    value.iter().map(|byte| i64::from(*byte)).collect()
}

pub fn bytes_to_string(value: &[u8]) -> String {
    String::from_utf8_lossy(value).to_string()
}

pub fn bytes_from_buffer(buffer: &[u8]) -> Vec<u8> {
    buffer.to_vec()
}

pub fn bytes_len(value: &[u8]) -> i64 {
    value.len() as i64
}

pub fn bytes_is_empty(value: &[u8]) -> bool {
    value.is_empty()
}

pub fn bytes_concat(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(left.len() + right.len());
    result.extend_from_slice(left);
    result.extend_from_slice(right);
    result
}

pub fn bytes_slice(value: &[u8], start: i64, len: i64) -> Vec<u8> {
    bytes_view_range(value, start, len).to_vec()
}

pub fn bytes_view(value: &[u8], start: i64, len: i64) -> &[u8] {
    bytes_view_range(value, start, len)
}

pub fn bytes_consume(bytes: Vec<u8>) {
    drop(bytes);
}

pub fn bytes_view_slice(value: &[u8], start: i64, len: i64) -> &[u8] {
    bytes_view_range(value, start, len)
}

pub fn bytes_view_len(value: &[u8]) -> i64 {
    value.len() as i64
}

pub fn bytes_view_is_empty(value: &[u8]) -> bool {
    value.is_empty()
}

pub fn bytes_view_to_bytes(value: &[u8]) -> Vec<u8> {
    value.to_vec()
}

pub fn bytes_view_starts_with(value: &[u8], prefix: &[u8]) -> bool {
    value.starts_with(prefix)
}

pub fn buffer_view(buffer: &[u8], start: i64, len: i64) -> &[u8] {
    bytes_view_range(buffer, start, len)
}

pub fn buffer_view_slice(value: &[u8], start: i64, len: i64) -> &[u8] {
    bytes_view_range(value, start, len)
}

pub fn buffer_view_len(value: &[u8]) -> i64 {
    value.len() as i64
}

pub fn buffer_view_is_empty(value: &[u8]) -> bool {
    value.is_empty()
}

pub fn buffer_view_to_bytes(value: &[u8]) -> Vec<u8> {
    value.to_vec()
}

fn bytes_view_range(value: &[u8], start: i64, len: i64) -> &[u8] {
    let start = (start.max(0) as usize).min(value.len());
    let end = start.saturating_add(len.max(0) as usize).min(value.len());
    &value[start..end]
}

pub fn url_from_string(value: &str) -> String {
    value.to_string()
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn bytes_and_buffer_views_clamp_to_available_range() {
        let bytes = b"abcdef";

        assert_eq!(bytes_view(bytes, 2, 3), b"cde");
        assert_eq!(bytes_view(bytes, 20, 3), b"");
        assert_eq!(buffer_view(bytes, 4, 20), b"ef");
    }

    #[test]
    fn list_mutation_helpers_cover_common_updates() {
        let mut items = list_new();
        list_push(&mut items, &1);
        list_push(&mut items, &2);
        list_set(&mut items, 1, &3);
        list_append(&mut items, &[4, 5]);

        assert_eq!(items, vec![1, 3, 4, 5]);
        assert_eq!(list_slice(&items, 1, 2), vec![3, 4]);
        assert_eq!(list_slice(&items, 99, 2), Vec::<i32>::new());
        assert_eq!(list_pop(&mut items), Some(5));

        list_clear(&mut items);
        assert!(items.is_empty());
    }

    #[test]
    fn deque_helpers_cover_both_ends() {
        let mut deque = deque_new();
        deque_push_back(&mut deque, &2);
        deque_push_front(&mut deque, &1);
        deque_push_back(&mut deque, &3);

        assert_eq!(deque_len(&deque), 3);
        assert!(!deque_is_empty(&deque));
        assert_eq!(deque_to_list(&deque), vec![1, 2, 3]);
        assert_eq!(deque_pop_front(&mut deque), Some(1));
        assert_eq!(deque_pop_back(&mut deque), Some(3));
        assert_eq!(deque_pop_front(&mut deque), Some(2));
        assert_eq!(deque_pop_front(&mut deque), None);

        deque_push_back(&mut deque, &4);
        deque_clear(&mut deque);
        assert!(deque_is_empty(&deque));
    }

    #[test]
    fn sorted_collections_iterate_in_key_order() {
        let mut map = sorted_map_new();
        sorted_map_insert(&mut map, &2, &"two".to_string());
        sorted_map_insert(&mut map, &1, &"one".to_string());
        sorted_map_insert(&mut map, &3, &"three".to_string());

        assert_eq!(sorted_map_len(&map), 3);
        assert!(sorted_map_contains_key(&map, &2));
        assert_eq!(sorted_map_get(&map, &1), Some("one".to_string()));
        assert_eq!(sorted_map_keys(&map), vec![1, 2, 3]);
        assert_eq!(
            sorted_map_values(&map),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
        assert_eq!(sorted_map_remove(&mut map, &2), Some("two".to_string()));
        sorted_map_clear(&mut map);
        assert!(sorted_map_is_empty(&map));

        let mut set = sorted_set_new();
        assert!(sorted_set_insert(&mut set, &3));
        assert!(sorted_set_insert(&mut set, &1));
        assert!(sorted_set_insert(&mut set, &2));
        assert!(!sorted_set_insert(&mut set, &2));

        assert_eq!(sorted_set_len(&set), 3);
        assert!(sorted_set_contains(&set, &1));
        assert_eq!(sorted_set_to_list(&set), vec![1, 2, 3]);
        assert!(sorted_set_remove(&mut set, &2));
        sorted_set_clear(&mut set);
        assert!(sorted_set_is_empty(&set));
    }

    #[test]
    fn persistent_map_updates_return_fresh_maps() {
        let empty = persistent_map_new();
        let one = persistent_map_insert(&empty, &"one".to_string(), &1);
        let two = persistent_map_insert(&one, &"two".to_string(), &2);
        let removed = persistent_map_remove(&two, &"one".to_string());
        let cleared = persistent_map_clear(&two);

        assert!(persistent_map_is_empty(&empty));
        assert_eq!(persistent_map_len(&one), 1);
        assert_eq!(persistent_map_len(&two), 2);
        assert_eq!(persistent_map_get(&one, &"one".to_string()), Some(1));
        assert_eq!(persistent_map_get(&empty, &"one".to_string()), None);
        assert!(persistent_map_contains_key(&two, &"two".to_string()));
        assert_eq!(persistent_map_get(&removed, &"one".to_string()), None);
        assert_eq!(persistent_map_get(&two, &"one".to_string()), Some(1));
        assert!(persistent_map_is_empty(&cleared));
    }

    #[test]
    fn list_sum_uses_structured_checked_overflow() {
        assert_eq!(list_sum(&[]), 0);
        assert_eq!(list_sum(&[i64::MAX, i64::MIN]), -1);
        let panic =
            std::panic::catch_unwind(|| list_sum(&[i64::MAX, 1])).expect_err("overflow must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("integer_overflow"), "{message}");
    }
}
