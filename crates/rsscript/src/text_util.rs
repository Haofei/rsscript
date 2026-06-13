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

/// The top-level generic arguments of a type, or `None` if it isn't generic.
/// `Map<K, Result<A, B>>` → `["K", "Result<A, B>"]`.
pub(crate) fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let (_, rest) = type_name.split_once('<')?;
    let inner = rest.strip_suffix('>')?;
    Some(split_top_level_type_args(inner))
}

/// Split a generic argument list on top-level commas (commas nested inside
/// `<...>` are not separators), trimming each argument. The single canonical
/// splitter shared by [`type_arg_names`] and the VM.
pub(crate) fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
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
