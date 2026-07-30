// Secure source traversal and iterative Terraform state traversal.

struct TerraformResourceBlock {
    file: String,
    resource_type: String,
    name: String,
    body: String,
    line: usize,
}

fn canonical_terraform_root(root: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
    if is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing Terraform source root that is a symlink or reparse point: {}",
            root.display()
        ));
    }
    fs::canonicalize(root)
        .map_err(|error| format!("failed to canonicalize {}: {error}", root.display()))
}

fn collect_tf_files(
    canonical_root: &Path,
    root: &Path,
    depth: usize,
    limits: TerraformSourceLimits,
    budget: &mut TerraformSourceBudget,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > limits.max_depth {
        return Err(format!(
            "Terraform source traversal exceeded maximum depth {} at {}",
            limits.max_depth,
            root.display()
        ));
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
    if is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing to follow symlink or reparse point in Terraform source tree: {}",
            root.display()
        ));
    }
    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("failed to canonicalize {}: {error}", root.display()))?;
    ensure_beneath_root(canonical_root, &canonical)?;
    if metadata.is_file() {
        if canonical
            .extension()
            .is_some_and(|extension| extension == "tf")
        {
            account_tf_file(&canonical, metadata.len(), limits, budget)?;
            files.push(canonical);
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "Terraform source root is neither a file nor directory: {}",
            root.display()
        ));
    }
    if !visited.insert(canonical.clone()) {
        return Err(format!(
            "Terraform source traversal encountered a directory more than once: {}",
            canonical.display()
        ));
    }

    let entries = fs::read_dir(&canonical).map_err(|error| {
        format!(
            "failed to read Terraform directory {}: {error}",
            canonical.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read Terraform directory entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if is_link_or_reparse_point(&metadata) {
            return Err(format!(
                "refusing to follow symlink or reparse point in Terraform source tree: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_tf_files(
                canonical_root,
                &path,
                depth + 1,
                limits,
                budget,
                visited,
                files,
            )?;
        } else if metadata.is_file() && path.extension().is_some_and(|extension| extension == "tf")
        {
            let canonical = fs::canonicalize(&path)
                .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
            ensure_beneath_root(canonical_root, &canonical)?;
            account_tf_file(&canonical, metadata.len(), limits, budget)?;
            files.push(canonical);
        }
    }
    Ok(())
}

fn ensure_beneath_root(root: &Path, path: &Path) -> Result<(), String> {
    if path == root || path.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "Terraform source path escapes canonical root {}: {}",
            root.display(),
            path.display()
        ))
    }
}

fn is_link_or_reparse_point(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn account_tf_file(
    path: &Path,
    bytes: u64,
    limits: TerraformSourceLimits,
    budget: &mut TerraformSourceBudget,
) -> Result<(), String> {
    if bytes > limits.max_file_bytes {
        return Err(format!(
            "Terraform source file {} is {bytes} bytes, exceeding the {} byte limit",
            path.display(),
            limits.max_file_bytes
        ));
    }
    budget.files = budget
        .files
        .checked_add(1)
        .ok_or_else(|| "Terraform source file count overflow".to_owned())?;
    if budget.files > limits.max_files {
        return Err(format!(
            "Terraform source traversal exceeded the {} file limit",
            limits.max_files
        ));
    }
    budget.bytes = budget
        .bytes
        .checked_add(bytes)
        .ok_or_else(|| "Terraform source byte count overflow".to_owned())?;
    if budget.bytes > limits.max_total_bytes {
        return Err(format!(
            "Terraform source traversal exceeded the {} byte limit",
            limits.max_total_bytes
        ));
    }
    Ok(())
}

fn read_tf_file(canonical_root: &Path, path: &Path, max_bytes: u64) -> Result<String, String> {
    let link_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if is_link_or_reparse_point(&link_metadata) || !link_metadata.is_file() {
        return Err(format!(
            "Terraform source path changed to an unsupported file type: {}",
            path.display()
        ));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
    ensure_beneath_root(canonical_root, &canonical)?;
    let mut file = File::open(&canonical)
        .map_err(|error| format!("failed to read {}: {error}", canonical.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", canonical.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "Terraform source path is not a regular file: {}",
            canonical.display()
        ));
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "Terraform source file {} exceeds the {max_bytes} byte limit",
            canonical.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", canonical.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "Terraform source file {} grew beyond the {max_bytes} byte limit while reading",
            canonical.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        format!(
            "Terraform source file {} is not UTF-8: {error}",
            canonical.display()
        )
    })
}

fn collect_state_resources<'a>(
    root: &'a Value,
    resources: &mut Vec<&'a Value>,
    limits: TerraformPlanLimits,
    budget: &mut TerraformPlanBudget,
) -> Result<(), String> {
    let mut modules = vec![(root, 1_usize)];
    while let Some((module, depth)) = modules.pop() {
        if depth > limits.max_json_depth {
            return Err(format!(
                "Terraform module tree exceeds the {} level depth limit",
                limits.max_json_depth
            ));
        }
        if let Some(items) = module.get("resources").and_then(Value::as_array) {
            for resource in items {
                account_terraform_resource(limits, budget)?;
                resources.push(resource);
            }
        }
        if let Some(children) = module.get("child_modules").and_then(Value::as_array) {
            modules.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    Ok(())
}
