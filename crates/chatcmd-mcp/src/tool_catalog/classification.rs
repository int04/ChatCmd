fn operation_class(name: &str) -> ToolOperationClass {
    match name {
        "device_list"
        | "device_get"
        | "workspace_roots"
        | "workspace_index_status"
        | "process_list"
        | "process_inspect"
        | "shell_list"
        | "shell_inspect"
        | "task_get"
        | "task_list"
        | "task_artifact_list"
        | "blob_status" => ToolOperationClass::MetadataRead,
        "fs_list" | "fs_list_v2" | "fs_stat" | "fs_batch_stat" | "fs_read_text"
        | "fs_read_text_v2" | "fs_batch_read" | "fs_find" | "fs_search" | "task_artifact_read"
        | "skill_read" | "skills_list" | "project_context" => ToolOperationClass::ContentRead,
        "fs_create_directory"
        | "fs_write_text"
        | "fs_write_raw"
        | "fs_replace_text"
        | "fs_apply_edits"
        | "fs_copy"
        | "fs_move"
        | "fs_delete"
        | "fs_restore_quarantine"
        | "fs_quarantine_gc"
        | "workspace_index_rebuild"
        | "blob_begin"
        | "blob_write_chunk"
        | "blob_seal"
        | "git_commit"
        | "shell_write"
        | "shell_resize"
        | "task_artifact_create"
        | "process_kill" => ToolOperationClass::Mutation,
        "command_run" | "shell_create" | "git_status" | "git_diff" | "git_log" | "git_branch"
        | "git_show" => ToolOperationClass::ProcessExecution,
        "task_set_execution_mode" => ToolOperationClass::PermissionChange,
        "agent_user_message"
        | "agent_progress"
        | "agent_plan_question"
        | "agent_subagent_start"
        | "agent_subagent_wait"
        | "agent_turn_complete"
        | "shell_wait"
        | "shell_read" => ToolOperationClass::Lifecycle,
        "blob_abort" | "shell_close" | "shell_signal" => ToolOperationClass::StopCleanup,
        // A newly introduced tool must fail closed until it receives an
        // explicit semantic classification and catalog regression coverage.
        _ => ToolOperationClass::PermissionChange,
    }
}

fn risk_class(name: &str) -> ToolRiskClass {
    match name {
        "device_list"
        | "device_get"
        | "workspace_roots"
        | "fs_list"
        | "fs_list_v2"
        | "fs_stat"
        | "fs_batch_stat"
        | "workspace_index_status"
        | "process_list"
        | "process_inspect"
        | "shell_list"
        | "shell_inspect"
        | "task_get"
        | "task_list"
        | "task_artifact_list"
        | "blob_status" => ToolRiskClass::MetadataRead,
        "fs_read_text" | "fs_read_text_v2" | "fs_batch_read" | "task_artifact_read"
        | "skill_read" | "shell_read" => ToolRiskClass::ContentRead,
        "fs_find" | "fs_search" | "skills_list" | "project_context" => ToolRiskClass::ComputeRead,
        "fs_create_directory" | "blob_begin" | "blob_write_chunk" | "blob_seal" => {
            ToolRiskClass::Create
        }
        "fs_write_text"
        | "fs_write_raw"
        | "fs_replace_text"
        | "fs_apply_edits"
        | "workspace_index_rebuild"
        | "shell_write"
        | "shell_resize"
        | "task_artifact_create" => ToolRiskClass::Modify,
        "fs_copy" | "fs_move" | "fs_restore_quarantine" => ToolRiskClass::MoveCopy,
        "fs_delete" | "fs_quarantine_gc" | "process_kill" | "blob_abort" | "shell_close" => {
            ToolRiskClass::Destructive
        }
        "command_run" | "shell_create" | "shell_wait" | "shell_signal" | "git_status"
        | "git_diff" | "git_log" | "git_branch" | "git_show" | "git_commit" => {
            ToolRiskClass::ProcessExecution
        }
        "task_set_execution_mode"
        | "agent_user_message"
        | "agent_progress"
        | "agent_plan_question"
        | "agent_subagent_start"
        | "agent_subagent_wait"
        | "agent_turn_complete" => ToolRiskClass::Privileged,
        _ => ToolRiskClass::Privileged,
    }
}

fn path_fields(name: &str) -> Vec<PathFieldRole> {
    use PathFieldRole::{
        Cwd, Destination, Path, Paths, QuarantinePath, RequestPaths, Source, WorkingDirectory,
    };
    match name {
        "fs_batch_stat" | "fs_batch_read" => vec![Paths, RequestPaths],
        "fs_copy" | "fs_move" => vec![Source, Destination],
        "fs_restore_quarantine" => vec![QuarantinePath, Destination],
        "git_commit" => vec![Cwd, Paths],
        "git_diff" | "git_log" | "git_show" => vec![Cwd, Path],
        "git_status" | "git_branch" => vec![Cwd],
        "project_context" => vec![Paths],
        "shell_create" => vec![WorkingDirectory],
        "command_run" => vec![Cwd],
        name if name.starts_with("fs_") || name.starts_with("workspace_index_") => vec![Path],
        _ => Vec::new(),
    }
}

fn result_schema(name: &str) -> Value {
    let schema = match name {
        "fs_list_v2" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsListPageData>
        )),
        "fs_find" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsFindPageData>
        )),
        "fs_search" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsSearchPageData>
        )),
        "fs_read_text_v2" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::TextReadResultV2))
        }
        "fs_batch_read" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::FsBatchReadResult))
        }
        "fs_batch_stat" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::FsBatchStatResult))
        }
        "workspace_index_status" | "workspace_index_rebuild" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::WorkspaceIndexStatus))
        }
        "fs_apply_edits" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::ApplyEditsResult))
        }
        "command_run" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::CommandExecutionResult
        )),
        "project_context" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::ProjectContextBundle))
        }
        "fs_write_text" | "fs_write_raw" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::AtomicWriteResult))
        }
        _ => Ok(generic_result_schema()),
    }
    .expect("result schema must serialize");
    canonicalize_contract(schema)
}

fn generic_result_schema() -> Value {
    serde_json::json!({
        "$comment": "Bounded JSON value returned as MCP structuredContent; errors use the common error and usage object.",
        "anyOf": [
            {
                "type": "object",
                "maxProperties": 1024,
                "properties": {
                    "error": {
                        "type": "object",
                        "required": ["code", "message", "retryable", "approvalRequired"],
                        "properties": {
                            "code": {"type": "string", "maxLength": 256},
                            "message": {"type": "string", "maxLength": 65536},
                            "retryable": {"type": "boolean"},
                            "approvalRequired": {"type": "boolean"},
                            "phase": {"type": ["string", "null"]},
                            "outcome": {"type": "string"},
                            "recovery": {"type": "string"}
                        },
                        "additionalProperties": true
                    },
                    "usage": true
                },
                "additionalProperties": true
            },
            {"type": "array", "maxItems": 100000, "items": true},
            {"type": "string", "maxLength": 16777216},
            {"type": ["number", "integer", "boolean", "null"]}
        ]
    })
}

fn is_mutating(name: &str) -> bool {
    name.starts_with("blob_")
        || name.starts_with("fs_write")
        || name.starts_with("fs_replace")
        || name == "fs_apply_edits"
        || name == "workspace_index_rebuild"
        || name.starts_with("fs_create")
        || matches!(
            name,
            "fs_copy"
                | "fs_move"
                | "fs_delete"
                | "fs_restore_quarantine"
                | "fs_quarantine_gc"
                | "git_commit"
                | "command_run"
                | "process_kill"
        )
        || name.starts_with("shell_write")
        || name.starts_with("shell_signal")
        || name.starts_with("shell_resize")
        || name.starts_with("shell_close")
        || name.starts_with("task_set_")
        || name == "task_artifact_create"
        || name.starts_with("agent_")
}
