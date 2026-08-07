pub fn arguments_count(args: &[String]) -> i64 {
    args.len() as i64
}

pub fn arguments_all(args: &[String]) -> Vec<String> {
    args.to_vec()
}

pub fn arguments_get(args: &[String], index: i64) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|index| args.get(index).cloned())
}

pub fn arguments_get_or_default(args: &[String], index: i64, default: &str) -> String {
    arguments_get(args, index).unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_arguments_do_not_read_process_state() {
        let args = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(arguments_count(&args), 2);
        assert_eq!(arguments_all(&args), args);
        assert_eq!(arguments_get(&args, 1).as_deref(), Some("beta"));
        assert_eq!(arguments_get(&args, -1), None);
        assert_eq!(arguments_get_or_default(&args, 9, "fallback"), "fallback");
    }
}
