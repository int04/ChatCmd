use chatcmd_core::{ExecutionMode, TaskExecutionMode, TaskId, TaskStore as _};
use serde_json::json;
use sqlx::Row as _;
use tempfile::TempDir;

use super::user_message_tests::{test_host, turn_context};
use super::*;

#[tokio::test]
async fn user_supplied_absolute_path_grant_persists_for_task() {
    let (host, agent_id, _workspace_directory) = test_host().await;
    let external = TempDir::new().expect("external directory");
    let granted_directory = external.path().join("reference project");
    std::fs::create_dir_all(&granted_directory).expect("create granted directory");
    let allowed_file = granted_directory.join("reference.txt");
    std::fs::write(&allowed_file, "outside workspace").expect("write allowed file");

    let denied_external = TempDir::new().expect("denied external directory");
    let denied_file = denied_external.path().join("not-mentioned.txt");
    std::fs::write(&denied_file, "must stay blocked").expect("write denied file");

    let scope = "conversation-explicit-path-grant";
    let turn = "turn-explicit-path-grant";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context("path-user", &agent_id, "agent_user_message", turn, scope),
            json!({"content": format!("Tham khảo từ `{}`", granted_directory.display())}),
        )
        .await
        .expect("sync user path message");
    let task_id = accepted["taskId"].as_str().expect("task id").to_owned();
    host.repository
        .set_execution_mode(&TaskExecutionMode {
            task_id: TaskId::new(task_id.clone()).expect("task id model"),
            mode: ExecutionMode::Allow,
            updated_at_ms: now_ms(),
        })
        .await
        .expect("allow task execution");

    let mut allowed_context =
        turn_context("path-read-allowed", &agent_id, "fs_read_text", turn, scope);
    allowed_context.task_id = Some(task_id.clone());
    let allowed = host
        .call_persisted(
            "fs_read_text",
            allowed_context,
            json!({"path": allowed_file.display().to_string(), "maxCharacters": 1000}),
        )
        .await
        .expect("read user-granted file");
    assert_eq!(allowed["content"], "outside workspace");

    let mut denied_context =
        turn_context("path-read-denied", &agent_id, "fs_read_text", turn, scope);
    denied_context.task_id = Some(task_id.clone());
    let denied = host
        .call_persisted(
            "fs_read_text",
            denied_context,
            json!({"path": denied_file.display().to_string(), "maxCharacters": 1000}),
        )
        .await
        .expect_err("unmentioned path must remain blocked");
    assert_eq!(denied.code, "path_outside_allowed_scope");

    let next_turn = "turn-after-explicit-path-grant";
    let mut next_user_context = turn_context(
        "path-next-user",
        &agent_id,
        "agent_user_message",
        next_turn,
        scope,
    );
    next_user_context.task_id = Some(task_id.clone());
    host.call_persisted(
        "agent_user_message",
        next_user_context,
        json!({"content":"Tiếp tục"}),
    )
    .await
    .expect("sync next user turn");

    let mut continued_context = turn_context(
        "path-read-continued",
        &agent_id,
        "fs_read_text",
        next_turn,
        scope,
    );
    continued_context.task_id = Some(task_id);
    let continued = host
        .call_persisted(
            "fs_read_text",
            continued_context,
            json!({"path": allowed_file.display().to_string(), "maxCharacters": 1000}),
        )
        .await
        .expect("user path grant must remain available in later turns of the same task");
    assert_eq!(continued["content"], "outside workspace");
}

#[tokio::test]
async fn chatgpt_bridge_reuses_existing_task_when_mcp_scope_differs_from_url_scope() {
    let (host, agent_id, _directory) = test_host().await;
    let task_id = "task-chatgpt-bridge-existing";
    let request_id = "chatgpt-request-existing";
    let submitted = "Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:\n\nKiểm tra duplicate task";
    let now = now_ms();

    sqlx::query(
        "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web','running',NULL,1,NULL,?,?)",
    )
    .bind(task_id)
    .bind(&agent_id)
    .bind(host.device.id.as_str())
    .bind("openai:url-conversation-id")
    .bind("Kiểm tra duplicate task")
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge task");

    sqlx::query(
        "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,'running',?,?,NULL,NULL,?,?,NULL)",
    )
    .bind(request_id)
    .bind(task_id)
    .bind("chatgpt-turn-existing")
    .bind(&agent_id)
    .bind("Auto")
    .bind("Kiểm tra duplicate task")
    .bind(submitted)
    .bind("conversation-url-id")
    .bind("https://chatgpt.com/c/conversation-url-id")
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge request");

    sqlx::query(
        "INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?)",
    )
    .bind(task_id)
    .bind("conversation-url-id")
    .bind("https://chatgpt.com/c/conversation-url-id")
    .bind("Auto")
    .bind(request_id)
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge conversation");

    let result = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "mcp-user-message",
                &agent_id,
                "agent_user_message",
                "turn-from-openai-session",
                "openai:mcp-session-derived",
            ),
            json!({"content": submitted}),
        )
        .await
        .expect("reuse ChatGPT bridge task");

    assert_eq!(result["taskId"], task_id);
    let row = sqlx::query("SELECT source,conversation_scope_hash FROM tasks WHERE id=?")
        .bind(task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read reused bridge task");
    assert_eq!(row.get::<String, _>("source"), "chatgpt_web");
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        "openai:mcp-session-derived"
    );
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE agent_id=?")
        .bind(&agent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("count tasks");
    assert_eq!(task_count, 1);
}
