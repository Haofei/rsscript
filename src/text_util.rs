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

/// Split the inside of a generic argument list on top-level commas (commas nested
/// inside `<...>` are not separators), trimming each argument.
pub(crate) fn split_type_args(args: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(args[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = args[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }
    result
}
