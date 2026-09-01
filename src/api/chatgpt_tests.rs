use chatcmd_core::{McpAgentStore as _, NewMcpAgent};
use chatcmd_storage::SqliteRepository;
use tempfile::TempDir;

use super::chatgpt::{BridgeStartedBinding, persist_bridge_started_binding};
use super::chatgpt_completion::{BrowserCompletion, persist_browser_completion};

#[tokio::test]
async fn bridge_started_keeps_concurrent_winner_and_removes_losing_candidate() {
    let directory = TempDir::new().expect("temporary directory");
    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .expect("open repository");
    let agent = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Bridge started race".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent")
        .agent;
    let request_id = "bridge-started-race-request";
    let winner_task_id = "task-chatgpt-mcp-winner";
    let losing_candidate_id = "task-chatgpt-browser-loser";
    let project_folder = "D:\\DEV\\race-project";
    let now = super::now_ms();

    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',NULL,1,'running',NULL,1,NULL,?,?)")
        .bind(winner_task_id)
        .bind(agent.id.as_str())
        .bind(bootstrap.device.id.as_str())
        .bind("openai:mcp-winner")
        .bind("MCP winner")
        .bind(now)
        .bind(now)
        .execute(repository.pool())
        .await
        .expect("insert winning task");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,project_folder,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,?,'queued',NULL,NULL,NULL,NULL,?,?,NULL)")
        .bind(request_id)
        .bind(winner_task_id)
        .bind("bridge-started-race-turn")
        .bind(agent.id.as_str())
        .bind("Auto")
        .bind("Kiểm tra race")
        .bind("Sử dụng plugin @worker để kiểm tra race")
        .bind(project_folder)
        .bind(now)
        .bind(now)
        .execute(repository.pool())
        .await
        .expect("insert bridge request");

    let authoritative = persist_bridge_started_binding(
        &repository,
        &BridgeStartedBinding {
            request_id,
            candidate_task_id: losing_candidate_id,
            agent_id: agent.id.as_str(),
            device_id: bootstrap.device.id.as_str(),
            scope: "openai:browser-scope",
            title: "Kiểm tra race",
            project_folder: Some(project_folder),
            model: "Auto",
            conversation_id: "browser-conversation-id",
            conversation_url: "https://chatgpt.com/c/browser-conversation-id",
            now,
        },
    )
    .await
    .expect("persist monotonic bridge binding");

    assert_eq!(authoritative, winner_task_id);
    let stored_request_task: String =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_bridge_requests WHERE id=?")
            .bind(request_id)
            .fetch_one(repository.pool())
            .await
            .expect("read request binding");
    assert_eq!(stored_request_task, winner_task_id);
    let losing_candidate_exists: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
            .bind(losing_candidate_id)
            .fetch_one(repository.pool())
            .await
            .expect("check losing candidate");
    assert_eq!(losing_candidate_exists, 0);
    let conversation_task: String =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_conversations WHERE conversation_id=?")
            .bind("browser-conversation-id")
            .fetch_one(repository.pool())
            .await
            .expect("read conversation binding");
    assert_eq!(conversation_task, winner_task_id);
    let stored_project_folder: String =
        sqlx::query_scalar("SELECT project_folder FROM tasks WHERE id=?")
            .bind(winner_task_id)
            .fetch_one(repository.pool())
            .await
            .expect("read authoritative task project folder");
    assert_eq!(stored_project_folder, project_folder);
}

async fn browser_completion_fixture(
    with_mcp_user: bool,
) -> (TempDir, SqliteRepository, String, String, String) {
    let directory = TempDir::new().expect("temporary directory");
    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .expect("open repository");
    let agent = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Browser completion".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent")
        .agent;
    let request_id = "browser-completion-request".to_owned();
    let task_id = "task-browser-completion".to_owned();
    let submitted = "Sử dụng plugin @worker để kiểm tra raw bubble".to_owned();
    let turn_id = "turn-browser-completion".to_owned();
    let now = super::now_ms();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?, 'chatgpt_web',1,'running',1,?,?)")
        .bind(&task_id).bind(agent.id.as_str()).bind(bootstrap.device.id.as_str())
        .bind("Raw bubble").bind(now).bind(now).execute(repository.pool()).await
        .expect("insert task");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,'running',?,?,?,?)")
        .bind(&request_id).bind(&task_id).bind(&turn_id).bind(agent.id.as_str()).bind("Auto")
        .bind("Kiểm tra raw bubble").bind(&submitted).bind("conversation-browser-completion")
        .bind("https://chatgpt.com/c/conversation-browser-completion").bind(now).bind(now)
        .execute(repository.pool()).await.expect("insert request");
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?)")
        .bind(&task_id).bind("conversation-browser-completion")
        .bind("https://chatgpt.com/c/conversation-browser-completion").bind("Auto")
        .bind(&request_id).bind(now).bind(now).execute(repository.pool()).await
        .expect("insert conversation");
    if with_mcp_user {
        let payload = serde_json::json!({"content":submitted,"provider":"mcp"});
        sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'user','message',?,?,?)")
            .bind("mcp-user-browser-completion").bind(&task_id).bind("mcp-turn-browser-completion")
            .bind("mcp-user-browser-completion").bind(payload.to_string()).bind(now)
            .execute(repository.pool()).await.expect("insert MCP user event");
    }
    (directory, repository, request_id, task_id, submitted)
}

#[tokio::test]
async fn raw_browser_completion_injects_idempotent_user_and_final_events_without_mcp() {
    let (_directory, repository, request_id, task_id, submitted) =
        browser_completion_fixture(false).await;
    let now = super::now_ms();
    let input = BrowserCompletion {
        request_id: &request_id,
        conversation_id: Some("conversation-browser-completion"),
        conversation_url: Some("https://chatgpt.com/c/conversation-browser-completion"),
        assistant_content: "Raw ChatGPT answer",
        now,
    };
    let first = persist_browser_completion(&repository, &input)
        .await
        .expect("persist browser completion");
    assert!(first.user.is_some());
    assert!(first.status.is_some());
    let repeated = persist_browser_completion(&repository, &input)
        .await
        .expect("repeat browser completion");
    assert!(repeated.user.is_none());
    assert!(repeated.status.is_none());
    let stored_submitted: String = sqlx::query_scalar("SELECT json_extract(payload_json,'$.submittedContent') FROM timeline_events WHERE event_id=?")
        .bind(format!("chatgpt-user-{request_id}")).fetch_one(repository.pool()).await
        .expect("read synthetic submitted content");
    assert_eq!(stored_submitted, submitted);
    let final_matches: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM timeline_events user_event JOIN timeline_events final_event ON final_event.task_id=user_event.task_id AND final_event.turn_id=user_event.turn_id WHERE user_event.task_id=? AND COALESCE(json_extract(user_event.payload_json,'$.submittedContent'),json_extract(user_event.payload_json,'$.content'))=? AND json_extract(final_event.payload_json,'$.status')='completed')")
        .bind(&task_id).bind(&submitted).fetch_one(repository.pool()).await
        .expect("check browser completion final marker");
    assert_eq!(final_matches, 1);
    let task_status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(&task_id)
        .fetch_one(repository.pool())
        .await
        .expect("read task status");
    assert_eq!(task_status, "completed");
    let active_request: Option<String> =
        sqlx::query_scalar("SELECT active_request_id FROM chatgpt_conversations WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(repository.pool())
            .await
            .expect("read active request");
    assert!(active_request.is_none());
}

#[tokio::test]
async fn raw_browser_completion_reuses_mcp_user_and_only_injects_missing_final() {
    let (_directory, repository, request_id, _task_id, _submitted) =
        browser_completion_fixture(true).await;
    let input = BrowserCompletion {
        request_id: &request_id,
        conversation_id: None,
        conversation_url: None,
        assistant_content: "Raw answer after MCP user sync",
        now: super::now_ms(),
    };
    let events = persist_browser_completion(&repository, &input)
        .await
        .expect("persist completion after MCP user");
    assert!(events.user.is_none());
    assert!(events.status.is_some());
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND actor='user'")
            .bind("task-browser-completion")
            .fetch_one(repository.pool())
            .await
            .expect("count users");
    assert_eq!(user_count, 1);
    let final_turn: String =
        sqlx::query_scalar("SELECT turn_id FROM timeline_events WHERE event_id=?")
            .bind(format!("chatgpt-result-{request_id}"))
            .fetch_one(repository.pool())
            .await
            .expect("read browser final turn");
    assert_eq!(final_turn, "mcp-turn-browser-completion");
}
