use std::path::{Path, PathBuf};

use serde_json::Value;

pub(super) fn argument_path_scopes(arguments: &Value) -> Vec<PathBuf> {
    let mut scopes = Vec::new();
    collect_value_paths(arguments, &mut scopes);
    scopes.sort();
    scopes.dedup();
    scopes
}

pub(super) fn scope_for_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut candidate = path.to_path_buf();
    loop {
        if candidate.exists() {
            let canonical = candidate.canonicalize().ok()?;
            return canonical.parent().is_some().then_some(canonical);
        }
        if !candidate.pop() {
            return None;
        }
    }
}

fn collect_value_paths(value: &Value, scopes: &mut Vec<PathBuf>) {
    match value {
        Value::String(value) => {
            let path = PathBuf::from(value);
            if let Some(scope) = scope_for_path(&path) {
                scopes.push(scope);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_value_paths(value, scopes);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_value_paths(value, scopes);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn grants_existing_absolute_path_from_tool_arguments() {
        let external = TempDir::new().expect("external");
        let arguments = json!({"path": external.path()});
        let scopes = argument_path_scopes(&arguments);
        assert!(scopes.contains(&external.path().canonicalize().expect("canonical")));
    }

    #[test]
    fn grants_existing_parent_for_new_absolute_target() {
        let external = TempDir::new().expect("external");
        let target = external.path().join("new-folder").join("new-file.txt");
        let arguments = json!({"path": target});
        let scopes = argument_path_scopes(&arguments);
        assert!(scopes.contains(&external.path().canonicalize().expect("canonical")));
    }

    #[test]
    fn ignores_relative_argument_paths() {
        assert!(argument_path_scopes(&json!({"path": "src/main.rs"})).is_empty());
    }
}
