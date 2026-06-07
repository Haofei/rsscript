//! Capability taxonomy gate: every `category` declared in a shipped package's
//! `rsspkg.toml` must be a canonical capability category. This keeps the taxonomy
//! (src/capability.rs) and the packages in sync — adding a new category to a
//! package without registering it fails here.

mod common;

use std::path::Path;

use rsscript::is_known_capability_category;

fn categories_in(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("category = \"")?;
            rest.strip_suffix('"').map(str::to_string)
        })
        .collect()
}

#[test]
fn shipped_package_capability_categories_are_canonical() {
    let mut unknown = Vec::new();
    for dir in ["rss", "core", "examples"] {
        if !Path::new(dir).exists() {
            continue;
        }
        for path in common::recursive_paths_with_extension(dir, "toml") {
            if path.file_name().and_then(|n| n.to_str()) != Some("rsspkg.toml") {
                continue;
            }
            for category in categories_in(&path) {
                if !is_known_capability_category(&category) {
                    unknown.push(format!("{}: `{category}`", path.display()));
                }
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "unrecognized capability categories (register them in src/capability.rs):\n{}",
        unknown.join("\n")
    );
}
