fn operation_digest(tool: &str, arguments: &Value) -> String {
    let canonical_arguments = canonical_json(arguments);
    format!(
        "sha256:{:x}",
        Sha256::digest(format!("{tool}\n{canonical_arguments}").as_bytes())
    )
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("JSON string serialization"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON object key serialization"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn option_constraints_match(arguments: &Value, raw_constraints: &str) -> bool {
    let Ok(Value::Object(constraints)) = serde_json::from_str::<Value>(raw_constraints) else {
        return false;
    };
    constraints
        .iter()
        .all(|(key, expected)| match key.as_str() {
            "includeIgnored" | "includeHidden" => {
                let actual = arguments.get(key).cloned().unwrap_or(Value::Bool(false));
                actual == *expected
            }
            _ => false,
        })
}

fn approval_summary(tool: &str, risk: ToolRiskClass, arguments: &Value) -> Value {
    let paths = extract_paths(tool, arguments)
        .unwrap_or_default()
        .into_iter()
        .map(|path| normalized_path(&path))
        .collect::<Vec<_>>();
    json!({"operation": tool, "riskClass": risk, "paths": paths, "pathCount": paths.len(),
        "overwrite": arguments.get("overwrite"), "recursive": arguments.get("recursive"),
        "deleteMode": arguments.get("mode"), "expectedVersion": arguments.get("expectedVersion"),
        "dryRun": arguments.get("dryRun"), "budget": arguments.get("budget"),
        "editCount": edit_count(arguments), "contentBytesEstimate": content_bytes_estimate(arguments),
        "command": command_approval_summary(tool, arguments), "contentRedacted": true})
}

fn command_approval_summary(tool: &str, arguments: &Value) -> Option<Value> {
    if !matches!(tool, "command_run" | "shell_create") {
        return None;
    }
    let executable = arguments
        .get("executable")
        .and_then(Value::as_str)
        .unwrap_or("<default shell>");
    let argument_values = arguments
        .get("arguments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    for argument in argument_values {
        let encoded = canonical_json(argument);
        hasher.update(encoded.len().to_le_bytes());
        hasher.update(encoded.as_bytes());
    }
    let (executable_preview, executable_truncated) = bounded_preview(executable, 256);
    Some(json!({
        "executable": executable_preview,
        "executableTruncated": executable_truncated,
        "argumentCount": argument_values.len(),
        "argumentsSha256": format!("sha256:{:x}", hasher.finalize()),
        "argumentsRedacted": true,
    }))
}

fn bounded_preview(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}

fn edit_count(arguments: &Value) -> usize {
    arguments
        .get("edits")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn content_bytes_estimate(arguments: &Value) -> usize {
    let direct = arguments
        .get("content")
        .and_then(Value::as_str)
        .map_or(0, str::len)
        .saturating_add(
            arguments
                .get("base64")
                .and_then(Value::as_str)
                .map_or(0, |v| v.len().saturating_mul(3) / 4),
        );
    arguments
        .get("edits")
        .and_then(Value::as_array)
        .map_or(direct, |edits| {
            edits.iter().fold(direct, |total, edit| {
                total.saturating_add(edit.get("text").and_then(Value::as_str).map_or(0, str::len))
            })
        })
}

fn nonnegative_i64(value: &Value) -> Option<i64> {
    value
        .as_u64()
        .map(|value| i64::try_from(value).unwrap_or(i64::MAX))
        .or_else(|| value.as_i64().map(|value| value.max(0)))
}

fn requested_charge(tool: &str, arguments: &Value) -> GrantCharge {
    let budget = arguments.get("budget").unwrap_or(&Value::Null);
    let files = budget
        .get("maxFiles")
        .or_else(|| budget.get("maxFilesScanned"))
        .or_else(|| budget.get("maxEntriesScanned"))
        .and_then(nonnegative_i64)
        .or_else(|| arguments.get("maxItems").and_then(nonnegative_i64))
        .unwrap_or(if matches!(tool, "fs_search" | "fs_find") {
            10_000
        } else {
            1
        });
    let bytes_read = budget
        .get("maxBytesRead")
        .and_then(nonnegative_i64)
        .or_else(|| arguments.get("maxBytes").and_then(nonnegative_i64))
        .unwrap_or(if matches!(tool, "fs_search" | "fs_find") {
            64 * 1024 * 1024
        } else {
            1024 * 1024
        });
    GrantCharge { files, bytes_read }
}

fn extract_paths(tool: &str, arguments: &Value) -> RuntimeResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for role in tool_capabilities(tool).path_fields {
        match role {
            PathFieldRole::Path
            | PathFieldRole::Source
            | PathFieldRole::Destination
            | PathFieldRole::QuarantinePath
            | PathFieldRole::WorkingDirectory
            | PathFieldRole::Cwd => {
                let key = match role {
                    PathFieldRole::Path => "path",
                    PathFieldRole::Source => "source",
                    PathFieldRole::Destination => "destination",
                    PathFieldRole::QuarantinePath => "quarantinePath",
                    PathFieldRole::WorkingDirectory => "workingDirectory",
                    PathFieldRole::Cwd => "cwd",
                    _ => unreachable!(),
                };
                if let Some(path) = arguments.get(key).and_then(Value::as_str) {
                    paths.push(canonical_read_path(path)?);
                }
            }
            PathFieldRole::Paths => {
                if let Some(values) = arguments.get("paths").and_then(Value::as_array) {
                    for path in values.iter().filter_map(Value::as_str) {
                        paths.push(canonical_read_path(path)?);
                    }
                }
            }
            PathFieldRole::RequestPaths => {
                if let Some(values) = arguments.get("requests").and_then(Value::as_array) {
                    for path in values
                        .iter()
                        .filter_map(|v| v.get("path"))
                        .filter_map(Value::as_str)
                    {
                        paths.push(canonical_read_path(path)?);
                    }
                }
            }
        }
    }
    Ok(paths)
}

fn canonical_read_path(path: &str) -> RuntimeResult<PathBuf> {
    std::fs::canonicalize(path).map_err(|_| {
        RuntimeError::new(
            "approval_path_invalid",
            "approval path does not exist or cannot be canonicalized",
        )
    })
}
fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}
fn path_allowed(path: &Path, scopes: &[GrantPathScope]) -> bool {
    let path = normalized_path(path);
    scopes.iter().any(|scope| {
        let scope_path = Path::new(&scope.path);
        let scope_still_bound = std::fs::canonicalize(scope_path).is_ok_and(|canonical| {
            normalized_path(&canonical) == scope.path && scope.identity == path_identity(&canonical)
        });
        scope_still_bound
            && match scope.kind {
                GrantPathScopeKind::Exact => path == scope.path,
                GrantPathScopeKind::Subtree => {
                    path == scope.path
                        || path
                            .strip_prefix(&scope.path)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                }
            }
    })
}

fn path_identity(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let created = metadata
            .created()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(format!("created:{created}:dir:{}", metadata.is_dir()))
    }
}

async fn record_grant_denial(
    pool: &sqlx::SqlitePool,
    grant_id: &str,
    task_id: &str,
    tool: &str,
    path_count: usize,
    reason: &str,
) -> RuntimeResult<()> {
    sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,tool,path_count,reason,created_at_ms) VALUES(?,?,?,'denied',?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(grant_id).bind(task_id).bind(tool)
        .bind(i64::try_from(path_count).unwrap_or(i64::MAX)).bind(reason).bind(now_ms())
        .execute(pool).await.map_err(|_| RuntimeError::new("storage_error", "approval grant denial audit failed"))?;
    Ok(())
}

fn rejection_reason(decision_json: Option<&str>) -> Option<String> {
    serde_json::from_str::<Value>(decision_json?)
        .ok()?
        .get("reason")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod command_summary_tests {
    use super::*;

    #[test]
    fn command_summary_shows_identity_without_argv_payload() {
        let secret = "token-that-must-not-reach-the-approval-ui";
        let arguments = json!({
            "executable": "cargo",
            "arguments": ["test", "--token", secret],
            "cwd": "."
        });
        let first = approval_summary("command_run", ToolRiskClass::ProcessExecution, &arguments);
        let second = approval_summary(
            "command_run",
            ToolRiskClass::ProcessExecution,
            &json!({"executable":"cargo","arguments":["test","--token","different"],"cwd":"."}),
        );

        assert_eq!(first["command"]["executable"], "cargo");
        assert_eq!(first["command"]["argumentCount"], 3);
        assert_eq!(first["command"]["argumentsRedacted"], true);
        assert!(!first.to_string().contains(secret));
        assert_ne!(
            first["command"]["argumentsSha256"],
            second["command"]["argumentsSha256"]
        );
    }
}
