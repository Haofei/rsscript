use rayon::prelude::*;

pub fn sum_squares(values: &Vec<i64>) -> i64 {
    values.par_iter().map(|value| value * value).sum()
}
