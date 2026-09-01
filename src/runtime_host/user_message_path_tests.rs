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
    let bound_task = host
        .repository
        .task(&TaskId::new(task_id.clone()).expect("task id model"))
        .await
        .expect("read bound task")
        .expect("bound task");
    assert_eq!(
        bound_task.project_folder.as_deref(),
        Some(
            granted_directory
                .canonicalize()
                .expect("canonical granted directory")
                .to_string_lossy()
                .as_ref()
        )
    );
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
async fn single_file_path_binds_its_parent_and_later_messages_do_not_override_it() {
    let (host, agent_id, _workspace_directory) = test_host().await;
    let first = TempDir::new().expect("first project directory");
    let file = first.path().join("src.rs");
    std::fs::write(&file, "fn main() {}").expect("write project file");
    let second = TempDir::new().expect("second project directory");
    let scope = "conversation-file-project-binding";

    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "file-path-user",
                &agent_id,
                "agent_user_message",
                "turn-file-path",
                scope,
            ),
            json!({"content": format!("Kiểm tra file `{}`", file.display())}),
        )
        .await
        .expect("sync file path message");
    let task_id = accepted["taskId"].as_str().expect("task id").to_owned();

    let mut next_context = turn_context(
        "second-path-user",
        &agent_id,
        "agent_user_message",
        "turn-second-path",
        scope,
    );
    next_context.task_id = Some(task_id.clone());
    host.call_persisted(
        "agent_user_message",
        next_context,
        json!({"content": format!("Tiếp tục ở `{}`", second.path().display())}),
    )
    .await
    .expect("sync later path message");

    let task = host
        .repository
        .task(&TaskId::new(task_id).expect("task id model"))
        .await
        .expect("read task")
        .expect("task");
    assert_eq!(
        task.project_folder.as_deref(),
        Some(
            first
                .path()
                .canonicalize()
                .expect("canonical first project")
                .to_string_lossy()
                .as_ref()
        )
    );
}

#[tokio::test]
async fn multiple_absolute_paths_do_not_bind_an_ambiguous_project_folder() {
    let (host, agent_id, _workspace_directory) = test_host().await;
    let first = TempDir::new().expect("first directory");
    let second = TempDir::new().expect("second directory");
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "ambiguous-path-user",
                &agent_id,
                "agent_user_message",
                "turn-ambiguous-path",
                "conversation-ambiguous-path-binding",
            ),
            json!({
                "content": format!(
                    "So sánh `{}` với `{}`",
                    first.path().display(),
                    second.path().display()
                )
            }),
        )
        .await
        .expect("sync ambiguous path message");
    let task_id =
        TaskId::new(accepted["taskId"].as_str().expect("task id")).expect("task id model");
    let task = host
        .repository
        .task(&task_id)
        .await
        .expect("read task")
        .expect("task");
    assert_eq!(task.project_folder, None);
}

#[tokio::test]
async fn chatgpt_bridge_reuses_existing_task_when_chatgpt_reformats_the_prompt() {
    let (host, agent_id, _directory) = test_host().await;
    let task_id = "task-chatgpt-bridge-existing";
    let request_id = "chatgpt-request-existing";
    let submitted = "Sử dụng plugin @test_rust\n\nThư mục dự án: D:\\DEV\\CmdGPT\\ChatCmdClient\n\nđể thực hiện yêu cầu sau: Kiểm tra http://localhost:8080/api/local/payment/create \n\nVí dụ abcd ";
    let message_from_chatgpt = submitted
        .replacen("@test_rust", "@test\\_rust", 1)
        .replacen("http://localhost:8080/api/local/payment/create", "[http://localhost:8080/api/local/payment/create](http://localhost:8080/api/local/payment/create)", 1)
        .replacen("Ví dụ abcd ", "Ví dụ abcd\u{00a0}", 1);
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
            json!({"content": message_from_chatgpt}),
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

#[tokio::test]
async fn chatgpt_bridge_uses_explicit_task_when_identical_messages_are_active() {
    let (host, agent_id, _directory) = test_host().await;
    let submitted = "Sử dụng plugin @test_rust\n\nThư mục dự án: D:\\DEV\\CmdGPT\\ChatCmdClient\n\nđể thực hiện yêu cầu sau: commit";
    let first_task = "task-chatgpt-bridge-commit-a";
    let second_task = "task-chatgpt-bridge-commit-b";
    let now = now_ms();

    for (task_id, scope) in [
        (first_task, "openai:commit-chat-a"),
        (second_task, "openai:commit-chat-b"),
    ] {
        sqlx::query(
            "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web','running',NULL,1,NULL,?,?)",
        )
        .bind(task_id)
        .bind(&agent_id)
        .bind(host.device.id.as_str())
        .bind(scope)
        .bind("Commit")
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert bridge task");

        sqlx::query(
            "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,'running',?,?,NULL,NULL,?,?,NULL)",
        )
        .bind(format!("request-{task_id}"))
        .bind(task_id)
        .bind(format!("bridge-turn-{task_id}"))
        .bind(&agent_id)
        .bind("Auto")
        .bind("commit")
        .bind(submitted)
        .bind(format!("conversation-{task_id}"))
        .bind(format!("https://chatgpt.com/c/conversation-{task_id}"))
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert active bridge request");
    }

    let mut context = turn_context(
        "mcp-commit-user-message",
        &agent_id,
        "agent_user_message",
        "turn-commit",
        "openai:new-host-scope",
    );
    context.task_id = Some(second_task.to_owned());
    let result = host
        .call_persisted("agent_user_message", context, json!({"content": submitted}))
        .await
        .expect("reuse the explicit conversation task");

    assert_eq!(result["taskId"], second_task);
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE agent_id=?")
        .bind(&agent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("count tasks");
    assert_eq!(task_count, 2, "must not create a third ghost task");
}

#[tokio::test]
async fn chatgpt_bridge_claims_first_tool_call_before_user_message_sync() {
    let (host, agent_id, _directory) = test_host().await;
    let task_id = "task-chatgpt-bridge-pre-user-tool";
    let request_id = "chatgpt-request-pre-user-tool";
    let submitted = "Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:\n\nKiểm tra tool đến trước user message";
    let now = now_ms();

    sqlx::query(
        "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web','running',NULL,1,NULL,?,?)",
    )
    .bind(task_id)
    .bind(&agent_id)
    .bind(host.device.id.as_str())
    .bind("openai:WEB:temporary-browser-scope")
    .bind("Tool trước user message")
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
    .bind("chatgpt-turn-pre-user-tool")
    .bind(&agent_id)
    .bind("Auto")
    .bind("Kiểm tra tool đến trước user message")
    .bind(submitted)
    .bind("WEB:temporary-browser-id")
    .bind("https://chatgpt.com/c/WEB:temporary-browser-id")
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert active bridge request");

    let mut context = turn_context(
        "pre-user-tool-call",
        &agent_id,
        "workspace_roots",
        "turn-from-openai-session",
        "openai:host-session-scope",
    );
    host.ensure_call_identity(&mut context, None)
        .await
        .expect("first tool call should reuse the unique active ChatGPT bridge task");

    assert_eq!(context.task_id.as_deref(), Some(task_id));
    let row = sqlx::query("SELECT source,conversation_scope_hash FROM tasks WHERE id=?")
        .bind(task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read reused bridge task");
    assert_eq!(row.get::<String, _>("source"), "chatgpt_web");
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        "openai:host-session-scope"
    );
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE agent_id=?")
        .bind(&agent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("count tasks");
    assert_eq!(
        task_count, 1,
        "pre-user tool call must not create a ghost approval task"
    );
}

#[tokio::test]
async fn chatgpt_bridge_claims_unbound_request_before_bridge_started() {
    let (host, agent_id, _directory) = test_host().await;
    let request_id = "chatgpt-request-unbound-before-started";
    let submitted = "Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:\n\nClaim request trước bridge_started";
    let now = now_ms();

    sqlx::query(
        "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,NULL,?,?,?,?,?,'queued',NULL,NULL,NULL,NULL,?,?,NULL)",
    )
    .bind(request_id)
    .bind("chatgpt-turn-unbound")
    .bind(&agent_id)
    .bind("Auto")
    .bind("Claim request trước bridge_started")
    .bind(submitted)
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert unbound bridge request");

    let mut context = turn_context(
        "early-tool-before-started",
        &agent_id,
        "workspace_roots",
        "turn-early-tool-before-started",
        "openai:early-host-scope",
    );
    host.ensure_call_identity(&mut context, None)
        .await
        .expect("early tool should claim the only unbound ChatGPT bridge request");

    let task_id = context.task_id.clone().expect("claimed task id");
    let request_task: Option<String> =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_bridge_requests WHERE id=?")
            .bind(request_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read claimed request task");
    assert_eq!(request_task.as_deref(), Some(task_id.as_str()));

    let row =
        sqlx::query("SELECT source,allow_execute,conversation_scope_hash FROM tasks WHERE id=?")
            .bind(&task_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read claimed task");
    assert_eq!(row.get::<String, _>("source"), "chatgpt_web");
    assert_eq!(row.get::<i64, _>("allow_execute"), 1);
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        "openai:early-host-scope"
    );

    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE agent_id=?")
        .bind(&agent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("count claimed tasks");
    assert_eq!(
        task_count, 1,
        "claiming must not create an approval ghost task"
    );
}
