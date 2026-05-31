use crate::error::{assertion_failed_error, panic_runtime_error};

pub fn assert_equal(left: &str, right: &str) {
    if left != right {
        panic_runtime_error(assertion_failed_error(format!(
            "assertion failed: left `{left}` did not equal right `{right}`"
        )));
    }
}

pub fn assert_equal_int(left: i64, right: i64) {
    if left != right {
        panic_runtime_error(assertion_failed_error(format!(
            "assertion failed: left `{left}` did not equal right `{right}`"
        )));
    }
}

pub fn assert_equal_bool(left: bool, right: bool) {
    if left != right {
        panic_runtime_error(assertion_failed_error(format!(
            "assertion failed: left `{left}` did not equal right `{right}`"
        )));
    }
}
