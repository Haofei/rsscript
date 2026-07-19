//! Pure value-helper free functions split out of `reg_vm/mod.rs`
//! (numeric arithmetic/compare, typed value extraction, crypto digests, date
//! math, sorted-collection ops, path helpers, JSON helpers). The VM core calls
//! these via `use value_ops::*`.

use super::*;

/// A folder closure recognized as a single numeric binary op over its two
/// parameters: `op` is the arithmetic operator and `lhs_is_state` says whether
/// the op's left operand is the accumulator (param 0) — i.e. `acc <op> x` — or
/// the element (`x <op> acc`). Used to fast-path `List.fold` without losing the
/// closure's exact operand order.
pub(super) struct NumericBinaryClosure {
    pub(super) op: BinaryOp,
    pub(super) lhs_is_state: bool,
}

/// Recognize `|state, x| state <op> x` (or `x <op> state`) closures with no
/// captures whose body is exactly one arithmetic instruction returning its
/// result. Returns `None` for anything else so the caller falls back to the
/// generic interpreter (the recognizer is intentionally conservative — a missed
/// match only forgoes a speedup, never changes results).
pub(super) fn recognize_numeric_binary_closure(
    unit: &RegUnit,
    closure: &VmClosure,
) -> Option<NumericBinaryClosure> {
    // The fast path supplies the two operands by value at the param registers;
    // captured values would not be supplied, so require a capture-free closure.
    if !closure.captures.is_empty() {
        return None;
    }
    let function = &unit.functions[closure.function];
    if function.params != 2 {
        return None;
    }
    // Param registers: captures occupy `0..captures.len()` (none here), then the
    // two params at registers 0 and 1.
    let state_reg = 0usize;
    let item_reg = 1usize;
    let [instr, RegInstr::Return { src }] = function.code.as_slice() else {
        return None;
    };
    let (op, dst, lhs, rhs) = arithmetic_binop_parts(instr)?;
    if dst != *src {
        return None;
    }
    // Both operands must be exactly the two distinct param registers (so every
    // input is a supplied param and nothing else is read).
    let lhs_is_state = if lhs == state_reg && rhs == item_reg {
        true
    } else if lhs == item_reg && rhs == state_reg {
        false
    } else {
        return None;
    };
    Some(NumericBinaryClosure { op, lhs_is_state })
}

/// If `instr` is one of the numeric arithmetic instructions handled by
/// [`eval_numeric_binary`], return `(op, dst, lhs, rhs)`. Bitwise/shift ops are
/// excluded: they are `Int`-only and not routed through `eval_numeric_binary`.
pub(super) fn arithmetic_binop_parts(instr: &RegInstr) -> Option<(BinaryOp, Reg, Reg, Reg)> {
    match instr {
        RegInstr::AddInt { dst, lhs, rhs } => Some((BinaryOp::Add, *dst, *lhs, *rhs)),
        RegInstr::SubInt { dst, lhs, rhs } => Some((BinaryOp::Subtract, *dst, *lhs, *rhs)),
        RegInstr::MulInt { dst, lhs, rhs } => Some((BinaryOp::Multiply, *dst, *lhs, *rhs)),
        RegInstr::DivInt { dst, lhs, rhs } => Some((BinaryOp::Divide, *dst, *lhs, *rhs)),
        RegInstr::ModInt { dst, lhs, rhs } => Some((BinaryOp::Modulo, *dst, *lhs, *rhs)),
        _ => None,
    }
}

pub(super) fn eval_numeric_binary(
    op: BinaryOp,
    lhs: &VmValue,
    rhs: &VmValue,
) -> Result<VmValue, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => match op {
            // Integer arithmetic traps (overflow, divide/modulo by zero) are
            // language-level runtime errors, never host panics, so the VM exits
            // through `EvalError` exactly like the Rust backend's checked ops.
            BinaryOp::Add => lhs
                .checked_add(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("addition", *lhs, *rhs)),
            BinaryOp::Subtract => lhs
                .checked_sub(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("subtraction", *lhs, *rhs)),
            BinaryOp::Multiply => lhs
                .checked_mul(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("multiplication", *lhs, *rhs)),
            BinaryOp::Divide => {
                if *rhs == 0 {
                    return Err(EvalError::Runtime("integer division by zero".to_string()));
                }
                lhs.checked_div(*rhs)
                    .map(VmValue::Int)
                    .ok_or_else(|| int_overflow_error("division", *lhs, *rhs))
            }
            BinaryOp::Modulo => {
                if *rhs == 0 {
                    return Err(EvalError::Runtime("integer modulo by zero".to_string()));
                }
                lhs.checked_rem(*rhs)
                    .map(VmValue::Int)
                    .ok_or_else(|| int_overflow_error("modulo", *lhs, *rhs))
            }
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        (VmValue::Float(lhs), VmValue::Float(rhs)) => match op {
            BinaryOp::Add => Ok(VmValue::Float(lhs + rhs)),
            BinaryOp::Subtract => Ok(VmValue::Float(lhs - rhs)),
            BinaryOp::Multiply => Ok(VmValue::Float(lhs * rhs)),
            BinaryOp::Divide => Ok(VmValue::Float(lhs / rhs)),
            BinaryOp::Modulo => Err(EvalError::Runtime(
                "reg VM modulo expects Int operands.".to_string(),
            )),
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric operator expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

pub(super) fn checked_shift_count(bits: i64) -> Result<u32, EvalError> {
    u32::try_from(bits)
        .ok()
        .filter(|bits| *bits < i64::BITS)
        .ok_or_else(|| {
            EvalError::Runtime(format!(
                "shift count must be between 0 and {}, got {bits}",
                i64::BITS - 1
            ))
        })
}

pub(super) fn eval_numeric_compare(
    op: RegIntCompare,
    lhs: &VmValue,
    rhs: &VmValue,
) -> Result<bool, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => Ok(eval_int_compare(op, *lhs, *rhs)),
        (VmValue::Float(lhs), VmValue::Float(rhs)) => Ok(match op {
            RegIntCompare::Less => lhs < rhs,
            RegIntCompare::LessEqual => lhs <= rhs,
            RegIntCompare::Greater => lhs > rhs,
            RegIntCompare::GreaterEqual => lhs >= rhs,
        }),
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric comparison expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

pub(super) fn expect_int_ref(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Int(value) => Ok(*value),
        // `Managed` is transparent (see vm_value: display/native_value/equality
        // all unwrap it). A value retained into storage via `manage` and read
        // back arrives wrapped; see through it like the rest of the value model.
        VmValue::Managed(inner) => expect_int_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Int, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_float_ref(value: &VmValue) -> Result<f64, EvalError> {
    match value {
        VmValue::Float(value) => Ok(*value),
        VmValue::Managed(inner) => expect_float_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Float, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_char_ref(value: &VmValue) -> Result<char, EvalError> {
    match value {
        VmValue::Char(value) => Ok(*value),
        VmValue::Managed(inner) => expect_char_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Char, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_bytes_ref(value: &VmValue) -> Result<&[u8], EvalError> {
    match value {
        VmValue::Bytes(value) => Ok(value.as_slice()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bytes, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_usize_ref(value: &VmValue) -> Result<usize, EvalError> {
    let value = expect_int_ref(value)?;
    usize::try_from(value).map_err(|_| {
        EvalError::Runtime(format!(
            "reg VM expected non-negative index, got `{value}`."
        ))
    })
}

pub(super) fn sha256_digest(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    Digest::update(&mut hasher, value);
    format!("{:x}", hasher.finalize())
}

pub(super) fn sha3_224_digest(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_224::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

pub(super) fn sha3_256_digest(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_256::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

pub(super) fn shake128_digest(value: &[u8], out_len: usize) -> Vec<u8> {
    let mut hasher = Shake128::default();
    Update::update(&mut hasher, value);
    let mut reader = hasher.finalize_xof();
    let mut out = vec![0u8; out_len];
    XofReader::read(&mut reader, &mut out);
    out
}

pub(super) fn hmac_sha256_digest(key: &[u8], value: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, value);
    format!("{:x}", mac.finalize().into_bytes())
}

pub(super) fn utc_datetime(unix_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(unix_ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("epoch is valid")
        })
}

pub(super) fn date_parse_ymd(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let datetime = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&datetime).timestamp_millis())
}

pub(super) fn date_parse_iso(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc).timestamp_millis())
}

pub(super) fn date_is_leap_year(year: i64) -> bool {
    let Ok(year) = i32::try_from(year) else {
        return false;
    };
    NaiveDate::from_ymd_opt(year, 2, 29).is_some()
}

pub(super) fn date_days_in_month(year: i64, month: i64) -> i64 {
    let Ok(year) = i32::try_from(year) else {
        return 0;
    };
    let Ok(month) = u32::try_from(month) else {
        return 0;
    };
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return 0;
    };
    let Some(next_month) = (if month == 12 {
        year.checked_add(1)
            .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
    } else {
        month
            .checked_add(1)
            .and_then(|month| NaiveDate::from_ymd_opt(year, month, 1))
    }) else {
        return 0;
    };
    (next_month - first).num_days()
}

/// Insert `value` into an already-sorted `Vec` via binary search, keeping it
/// sorted (`Ok(false)` if an equal element is present). O(log n) search + O(n)
/// shift — no clone and no full re-sort, unlike rebuilding the whole backing.
pub(super) fn sorted_insert_vm(
    items: &mut Vec<VmValue>,
    value: VmValue,
) -> Result<bool, EvalError> {
    vm_value_cmp(&value, &value)?; // reject non-orderable values (parity with re-sort)
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match vm_value_cmp(&items[mid], &value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(false),
        }
    }
    items.insert(lo, value);
    Ok(true)
}

/// Remove `value` from an already-sorted `Vec` via binary search.
pub(super) fn sorted_remove_vm(
    items: &mut Vec<VmValue>,
    value: &VmValue,
) -> Result<bool, EvalError> {
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match vm_value_cmp(&items[mid], value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                items.remove(mid);
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Binary-search a sorted `Vec` for `value` (O(log n)) — used by `SortedSet`'s
/// membership test in place of a linear scan.
pub(super) fn sorted_contains_vm(items: &TypedVec, value: &VmValue) -> Result<bool, EvalError> {
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let item = items.get(mid).expect("binary-search index in bounds");
        match vm_value_cmp(&item, value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(true),
        }
    }
    Ok(false)
}

/// Binary-search a sorted-map backing by key and return the value if present —
/// O(log n), cloning only the matched value (not the whole backing).
pub(super) fn sorted_map_get_in_place(
    backing: &TypedVec,
    key: &VmValue,
) -> Result<Option<VmValue>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let entry = backing.get(mid).expect("binary-search index in bounds");
        let pair = expect_list_ref(&entry)?;
        let pair = pair.borrow();
        let entry_key = pair
            .first()
            .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
        match vm_value_cmp(&entry_key, key)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(pair.get(1)),
        }
    }
    Ok(None)
}

/// Binary-search a sorted-map backing (a `List` of `[key, value]` pair lists) by
/// key and insert/update in place — no clone, no full re-sort, no rebuild.
pub(super) fn sorted_map_insert_in_place(
    backing: &mut Vec<VmValue>,
    key: VmValue,
    value: VmValue,
) -> Result<(), EvalError> {
    vm_value_cmp(&key, &key)?;
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ordering = {
            let pair = expect_list_ref(&backing[mid])?;
            let pair = pair.borrow();
            let entry_key = pair.first().ok_or_else(|| {
                EvalError::Runtime("reg VM SortedMap entry missing key.".to_string())
            })?;
            vm_value_cmp(&entry_key, &key)?
        };
        match ordering {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                let pair = expect_list_ref(&backing[mid])?;
                let mut pair = pair.borrow_mut();
                if pair.len() > 1 {
                    // Total mutation helper: a pair-list update promotes on a
                    // kind mismatch rather than erroring (construction-path rule).
                    pair.set(1, value);
                } else {
                    pair.push(value);
                }
                return Ok(());
            }
        }
    }
    backing.insert(
        lo,
        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
            key, value,
        ])))),
    );
    Ok(())
}

/// Binary-search a sorted-map backing by key and remove the entry, returning its
/// value if present.
pub(super) fn sorted_map_remove_in_place(
    backing: &mut Vec<VmValue>,
    key: &VmValue,
) -> Result<Option<VmValue>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ordering = {
            let pair = expect_list_ref(&backing[mid])?;
            let pair = pair.borrow();
            let entry_key = pair.first().ok_or_else(|| {
                EvalError::Runtime("reg VM SortedMap entry missing key.".to_string())
            })?;
            vm_value_cmp(&entry_key, key)?
        };
        match ordering {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                let removed = backing.remove(mid);
                let pair = expect_list_ref(&removed)?;
                let value = pair.borrow().get(1);
                return Ok(value);
            }
        }
    }
    Ok(None)
}

pub(super) fn sort_vm_values(items: &mut [VmValue]) -> Result<(), EvalError> {
    for item in items.iter() {
        vm_value_cmp(item, item)?;
    }
    let mut sorted = items.to_vec();
    sorted.sort_by(|left, right| vm_value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        vm_value_cmp(&pair[0], &pair[1])?;
    }
    items.clone_from_slice(&sorted);
    Ok(())
}

pub(super) fn vm_value_cmp(left: &VmValue, right: &VmValue) -> Result<Ordering, EvalError> {
    match (left, right) {
        (VmValue::Unit, VmValue::Unit) => Ok(Ordering::Equal),
        (VmValue::Int(left), VmValue::Int(right)) => Ok(left.cmp(right)),
        (VmValue::Bool(left), VmValue::Bool(right)) => Ok(left.cmp(right)),
        (VmValue::String(left), VmValue::String(right)) => Ok(left.cmp(right)),
        (VmValue::Char(left), VmValue::Char(right)) => Ok(left.cmp(right)),
        _ => Err(EvalError::Runtime(format!(
            "SortedSet value is not orderable: `{}`.",
            left.display()
        ))),
    }
}

pub(super) fn sorted_map_value(entries: Vec<(VmValue, VmValue)>) -> VmValue {
    VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
        sorted_map_entry_values(entries),
    ))))
}

fn sorted_map_entry_values(entries: Vec<(VmValue, VmValue)>) -> Vec<VmValue> {
    entries
        .into_iter()
        .map(|(key, value)| {
            VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
                key, value,
            ]))))
        })
        .collect()
}

pub(super) fn sorted_map_get(entries: &[(VmValue, VmValue)], key: &VmValue) -> Option<VmValue> {
    entries
        .iter()
        .find_map(|(entry_key, value)| (entry_key == key).then(|| value.clone()))
}

pub(super) fn sorted_map_insert(
    entries: &mut Vec<(VmValue, VmValue)>,
    key: VmValue,
    value: VmValue,
) {
    if let Some((_, existing)) = entries.iter_mut().find(|(entry_key, _)| entry_key == &key) {
        *existing = value;
        return;
    }
    entries.push((key, value));
}

pub(super) fn sorted_map_remove(
    entries: &mut Vec<(VmValue, VmValue)>,
    key: &VmValue,
) -> Option<VmValue> {
    let index = entries.iter().position(|(entry_key, _)| entry_key == key)?;
    Some(entries.remove(index).1)
}

pub(super) fn path_join_string(base: &str, child: &str) -> String {
    Path::new(base).join(child).to_string_lossy().to_string()
}

pub(super) fn path_normalize_string(path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized.to_string_lossy().to_string()
}

pub(super) fn path_safe_relative_string(value: &str) -> Result<String, String> {
    let path = Path::new(value);
    if value.is_empty() {
        return Err("path must be non-empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err("parent-directory traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path must name a relative file or directory".to_string());
    }
    Ok(normalized.to_string_lossy().to_string())
}

pub(super) fn path_resolve_relative_string(root: &str, relative: &str) -> Result<String, String> {
    let relative = path_safe_relative_string(relative)?;
    let resolved = path_normalize_string(&path_join_string(root, &relative));
    if !Path::new(&resolved).starts_with(Path::new(root)) {
        return Err("resolved path escapes the workspace root".to_string());
    }
    Ok(resolved)
}

pub(super) fn expect_string_list_ref(value: &VmValue) -> Result<Vec<String>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow()
        .iter()
        .map(|value| expect_string_ref(&value).map(str::to_string))
        .collect()
}

pub(super) fn expect_float_list_ref(value: &VmValue) -> Result<Vec<f64>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow().iter().map(|v| expect_float_ref(&v)).collect()
}

pub(super) fn expect_int_list_ref(value: &VmValue) -> Result<Vec<i64>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow().iter().map(|v| expect_int_ref(&v)).collect()
}

pub(super) fn expect_bool_ref(value: &VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Bool(value) => Ok(*value),
        VmValue::Managed(inner) => expect_bool_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_list_ref(value: &VmValue) -> Result<Rc<RefCell<TypedVec>>, EvalError> {
    match value {
        VmValue::List(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_list_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected List, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_deque_ref(
    value: &VmValue,
) -> Result<Rc<RefCell<std::collections::VecDeque<VmValue>>>, EvalError> {
    match value {
        VmValue::Deque(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_deque_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Deque, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_map_ref(value: &VmValue) -> Result<Rc<RefCell<ValueMap>>, EvalError> {
    match value {
        VmValue::Map(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_map_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Map, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_json_ref(value: &VmValue) -> Result<&serde_json::Value, EvalError> {
    match value {
        VmValue::Json(value) => Ok(value.as_ref()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected JsonValue, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn map_key_from_value(value: &VmValue) -> Result<(VmMapKey, usize), EvalError> {
    // The checker guarantees a key's static type is `Hashable`, so this is a
    // defensive gate: it accepts every shape RSScript admits as a key — scalars,
    // strings/bytes, the structural `List`/`Deque`/`Option` containers, and
    // `derives(Eq, Hash)` structs/sums (recursively) — and rejects only the
    // genuinely unhashable values (`Float`, `Map`, raw `Json`, closures) with a
    // clean runtime error rather than a host panic.
    fn is_hashable(
        value: &VmValue,
        active: &mut std::collections::HashSet<usize>,
        depth: usize,
        work: &mut usize,
    ) -> bool {
        *work = work.saturating_add(match value {
            VmValue::String(value) => 1 + value.len() / 64,
            VmValue::Bytes(value) => 1 + value.len() / 64,
            _ => 1,
        });
        if depth > 256 || *work > 1_000_000 {
            return false;
        }
        let node = crate::vm_value::vm_value_node_id(value);
        if let Some(node) = node {
            if !active.insert(node) {
                return false;
            }
        }
        let hashable = match value {
            VmValue::Unit
            | VmValue::Bool(_)
            | VmValue::Int(_)
            | VmValue::Char(_)
            | VmValue::String(_)
            | VmValue::Bytes(_)
            | VmValue::Native(_)
            | VmValue::OptionNone => true,
            VmValue::OptionSomeHeap(inner) => is_hashable(inner, active, depth + 1, work),
            // The inline payload is a scalar; recurse on the materialized value so
            // the hashability rule is identical to the heap arm (e.g. a
            // `Some(Float)` is inline yet still unhashable).
            VmValue::OptionSomeScalar(scalar) => {
                is_hashable(&scalar.to_value(), active, depth + 1, work)
            }
            VmValue::List(items) => items
                .borrow()
                .iter()
                .all(|value| is_hashable(&value, active, depth + 1, work)),
            VmValue::Deque(items) => items
                .borrow()
                .iter()
                .all(|value| is_hashable(value, active, depth + 1, work)),
            VmValue::Struct(data) | VmValue::Variant(data) => data
                .fields
                .iter()
                .all(|value| is_hashable(value, active, depth + 1, work)),
            VmValue::Managed(inner) => is_hashable(&inner.borrow(), active, depth + 1, work),
            VmValue::Float(_) | VmValue::Map(_) | VmValue::Json(_) | VmValue::Closure(_) => false,
        };
        if let Some(node) = node {
            active.remove(&node);
        }
        hashable
    }

    let mut work = 0;
    if is_hashable(value, &mut std::collections::HashSet::new(), 0, &mut work) {
        Ok((VmMapKey::new(value.clone()), work))
    } else {
        Err(EvalError::Runtime(format!(
            "reg VM Map key does not support `{}`.",
            value.display()
        )))
    }
}

pub(super) fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub(super) fn json_quote_string(value: &str) -> Result<String, EvalError> {
    serde_json::to_string(value).map_err(|error| EvalError::Runtime(error.to_string()))
}

pub(super) fn json_array_get_value(
    value: &serde_json::Value,
    index: i64,
) -> Result<VmValue, VmValue> {
    if index < 0 {
        return Err(json_error_value(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    items
        .get(index as usize)
        .cloned()
        .map(|value| VmValue::Json(Rc::new(value)))
        .ok_or_else(|| json_error_value(format!("JSON array index `{index}` is out of bounds")))
}

pub(super) fn json_array_items(
    value: &serde_json::Value,
) -> Result<&Vec<serde_json::Value>, VmValue> {
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    Ok(items)
}

pub(super) fn json_array_bools_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(VmValue::Bool(flag));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
        flags,
    )))))
}

pub(super) fn json_array_ints_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(VmValue::Int(number));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
        numbers,
    )))))
}

pub(super) fn json_array_strings_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(VmValue::string(text));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
        strings,
    )))))
}

#[derive(Debug, Clone, Copy)]
pub(super) enum JsonArrayStringMatch {
    Exact,
    Substring,
    Prefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JsonPathPart {
    Field(String),
    Index(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_map_key_reports_proportional_work() {
        let scalar_work = map_key_from_value(&VmValue::Int(1)).expect("integer key").1;
        let values = (0..128).map(VmValue::Int).collect::<Vec<_>>();
        let list = VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(values))));
        let list_work = map_key_from_value(&list).expect("list key").1;

        assert_eq!(scalar_work, 1);
        assert_eq!(list_work, 129);
    }
}
