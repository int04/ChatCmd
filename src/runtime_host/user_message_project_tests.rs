use chatcmd_runtime::OperationContext;
use serde_json::json;
use tempfile::TempDir;

use super::user_message_tests::{test_host, turn_context};
use super::*;

#[tokio::test]
async fn runtime_api_resolves_task_project_folder() {
    let (host, agent_id, directory) = test_host().await;
    let project = directory.path().join("project-root");
    std::fs::create_dir_all(&project).expect("create project root");
    let task_id = "task-project-folder-runtime-api";
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,NULL,NULL,'mcp',?,'running',NULL,1,NULL,0,0)")
        .bind(task_id)
        .bind(&agent_id)
        .bind(host.device.id.as_str())
        .bind(project.display().to_string())
        .execute(host.repository.pool())
        .await
        .expect("insert task project folder");

    let resolved = <RuntimeHost as chatcmd_mcp::RuntimeApi>::project_folder(&host, Some(task_id))
        .await
        .expect("resolve project folder");
    assert_eq!(
        resolved.as_deref(),
        Some(project.display().to_string().as_str())
    );
}

#[tokio::test]
async fn user_message_and_workspace_roots_report_the_task_project_folder() {
    let (host, agent_id, configured_workspace) = test_host().await;
    let project = TempDir::new().expect("external task project");
    let expected = project
        .path()
        .canonicalize()
        .expect("canonical task project")
        .to_string_lossy()
        .into_owned();
    let turn_id = "turn-task-workspace-result";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "task-workspace-user-message",
                &agent_id,
                "agent_user_message",
                turn_id,
                "conversation-task-workspace-result",
            ),
            json!({"content":format!("Kiểm tra dự án `{expected}`")}),
        )
        .await
        .expect("sync task project folder");
    assert_eq!(accepted["projectFolder"], expected);

    let mut roots_context =
        OperationContext::new("task-workspace-roots", &agent_id, "workspace_roots");
    roots_context.task_id = accepted["taskId"].as_str().map(str::to_owned);
    roots_context.turn_id = Some(turn_id.to_owned());
    let roots = host
        .dispatch("workspace_roots", roots_context, json!({}))
        .await
        .expect("read task workspace roots");

    assert_eq!(roots, json!([expected]));
    assert_ne!(
        roots,
        json!([configured_workspace.path().display().to_string()]),
        "workspace_roots must not expose the process-wide configured root"
    );
}

#[tokio::test]
async fn workspace_roots_without_task_context_does_not_fallback_to_process_root() {
    let (host, agent_id, _configured_workspace) = test_host().await;
    let roots = host
        .dispatch(
            "workspace_roots",
            OperationContext::new("workspace-roots-without-task", agent_id, "workspace_roots"),
            json!({}),
        )
        .await
        .expect("read roots without task context");

    assert_eq!(roots, json!([]));
}

#[tokio::test]
async fn shell_create_requires_a_task_project_folder_when_working_directory_is_omitted() {
    let (host, agent_id, _directory) = test_host().await;
    let error = host
        .dispatch(
            "shell_create",
            OperationContext::new("shell-without-project", agent_id, "shell_create"),
            json!({}),
        )
        .await
        .expect_err("shell must not fall back to the configured workspace root");
    assert_eq!(error.code, "project_folder_required");
    assert_eq!(
        error.message,
        "shell working directory requires the task project folder or an explicit absolute working path"
    );
}

#[tokio::test]
async fn relative_filesystem_path_uses_task_project_folder_outside_configured_roots() {
    let (host, agent_id, _directory) = test_host().await;
    let project = TempDir::new().expect("external task project");
    std::fs::write(project.path().join("outside.txt"), "task scoped")
        .expect("write task project file");
    let task_id = "task-external-project-folder";
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,NULL,NULL,'mcp',?,'running',NULL,1,NULL,0,0)")
        .bind(task_id)
        .bind(&agent_id)
        .bind(host.device.id.as_str())
        .bind(project.path().display().to_string())
        .execute(host.repository.pool())
        .await
        .expect("insert external task project");
    let mut context = OperationContext::new("read-relative-task-project", agent_id, "fs_read_text");
    context.task_id = Some(task_id.to_owned());

    let result = host
        .dispatch(
            "fs_read_text",
            context,
            json!({"path":"outside.txt","maxCharacters":100}),
        )
        .await
        .expect("read relative path from task project scope");
    assert_eq!(result["content"], "task scoped");
}

#[tokio::test]
async fn delegated_child_inherits_project_folder_and_keeps_internal_user_message_sync() {
    let (host, agent_id, directory) = test_host().await;
    let project = directory.path().join("delegated-project");
    std::fs::create_dir_all(&project).expect("create delegated project");
    let parent_scope = "conversation-subagent-internal-sync";
    let parent_turn = "turn-parent-subagent-internal-sync";
    let parent = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "parent-user",
                &agent_id,
                "agent_user_message",
                parent_turn,
                parent_scope,
            ),
            json!({"content":format!("Create one delegated child for `{}`", project.display())}),
        )
        .await
        .expect("sync parent user message");
    let parent_task = parent["taskId"].as_str().expect("parent task");
    let mut start_context =
        OperationContext::new("subagent-start", &agent_id, "agent_subagent_start");
    start_context.task_id = Some(parent_task.to_owned());
    start_context.turn_id = Some(parent_turn.to_owned());
    let registration = host
        .call_persisted(
            "agent_subagent_start",
            start_context,
            json!({"name":"Reader","request":"Read one file"}),
        )
        .await
        .expect("register child");
    assert_eq!(registration["taskId"], parent_task);
    let child_task = registration["childTaskId"]
        .as_str()
        .expect("child task")
        .to_owned();
    let parent_and_child_folders = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT parent.project_folder,child.project_folder FROM tasks parent JOIN tasks child ON child.id=? WHERE parent.id=?",
    )
    .bind(&child_task)
    .bind(parent_task)
    .fetch_one(host.repository.pool())
    .await
    .expect("read parent and child project folders");
    assert_eq!(parent_and_child_folders.1, parent_and_child_folders.0);
    assert_eq!(
        parent_and_child_folders.1.as_deref(),
        Some(
            project
                .canonicalize()
                .expect("canonical delegated project")
                .to_string_lossy()
                .as_ref()
        )
    );
    let marker = registration["delegationMarker"].as_str().expect("marker");
    let child_turn = format!(
        "turn-{}",
        registration["subagentId"].as_str().expect("subagent id")
    );

    let mut child_user = OperationContext::new("child-user", &agent_id, "agent_user_message");
    child_user.task_id = Some(child_task.clone());
    child_user.turn_id = Some(child_turn.clone());
    host.call_persisted(
        "agent_user_message",
        child_user,
        json!({"content":format!("Read one file\n\n{marker}")}),
    )
    .await
    .expect("sync child user message");

    let mut roots = OperationContext::new("child-roots", &agent_id, "workspace_roots");
    roots.task_id = Some(child_task);
    roots.turn_id = Some(child_turn);
    host.ensure_call_identity(&mut roots, None)
        .await
        .expect("normalize child internal identity");
    host.ensure_user_message_synced(&roots)
        .await
        .expect("child internal identity must see synchronized user message");
}
