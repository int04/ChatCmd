use super::*;
use std::io::Read as _;

pub(super) fn discover_manifests(root: &Path, warnings: &mut Vec<String>) -> Vec<String> {
    [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "pyproject.toml",
        "go.mod",
    ]
    .into_iter()
    .filter_map(|name| {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                Some(display(&path))
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                warnings.push(format!(
                    "{} was skipped because project manifests cannot be symlinks",
                    display(&path)
                ));
                None
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                warnings.push(rule_io_warning(&path, "metadata", &error));
                None
            }
        }
    })
    .collect()
}

pub(super) fn bundle_hash(
    root: &Path,
    rules: &[ProjectRuleRecord],
    manifests: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(display(root));
    for rule in rules {
        hasher.update(&rule.path);
        hasher.update(&rule.scope_root);
        hasher.update(&rule.content_hash);
    }
    for manifest in manifests {
        hasher.update(manifest);
        let Ok(metadata) = fs::symlink_metadata(manifest) else {
            hasher.update(b"manifest-unavailable");
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            hasher.update(b"manifest-unsafe");
            continue;
        }
        hasher.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            hasher.update(elapsed.as_nanos().to_le_bytes());
        }
        if let Ok(file) = fs::File::open(manifest) {
            let mut prefix = Vec::with_capacity(DEFAULT_MAX_FILE_BYTES);
            let _ = file
                .take(DEFAULT_MAX_FILE_BYTES as u64)
                .read_to_end(&mut prefix);
            hasher.update(sha256_hex(prefix));
        }
    }
    format!("{:x}", hasher.finalize())
}
