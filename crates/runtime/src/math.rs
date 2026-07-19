pub fn math_abs(value: i64) -> i64 {
    value.checked_abs().unwrap_or_else(|| {
        crate::error::panic_runtime_error(crate::error::integer_overflow_error(format!(
            "Math.abs overflow: abs({value}) exceeds the Int range"
        )))
    })
}

pub fn math_min(left: i64, right: i64) -> i64 {
    left.min(right)
}

pub fn math_max(left: i64, right: i64) -> i64 {
    left.max(right)
}

pub fn math_clamp(value: i64, min: i64, max: i64) -> i64 {
    if min > max {
        crate::error::panic_runtime_error(crate::error::invalid_argument_error(format!(
            "Math.clamp requires min <= max, got min {min} and max {max}"
        )));
    }
    value.clamp(min, max)
}

pub fn math_pow(base: i64, exponent: i64) -> i64 {
    let exponent = u32::try_from(exponent).unwrap_or_else(|_| {
        crate::error::panic_runtime_error(crate::error::invalid_argument_error(format!(
            "Math.pow exponent must be between 0 and {}, got {exponent}",
            u32::MAX
        )))
    });
    base.checked_pow(exponent).unwrap_or_else(|| {
        crate::error::panic_runtime_error(crate::error::integer_overflow_error(format!(
            "Math.pow overflow: {base} raised to {exponent} exceeds the Int range"
        )))
    })
}

// Explicit modular/clamping integer arithmetic. The `+`/`-`/`*` operators trap on
// overflow in every build profile (§6.8); these are the deliberate escape hatches
// for code that *wants* two's-complement wraparound or saturation at the Int
// bounds. They never trap.
pub fn math_wrapping_add(left: i64, right: i64) -> i64 {
    left.wrapping_add(right)
}

pub fn math_wrapping_sub(left: i64, right: i64) -> i64 {
    left.wrapping_sub(right)
}

pub fn math_wrapping_mul(left: i64, right: i64) -> i64 {
    left.wrapping_mul(right)
}

pub fn math_saturating_add(left: i64, right: i64) -> i64 {
    left.saturating_add(right)
}

pub fn math_saturating_sub(left: i64, right: i64) -> i64 {
    left.saturating_sub(right)
}

pub fn math_saturating_mul(left: i64, right: i64) -> i64 {
    left.saturating_mul(right)
}

pub fn math_abs_float(value: f64) -> f64 {
    value.abs()
}

pub fn math_min_float(left: f64, right: f64) -> f64 {
    left.min(right)
}

pub fn math_max_float(left: f64, right: f64) -> f64 {
    left.max(right)
}

pub fn math_clamp_float(value: f64, min: f64, max: f64) -> f64 {
    if min.is_nan() || max.is_nan() || min > max {
        crate::error::panic_runtime_error(crate::error::invalid_argument_error(format!(
            "Math.clamp_float requires non-NaN bounds with min <= max, got min {min} and max {max}"
        )));
    }
    value.clamp(min, max)
}

pub fn math_pow_float(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

pub fn math_exp2(value: f64) -> f64 {
    value.exp2()
}

pub fn math_log2(value: f64) -> f64 {
    value.log2()
}

pub fn math_sin(value: f64) -> f64 {
    value.sin()
}

pub fn math_cos(value: f64) -> f64 {
    value.cos()
}

pub fn math_exp(value: f64) -> f64 {
    value.exp()
}

pub fn math_log(value: f64) -> f64 {
    value.ln()
}

pub fn math_tanh(value: f64) -> f64 {
    value.tanh()
}

pub fn math_sqrt(value: f64) -> f64 {
    value.sqrt()
}

pub fn math_trunc_float(value: f64) -> f64 {
    value.trunc()
}

pub fn math_floor(value: f64) -> i64 {
    value.floor() as i64
}

pub fn math_ceil(value: f64) -> i64 {
    value.ceil() as i64
}

pub fn math_round(value: f64) -> i64 {
    value.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_math_rejects_invalid_domains_without_raw_std_panics() {
        for call in [
            || math_abs(i64::MIN),
            || math_clamp(1, 2, 1),
            || math_pow(2, -1),
            || math_pow(2, i64::from(u32::MAX) + 1),
            || math_pow(i64::MAX, 2),
        ] {
            let panic = std::panic::catch_unwind(call).expect_err("call must trap");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(message.starts_with(crate::diagnostics::RUNTIME_DIAGNOSTIC_PREFIX));
        }
    }

    #[test]
    fn clamp_float_rejects_nan_bounds_but_preserves_nan_values() {
        for call in [
            || math_clamp_float(0.0, f64::NAN, 1.0),
            || math_clamp_float(0.0, 0.0, f64::NAN),
            || math_clamp_float(0.0, f64::NAN, f64::NAN),
        ] {
            let panic = std::panic::catch_unwind(call).expect_err("NaN bound must trap");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(message.starts_with(crate::diagnostics::RUNTIME_DIAGNOSTIC_PREFIX));
        }
        assert!(math_clamp_float(f64::NAN, 0.0, 1.0).is_nan());
    }
}
