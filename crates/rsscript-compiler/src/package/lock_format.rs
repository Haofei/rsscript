use super::PackageLock;

/// Canonical package-lock serialization used by the package capture boundary.
///
/// This is deliberately not a review presentation API: writing a lock is part
/// of snapshot persistence and remains behind the explicit compatibility path.
pub(crate) fn package_lock_toml(lock: &PackageLock) -> String {
    toml::to_string_pretty(lock).expect("package lock TOML serialization should not fail")
}
