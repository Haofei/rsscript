use std::path::{Component, Path, PathBuf};

pub trait RuntimePath {
    fn as_path(&self) -> &Path;
}

impl RuntimePath for Path {
    fn as_path(&self) -> &Path {
        self
    }
}

impl RuntimePath for PathBuf {
    fn as_path(&self) -> &Path {
        self.as_path()
    }
}

impl RuntimePath for String {
    fn as_path(&self) -> &Path {
        Path::new(self)
    }
}

impl RuntimePath for str {
    fn as_path(&self) -> &Path {
        Path::new(self)
    }
}

impl<T: RuntimePath + ?Sized> RuntimePath for &T {
    fn as_path(&self) -> &Path {
        (*self).as_path()
    }
}

pub fn path_from_string(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn path_join<P: RuntimePath + ?Sized>(base: &P, child: &str) -> PathBuf {
    base.as_path().join(child)
}

/// Validate and normalize a relative path using lexical components only.
pub fn path_safe_relative(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty() {
        return Err("path must be non-empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err("parent-directory traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path must name a relative file or directory".to_string());
    }
    Ok(normalized)
}

/// Join a validated relative path to a lexical root.
///
/// Provider implementations remain responsible for handle-relative filesystem
/// confinement and symlink handling when they use the returned path.
pub fn path_resolve_relative<P: RuntimePath + ?Sized>(
    root: &P,
    relative: &str,
) -> Result<PathBuf, String> {
    let relative = path_safe_relative(relative)?;
    let root = root.as_path();
    let resolved = path_normalize(&root.join(relative));
    if !resolved.starts_with(root) {
        return Err("resolved path escapes the lexical root".to_string());
    }
    Ok(resolved)
}

pub fn path_normalize<P: RuntimePath + ?Sized>(path: &P) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.as_path().components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub fn path_to_string<P: RuntimePath + ?Sized>(path: &P) -> String {
    path.as_path().to_string_lossy().to_string()
}

pub fn path_file_name<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

pub fn path_extension<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
}

pub fn path_parent<P: RuntimePath + ?Sized>(path: &P) -> Option<PathBuf> {
    path.as_path().parent().map(PathBuf::from)
}

pub fn path_is_absolute<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().is_absolute()
}

pub fn path_with_extension<P: RuntimePath + ?Sized>(path: &P, extension: &str) -> PathBuf {
    let mut path = path.as_path().to_path_buf();
    path.set_extension(extension);
    path
}

pub fn path_starts_with<P: RuntimePath + ?Sized, Q: RuntimePath + ?Sized>(
    path: &P,
    base: &Q,
) -> bool {
    path.as_path().starts_with(base.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_path_operations_do_not_touch_the_filesystem() {
        let path = path_join(&PathBuf::from("root"), "a/../b.txt");
        assert_eq!(path_to_string(&path_normalize(&path)), "root/b.txt");
        assert_eq!(path_file_name(&path), Some("b.txt".to_string()));
        assert_eq!(path_extension(&path), Some("txt".to_string()));
    }

    #[test]
    fn relative_validation_rejects_traversal() {
        assert!(path_safe_relative("data/report.csv").is_ok());
        assert!(path_safe_relative("../secret").is_err());
        assert!(path_safe_relative("").is_err());
    }
}
