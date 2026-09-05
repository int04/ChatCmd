use super::*;

pub(super) fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn rule_io_warning(path: &Path, operation: &str, error: &std::io::Error) -> String {
    let category = if error.kind() == std::io::ErrorKind::PermissionDenied {
        "permission_denied"
    } else {
        "io_error"
    };
    format!("{category}: {} {operation} failed: {error}", display(path))
}

pub(super) fn read_rule_range(
    root: &Path,
    path: &Path,
    scope: &Path,
    kind: ProjectRuleKind,
    precedence: usize,
    max_bytes: usize,
    range: &ProjectContextRange,
) -> RuntimeResult<ProjectRuleRecord> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        RuntimeError::new(
            "project_context_rule_unavailable",
            format!("rule metadata is unavailable: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RuntimeError::new(
            "project_context_range_invalid",
            "continuation path is not a regular project rule file",
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        RuntimeError::new(
            "project_context_rule_unavailable",
            format!("rule cannot be resolved: {error}"),
        )
    })?;
    if !canonical.starts_with(root) || display(&canonical) != range.path {
        return Err(RuntimeError::new(
            "project_context_range_invalid",
            "continuation path does not resolve to its applicable project rule",
        ));
    }
    let bytes = fs::read(&canonical).map_err(|error| {
        RuntimeError::new(
            "project_context_rule_unavailable",
            format!("rule cannot be read: {error}"),
        )
    })?;
    let content_hash = sha256_hex(&bytes);
    let version_token = format!("sha256:{content_hash}");
    if version_token != range.version_token {
        return Err(RuntimeError::new(
            "project_context_version_conflict",
            "project rule changed before its continuation was read",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        RuntimeError::new(
            "project_context_rule_invalid",
            "project rule continuation is not valid UTF-8",
        )
    })?;
    if range.offset >= bytes.len() || !text.is_char_boundary(range.offset) {
        return Err(RuntimeError::new(
            "project_context_range_invalid",
            "continuation offset must be a valid UTF-8 boundary within the rule",
        ));
    }
    let end = floor_char_boundary(text, range.offset.saturating_add(max_bytes));
    let truncated = end < bytes.len();
    Ok(ProjectRuleRecord {
        path: display(&canonical),
        scope_root: display(scope),
        kind,
        version_token,
        content_hash,
        precedence,
        content: text[range.offset..end].to_owned(),
        truncated,
        next_range: truncated.then(|| format!("bytes={end}..{}", bytes.len())),
        warnings: truncated
            .then(|| "rule continuation was truncated by the configured byte budget".to_owned())
            .into_iter()
            .collect(),
    })
}
