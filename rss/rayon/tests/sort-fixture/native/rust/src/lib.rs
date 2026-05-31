use std::sync::OnceLock;
use std::time::Instant;

pub fn random_ints(size: i64, seed: i64) -> Vec<i64> {
    let len = size.max(0) as usize;
    let mut state = (seed as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        values.push((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 1) as i64);
    }
    values
}

pub fn sort_int_serial(values: &mut Vec<i64>) {
    values.sort_unstable();
}

pub fn now_ns() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as i64
}
