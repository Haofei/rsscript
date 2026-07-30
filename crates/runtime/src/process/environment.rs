#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEnv {
    pub name: String,
    pub value: String,
}

pub(super) fn configure_process_environment(
    command: &mut std::process::Command,
    requested: &[ProcessEnv],
) {
    const INHERITED_ENV_ALLOWLIST: &[&str] = &[
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "TMPDIR",
        "TMP",
        "TEMP",
    ];

    command.env_clear();
    for name in INHERITED_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for env in requested {
        command.env(&env.name, &env.value);
    }
}

pub(super) fn apply_default_ramdisk_env(command: &mut std::process::Command) {
    if ramdisk_auto_env_enabled()
        && std::env::var_os("RSSCRIPT_RAMDISK_PATH").is_none()
        && let Some(path) = default_ramdisk_root_dir()
    {
        command.env("RSSCRIPT_RAMDISK_PATH", path);
    }
}

fn ramdisk_auto_env_enabled() -> bool {
    matches!(
        std::env::var("RSSCRIPT_ENABLE_RAMDISK").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[cfg(target_os = "macos")]
fn default_ramdisk_root_dir() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from("/Volumes/RSScriptRAMDisk");
    if path.is_dir() {
        return Some(path);
    }

    let gib = std::env::var("RSSCRIPT_RAMDISK_GIB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8);
    let sectors = gib
        .saturating_mul(1024)
        .saturating_mul(1024)
        .saturating_mul(1024)
        / 512;
    let attach = std::process::Command::new("hdiutil")
        .arg("attach")
        .arg("-nomount")
        .arg(format!("ram://{sectors}"))
        .output()
        .ok()?;
    if !attach.status.success() {
        return None;
    }
    let device = String::from_utf8_lossy(&attach.stdout).trim().to_string();
    if device.is_empty() {
        return None;
    }

    let erase = std::process::Command::new("diskutil")
        .arg("erasevolume")
        .arg("HFS+")
        .arg("RSScriptRAMDisk")
        .arg(device)
        .output()
        .ok()?;
    if !erase.status.success() || !path.is_dir() {
        return None;
    }

    Some(path)
}

#[cfg(not(target_os = "macos"))]
fn default_ramdisk_root_dir() -> Option<std::path::PathBuf> {
    None
}
