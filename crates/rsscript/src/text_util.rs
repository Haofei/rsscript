//! Pure string / type-name text utilities shared across the VM and the Rust
//! lowering. These are total functions over `&str`/`String` with no dependency on
//! VM or HIR types, factored out of `reg_vm.rs` so the monolith carries less
//! incidental text-munging and `decode_string_token` has a single definition.

/// Decode a source string token's escape sequences (`\n`, `\t`, `\\`, `\"`, …).
/// Unknown escapes are preserved verbatim (backslash + char).
pub(crate) fn decode_string_token(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

/// Decode a character-literal token's raw inner text (escapes applied) into the
/// full sequence of scalars it denotes. A well-formed literal decodes to exactly
/// one scalar; `''` decodes to zero and `'ab'` to two. Handles the same escapes
/// as [`decode_string_token`] plus `\'`; unknown escapes preserve the backslash
/// + char (so they decode to >1 scalar and are rejected upstream).
fn decode_char_escapes(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('\'') => decoded.push('\''),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

/// The number of Unicode scalars a char-literal's raw inner text decodes to.
/// A well-formed `'c'` yields exactly `1`; the frontend rejects any other count
/// (see `check_char_literal_scalar`) before lowering runs.
pub(crate) fn char_literal_scalar_count(value: &str) -> usize {
    decode_char_escapes(value).chars().count()
}

/// Decode a character-literal token's raw inner text into its single scalar.
/// The frontend guarantees exactly one scalar (see `char_literal_scalar_count`),
/// so this is the single normalization point used by lowering. It is TOTAL: a
/// malformed literal that somehow reaches here yields the first scalar (or the
/// Unicode replacement character for the empty case) rather than panicking, so a
/// missed frontend check can never crash the compiler in release builds.
pub(crate) fn decode_char_token(value: &str) -> char {
    let decoded = decode_char_escapes(value);
    let mut iter = decoded.chars();
    let first = iter.next().unwrap_or('\u{FFFD}');
    debug_assert!(
        iter.next().is_none(),
        "char literal must contain exactly one scalar: {value:?}"
    );
    first
}

/// `value[start .. start+len]` clamped to char boundaries and the string bounds.
pub(crate) fn string_slice_range(value: &str, start: i64, len: i64) -> &str {
    let byte_start = clamp_to_char_boundary(value, start.max(0) as usize);
    let requested_end = byte_start.saturating_add(len.max(0) as usize);
    let byte_end = clamp_to_char_boundary(value, requested_end.min(value.len()));
    &value[byte_start..byte_end]
}

/// Pad `value` to `width` bytes with repetitions of `fill`, on the left or right.
pub(crate) fn string_pad(value: &str, width: i64, fill: &str, left: bool) -> String {
    let target = width.max(0) as usize;
    if value.len() >= target || fill.is_empty() {
        return value.to_string();
    }
    let missing = target - value.len();
    let mut padding = String::new();
    while padding.len() < missing {
        padding.push_str(fill);
    }
    while padding.len() > missing {
        padding.pop();
    }
    if left {
        format!("{padding}{value}")
    } else {
        format!("{value}{padding}")
    }
}

/// Byte length of [`string_pad`] without materializing the padded string.
pub(crate) fn string_pad_len(value: &str, width: i64, fill: &str) -> Option<i64> {
    let target = width.max(0) as usize;
    if value.len() >= target || fill.is_empty() {
        return i64::try_from(value.len()).ok();
    }

    let missing = target - value.len();
    let mut padding_len = 0usize;
    while padding_len < missing {
        padding_len = padding_len.checked_add(fill.len())?;
    }

    let mut rev_char_lens = fill.chars().rev().map(char::len_utf8).cycle();
    while padding_len > missing {
        padding_len = padding_len.checked_sub(rev_char_lens.next()?)?;
    }

    i64::try_from(value.len().checked_add(padding_len)?).ok()
}

/// Substitute `{}` placeholders in `template` with successive `args` (`{{`/`}}`
/// are literal braces; a `{}` with no remaining argument is left as-is).
pub(crate) fn string_format(template: &str, args: &[String]) -> String {
    let mut output = String::new();
    let mut chars = template.chars().peekable();
    let mut arg_index = 0;
    while let Some(ch) = chars.next() {
        match (ch, chars.peek().copied()) {
            ('{', Some('{')) => {
                chars.next();
                output.push('{');
            }
            ('}', Some('}')) => {
                chars.next();
                output.push('}');
            }
            ('{', Some('}')) => {
                chars.next();
                if let Some(value) = args.get(arg_index) {
                    output.push_str(value);
                    arg_index += 1;
                } else {
                    output.push_str("{}");
                }
            }
            _ => output.push(ch),
        }
    }
    output
}

/// Round `index` down to the nearest char boundary within `value`.
pub(crate) fn clamp_to_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Drop a leading `fresh ` freshness marker from a (trimmed) type name. The
/// single canonical definition; e.g. `fresh List<Int>` → `List<Int>`, `Foo` → `Foo`.
pub(crate) fn strip_fresh_type(type_name: &str) -> &str {
    let trimmed = type_name.trim();
    trimmed.strip_prefix("fresh ").unwrap_or(trimmed)
}

/// The root (non-generic) name of a type: trims, drops a leading `fresh ` marker,
/// and takes everything before the first `<`. Examples: `List<Int>` → `List`,
/// `fresh Map<K, V>` → `Map`, `  Foo  ` → `Foo`.
pub(crate) fn type_root_name(name: &str) -> &str {
    let trimmed = name.trim();
    let base = trimmed.strip_prefix("fresh ").unwrap_or(trimmed);
    base.split_once('<').map_or(base, |(root, _)| root)
}

/// Substitute generic parameter names for concrete argument names in a type
/// string. `substitute_type_args("A", {A: Int})` → `"Int"`;
/// `substitute_type_args("List<A>", {A: Int})` → `"List<Int>"`. Nested arguments
/// are rewritten recursively; unknown names pass through unchanged.
pub(crate) fn substitute_type_args(
    type_name: &str,
    substitutions: &std::collections::HashMap<String, String>,
) -> String {
    let trimmed = type_name.trim();
    if let Some(replacement) = substitutions.get(trimmed) {
        return replacement.clone();
    }
    let Some(args) = type_arg_names(trimmed) else {
        return trimmed.to_string();
    };
    let root = type_root_name(trimmed);
    let args = args
        .into_iter()
        .map(|arg| substitute_type_args(arg, substitutions))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{root}<{args}>")
}

/// The top-level generic arguments of a type, or `None` if it isn't generic.
/// `Map<K, Result<A, B>>` → `["K", "Result<A, B>"]`.
pub(crate) fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let (_, rest) = type_name.split_once('<')?;
    let inner = rest.strip_suffix('>')?;
    Some(split_top_level_type_args(inner))
}

/// Split a generic argument list on top-level commas (commas nested inside
/// `<...>` or `(...)` are not separators), trimming each argument. The single
/// canonical splitter shared by [`type_arg_names`] and the VM. Paren-awareness
/// is required so first-class `Fn(a, b) -> r` arguments (e.g. the element of a
/// `List<owned Fn(read UOp, mut Ctx) -> Option<UOp>>`) split on the right comma.
pub(crate) fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if start < args.len() {
        parts.push(args[start..].trim());
    }
    parts
}

/// Declared generic parameter names for a builtin generic type root, e.g.
/// `Map<K, V>` -> `["K", "V"]`, `Result<T, E>` -> `["T", "E"]`, `List<T>` ->
/// `["T"]`. `None` for non-generic / unknown roots. Single source of truth: the
/// HIR substitution path, the call-site substitution path, and the Rust lowerer
/// all use this, so the names can't drift apart per layer (they did once: a
/// `Result -> ["K","V"]` copy weakened error-combinator substitution).
pub(crate) fn builtin_generic_type_params(root: &str) -> Option<Vec<&'static str>> {
    match root {
        "List" | "Set" | "Option" | "ResourcePool" | "Channel" | "Sender" | "Receiver"
        | "Stream" | "Pipeline" => Some(vec!["T"]),
        "FalliblePipeline" => Some(vec!["T", "E"]),
        "Capability" => Some(vec!["P"]),
        "Map" => Some(vec!["K", "V"]),
        "Result" => Some(vec!["T", "E"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_pad_len_matches_materialized_padding() {
        let cases = [
            ("a", 2, "é"),
            ("a", 2, "aé"),
            ("a", 3, "éa"),
            ("abc", 2, "0"),
            ("abc", 8, ""),
            ("abc", 8, "01"),
            ("é", 5, "🙂x"),
        ];
        for (value, width, fill) in cases {
            assert_eq!(
                string_pad_len(value, width, fill),
                i64::try_from(string_pad(value, width, fill, true).len()).ok(),
                "value={value:?} width={width} fill={fill:?}",
            );
        }
    }
}
