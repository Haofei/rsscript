use std::path::PathBuf;

pub fn env_get(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub fn env_get_or_default(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

pub fn env_set(_name: &str, _value: &str) {
    // env::set_var is unsafe in Rust 2024 edition (process-global mutation).
    // RSS marks this as effects(native) in the interface to signal review risk.
    // Runtime implementation intentionally no-ops; real set_var requires platform glue.
    eprintln!("[rsscript] warning: Env.set is a no-op in the safe runtime");
}

pub fn env_current_dir() -> std::io::Result<PathBuf> {
    std::env::current_dir()
}

pub fn env_set_current_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::env::set_current_dir(path)
}

pub fn env_home_dir() -> Option<PathBuf> {
    dirs_fallback_home()
}

pub fn env_temp_dir() -> PathBuf {
    std::env::temp_dir()
}

fn dirs_fallback_home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}
