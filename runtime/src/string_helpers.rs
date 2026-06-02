pub fn string_from_int(value: i64) -> String {
    value.to_string()
}

pub fn string_copy(value: &str) -> String {
    value.to_string()
}

pub fn string_concat(left: &str, right: &str) -> String {
    format!("{left}{right}")
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

pub fn string_slice(value: &str, start: i64, len: i64) -> String {
    string_view_range(value, start, len).to_string()
}

pub fn string_index_of(value: &str, needle: &str) -> Option<i64> {
    value.find(needle).map(|index| index as i64)
}

pub fn string_repeat(value: &str, count: i64) -> String {
    value.repeat(count.max(0) as usize)
}

pub fn string_parse_int(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

pub fn string_view(value: &str, start: i64, len: i64) -> &str {
    string_view_range(value, start, len)
}

pub fn string_view_slice(value: &str, start: i64, len: i64) -> &str {
    string_view_range(value, start, len)
}

pub fn string_view_len(value: &str) -> i64 {
    value.len() as i64
}

pub fn string_view_is_empty(value: &str) -> bool {
    value.is_empty()
}

pub fn string_view_to_string(value: &str) -> String {
    value.to_string()
}

pub fn string_view_starts_with(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
}

pub fn string_view_contains(value: &str, needle: &str) -> bool {
    value.contains(needle)
}

pub fn string_view_before<'a>(value: &'a str, delimiter: &str) -> Option<&'a str> {
    let index = value.find(delimiter)?;
    Some(&value[..index])
}

pub fn string_view_after<'a>(value: &'a str, delimiter: &str) -> Option<&'a str> {
    let (_, right) = value.split_once(delimiter)?;
    Some(right)
}

fn string_view_range(value: &str, start: i64, len: i64) -> &str {
    let byte_start = clamp_to_char_boundary(value, start.max(0) as usize);
    let requested_end = byte_start.saturating_add(len.max(0) as usize);
    let byte_end = clamp_to_char_boundary(value, requested_end.min(value.len()));
    &value[byte_start..byte_end]
}

fn clamp_to_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_view_clamps_to_utf8_boundaries() {
        let value = "aébc";

        assert_eq!(string_view(value, 0, 3), "aé");
        assert_eq!(string_view(value, 2, 2), "é");
        assert_eq!(string_view_slice(value, 100, 5), "");
    }
}
