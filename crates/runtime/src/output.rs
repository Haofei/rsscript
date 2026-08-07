use crate::{JsonValue, json_to_string};

/// Writes language-visible standard output for generated Rust programs.
pub fn log_write(message: &str) {
    println!("{message}");
}

/// Writes a JSON value to language-visible standard output.
pub fn log_write_json(value: &JsonValue) {
    println!("{}", json_to_string(value));
}

/// Writes language-visible standard error for generated Rust programs.
pub fn log_error(message: &str) {
    eprintln!("{message}");
}

/// Writes a JSON value to language-visible standard error.
pub fn log_error_json(value: &JsonValue) {
    eprintln!("{}", json_to_string(value));
}

/// Writes an explicit trace record. This is output semantics, not host logging.
pub fn log_trace(event: &str, message: &str) {
    println!("trace {event}: {message}");
}
