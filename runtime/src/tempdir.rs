use std::path::PathBuf;

use crate::diagnostics::Resource;

pub struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl Resource for TempDir {}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub fn tempdir_new() -> std::io::Result<TempDir> {
    let path = std::env::temp_dir().join(format!("rsscript-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path)?;
    Ok(TempDir { path, keep: false })
}

pub fn tempdir_new_in(parent: &std::path::Path) -> std::io::Result<TempDir> {
    let path = parent.join(format!("rsscript-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path)?;
    Ok(TempDir { path, keep: false })
}

pub fn tempdir_path(dir: &TempDir) -> PathBuf {
    dir.path.clone()
}

pub fn tempdir_keep(mut dir: TempDir) -> PathBuf {
    dir.keep = true;
    dir.path.clone()
}
