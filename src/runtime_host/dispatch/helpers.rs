use super::*;

pub(super) fn value_with_usage<T: serde::Serialize>(
    value: T,
    usage: ToolUsage,
) -> RuntimeResult<Value> {
    let mut output = serde_json::to_value(value)
        .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))?;
    let usage = serde_json::to_value(usage)
        .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))?;
    if let Value::Object(object) = &mut output {
        object.insert("usage".to_owned(), usage);
        Ok(output)
    } else {
        Ok(json!({ "result": output, "usage": usage }))
    }
}

pub(super) fn transfer_conflict_policy(input: &TransferInput) -> RuntimeResult<FsConflictPolicy> {
    match (input.conflict_policy, input.overwrite) {
        (Some(policy), Some(true)) if policy != FsConflictPolicy::Replace => {
            Err(RuntimeError::new(
                "invalid_arguments",
                "overwrite=true conflicts with conflictPolicy",
            ))
        }
        (Some(policy), _) => Ok(policy),
        (None, Some(true)) => Ok(FsConflictPolicy::Replace),
        (None, _) => Ok(FsConflictPolicy::Error),
    }
}

pub(super) fn context_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}

pub(super) fn project_folder_required_for_shell() -> RuntimeError {
    RuntimeError::new(
        "project_folder_required",
        "shell working directory requires the task project folder or an explicit absolute working path",
    )
}

pub(super) fn is_filesystem_tool(tool: &str) -> bool {
    tool.starts_with("fs_") || matches!(tool, "workspace_index_status" | "workspace_index_rebuild")
}
