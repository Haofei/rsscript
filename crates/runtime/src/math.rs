pub fn math_abs(value: i64) -> i64 {
    value.abs()
}

pub fn math_min(left: i64, right: i64) -> i64 {
    left.min(right)
}

pub fn math_max(left: i64, right: i64) -> i64 {
    left.max(right)
}

pub fn math_clamp(value: i64, min: i64, max: i64) -> i64 {
    value.clamp(min, max)
}

pub fn math_pow(base: i64, exponent: i64) -> i64 {
    base.checked_pow(exponent.max(0) as u32).unwrap_or_else(|| {
        panic!("Math.pow overflow: {base} raised to {exponent} exceeds the Int range")
    })
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
