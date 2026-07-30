// Generated native bindings pass mutable RSScript lists as Vec references.
#[allow(clippy::ptr_arg)]
pub fn sort_int(values: &mut Vec<i64>) {
    values.sort_unstable();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_in_place_deterministically() {
        let mut values = vec![3, -1, 3, 0];
        sort_int(&mut values);
        assert_eq!(values, vec![-1, 0, 3, 3]);
    }
}
