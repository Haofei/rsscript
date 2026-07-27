use rand::Rng;

pub fn uuid_new_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn random_int(min: i64, max: i64) -> i64 {
    let mut rng = rand::thread_rng();
    rng.gen_range(min..=max)
}

pub fn random_bool() -> bool {
    let mut rng = rand::thread_rng();
    rng.r#gen()
}

pub fn random_float() -> f64 {
    let mut rng = rand::thread_rng();
    rng.r#gen()
}

pub fn random_bytes(len: i64) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut bytes =
        vec![0u8; crate::resource_budget::bounded_allocation_size(len, "random byte allocation")];
    rng.fill(bytes.as_mut_slice());
    bytes
}

pub fn random_string(len: i64) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let len = crate::resource_budget::bounded_allocation_size(len, "random string allocation");
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_allocations_reject_sizes_above_runtime_ceiling() {
        let oversized = crate::RUNTIME_ALLOCATION_CEILING_BYTES as i64 + 1;
        assert!(std::panic::catch_unwind(|| random_bytes(oversized)).is_err());
        assert!(std::panic::catch_unwind(|| random_string(oversized)).is_err());
    }

    #[test]
    fn negative_random_lengths_remain_empty() {
        assert!(random_bytes(-1).is_empty());
        assert!(random_string(-1).is_empty());
    }
}
