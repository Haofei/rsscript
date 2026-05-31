pub fn string_from_int(value: i64) -> String {
    value.to_string()
}

pub fn string_from_bool(value: bool) -> String {
    value.to_string()
}

pub fn string_len(value: &str) -> i64 {
    value.len() as i64
}

pub fn string_is_empty(value: &str) -> bool {
    value.is_empty()
}

pub fn string_starts_with(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
}

pub fn string_ends_with(value: &str, suffix: &str) -> bool {
    value.ends_with(suffix)
}

pub fn string_contains(value: &str, needle: &str) -> bool {
    value.contains(needle)
}

pub fn string_lines(value: &str) -> Vec<String> {
    value.lines().map(str::to_string).collect()
}

pub fn string_join(parts: &[String], separator: &str) -> String {
    parts.join(separator)
}

pub fn string_strip_prefix(value: &str, prefix: &str) -> Option<String> {
    value.strip_prefix(prefix).map(str::to_string)
}

pub fn string_before(value: &str, delimiter: &str) -> Option<String> {
    let index = value.find(delimiter)?;
    Some(value[..index].to_string())
}

pub fn string_after(value: &str, delimiter: &str) -> Option<String> {
    let (_, right) = value.split_once(delimiter)?;
    Some(right.to_string())
}

pub fn string_trim(value: &str) -> String {
    value.trim().to_string()
}

pub fn string_to_lowercase(value: &str) -> String {
    value.to_lowercase()
}

pub fn string_to_uppercase(value: &str) -> String {
    value.to_uppercase()
}

pub fn string_replace(value: &str, from: &str, to: &str) -> String {
    value.replace(from, to)
}

pub fn string_split(value: &str, delimiter: &str) -> Vec<String> {
    value.split(delimiter).map(str::to_string).collect()
}

pub fn string_builder_new() -> String {
    String::new()
}

pub fn string_builder_push(builder: &mut String, value: &str) {
    builder.push_str(value);
}

pub fn string_builder_finish(builder: String) -> String {
    builder
}
