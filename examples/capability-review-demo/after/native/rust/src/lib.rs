//! Stand-in native crate for the capability-review demo. The bodies are not run
//! by the demo (which only exercises `rss pkg review/diff/lock`); they exist so
//! the native bindings resolve to a real crate path.

pub fn query(_sql: &str) -> String {
    String::new()
}

pub fn fetch(_url: &str) -> String {
    String::new()
}
