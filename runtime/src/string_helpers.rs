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

pub fn string_chars(value: &str) -> Vec<char> {
    value.chars().collect()
}

pub fn char_is_digit(value: &char) -> bool {
    value.is_ascii_digit()
}

pub fn char_is_alpha(value: &char) -> bool {
    value.is_ascii_alphabetic()
}

pub fn char_is_whitespace(value: &char) -> bool {
    value.is_whitespace()
}

pub fn char_is_alphanumeric(value: &char) -> bool {
    value.is_ascii_alphanumeric()
}

pub fn char_to_code(value: &char) -> i64 {
    *value as u32 as i64
}

pub fn char_from_code(value: i64) -> Option<char> {
    u32::try_from(value).ok().and_then(char::from_u32)
}

pub fn char_compare(left: &char, right: &char) -> i64 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

pub fn char_to_string(value: &char) -> String {
    value.to_string()
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

    #[test]
    fn char_helpers_cover_lexer_classification() {
        assert_eq!(string_chars("a1 \n"), vec!['a', '1', ' ', '\n']);
        assert!(char_is_alpha(&'a'));
        assert!(char_is_digit(&'1'));
        assert!(char_is_whitespace(&'\n'));
        assert!(char_is_alphanumeric(&'a'));
        assert!(char_is_alphanumeric(&'1'));
        assert_eq!(char_to_code(&'a'), 97);
        assert_eq!(char_from_code(97), Some('a'));
        assert_eq!(char_from_code(-1), None);
        assert_eq!(char_compare(&'a', &'b'), -1);
        assert_eq!(char_compare(&'a', &'a'), 0);
        assert_eq!(char_compare(&'b', &'a'), 1);
        assert_eq!(char_to_string(&'a'), "a");
    }
}
