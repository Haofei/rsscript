use rayon::prelude::*;

// RSScript's native List<Int> ABI passes Vec-backed values to adapter functions.
#[allow(clippy::ptr_arg)]
pub fn sum_int(values: &Vec<i64>) -> i64 {
    checked_sum(values.iter().copied())
}

#[allow(clippy::ptr_arg)]
pub fn sum_squares(values: &Vec<i64>) -> i64 {
    let squares = values
        .par_iter()
        .map(|value| value.checked_mul(*value))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_else(|| panic!("RSScript integer overflow"));
    checked_sum(squares)
}

fn checked_sum(values: impl IntoIterator<Item = i64>) -> i64 {
    values
        .into_iter()
        .try_fold(0_i64, i64::checked_add)
        .unwrap_or_else(|| panic!("RSScript integer overflow"))
}

#[allow(clippy::ptr_arg)]
pub fn count_positive_int(values: &Vec<i64>) -> i64 {
    values.par_iter().filter(|value| **value > 0).count() as i64
}

pub fn sort_int(values: &mut [i64]) {
    values.par_sort_unstable();
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    fn random_ints(size: usize, seed: u64) -> Vec<i64> {
        let mut state = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xD1B5_4A32_D192_ED03);
        let mut values = Vec::with_capacity(size);
        for _ in 0..size {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            values.push((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 1) as i64);
        }
        values
    }

    #[test]
    fn rayon_sort_int_beats_serial_sort_on_real_workload() {
        let input = random_ints(5_000_000, 20_260_530);
        let mut serial = input.clone();
        let mut parallel = input;

        let serial_start = Instant::now();
        serial.sort_unstable();
        let serial_elapsed = serial_start.elapsed();

        let parallel_start = Instant::now();
        super::sort_int(&mut parallel);
        let parallel_elapsed = parallel_start.elapsed();

        assert_eq!(parallel.first(), serial.first());
        assert_eq!(parallel.last(), serial.last());
        assert_eq!(parallel.len(), serial.len());
        assert!(
            parallel_elapsed.as_nanos() * 100 < serial_elapsed.as_nanos() * 95,
            "Rayon sort must beat serial sort by at least 5%: serial={serial_elapsed:?} rayon={parallel_elapsed:?}"
        );
    }

    #[test]
    fn integer_reductions_unwind_on_overflow_for_the_native_abi_boundary() {
        assert!(std::panic::catch_unwind(|| super::sum_int(&vec![i64::MAX, 1])).is_err());
        assert!(std::panic::catch_unwind(|| super::sum_squares(&vec![i64::MAX])).is_err());
    }
}
