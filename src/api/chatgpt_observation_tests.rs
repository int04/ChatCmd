use super::chatgpt_completion::{BrowserCompletion, persist_browser_completion};
use super::chatgpt_observation::{BrowserMessage, Observation, persist_observation};
use axum::http::StatusCode;
use chatcmd_core::{McpAgentStore as _, NewMcpAgent};
use chatcmd_storage::SqliteRepository;
use serde_json::{Value, json};
use tempfile::TempDir;

const REQUEST: &str = "request-think";
const TASK: &str = "task-think";
const CREATED: i64 = 1_000;

async fn fixture() -> (TempDir, SqliteRepository) {
    let directory = TempDir::new().unwrap();
    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .unwrap();
    let agent = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Think".into(),
            enabled: true,
        })
        .await
        .unwrap()
        .agent;
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,'Think','chatgpt_web','running',1,?,?)")
        .bind(TASK).bind(agent.id.as_str()).bind(bootstrap.device.id.as_str()).bind(CREATED).bind(CREATED).execute(repository.pool()).await.unwrap();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES(?,?,'browser-turn',?,'Auto','Hello','Hello','running','conv-think','https://chatgpt.com/c/conv-think',?,?)")
        .bind(REQUEST).bind(TASK).bind(agent.id.as_str()).bind(CREATED).bind(CREATED).execute(repository.pool()).await.unwrap();
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,'conv-think','https://chatgpt.com/c/conv-think','Auto',?,?,?)")
        .bind(TASK).bind(REQUEST).bind(CREATED).bind(CREATED).execute(repository.pool()).await.unwrap();
    (directory, repository)
}
fn input(revision: u64, content: &str, completed: bool) -> Observation {
    Observation {
        user_message_id: None,
        revision,
        conversation_id: "conv-think".into(),
        conversation_url: "https://chatgpt.com/c/conv-think".into(),
        messages: vec![BrowserMessage {
            id: "part-1".into(),
            kind: "commentary".into(),
            content: content.into(),
        }],
        completed,
    }
}
async fn snapshot(repository: &SqliteRepository) -> Value {
    let text: String = sqlx::query_scalar(
        "SELECT payload_json FROM timeline_events WHERE event_id='chatgpt-think-request-think'",
    )
    .fetch_one(repository.pool())
    .await
    .unwrap();
    serde_json::from_str(&text).unwrap()
}
async fn status(repository: &SqliteRepository) -> String {
    sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(TASK)
        .fetch_one(repository.pool())
        .await
        .unwrap()
}
async fn mcp_user(repository: &SqliteRepository, turn: &str, request: &str, at: i64) {
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'user','message',?,?,?)")
        .bind(format!("mcp-{turn}")).bind(TASK).bind(turn).bind(format!("mcp-{turn}"))
        .bind(json!({"role":"user","content":"Hello","bridgeRequestId":request,"browserTurnId":"browser-turn"}).to_string())
        .bind(at).execute(repository.pool()).await.unwrap();
}
async fn complete(repository: &SqliteRepository, content: &str) {
    persist_browser_completion(
        repository,
        &BrowserCompletion {
            request_id: REQUEST,
            conversation_id: None,
            conversation_url: None,
            assistant_content: content,
            now: CREATED + 100,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn snapshots_upsert_out_of_order_and_survive_database_reopen() {
    let (directory, repository) = fixture().await;
    let first = persist_observation(&repository, REQUEST, &input(10, "Public commentary", false))
        .await
        .unwrap();
    assert!(first.user.is_some());
    assert!(first.snapshot.is_some());
    assert_eq!(first.turn_id, "browser-turn");
    assert!(
        persist_observation(&repository, REQUEST, &input(10, "Public commentary", false))
            .await
            .unwrap()
            .snapshot
            .is_none()
    );
    persist_observation(
        &repository,
        REQUEST,
        &input(12, "Updated public commentary", false),
    )
    .await
    .unwrap();
    assert!(
        persist_observation(&repository, REQUEST, &input(11, "Stale", false))
            .await
            .unwrap()
            .snapshot
            .is_none()
    );
    assert_eq!(status(&repository).await, "running");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=?")
        .bind(TASK)
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 2, "one user plus one replaceable snapshot");
    repository.pool().close().await;
    let (reopened, _) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .unwrap();
    assert_eq!(
        snapshot(&reopened).await["messages"][0]["content"],
        "Updated public commentary"
    );
}

#[tokio::test]
async fn final_snapshot_is_monotonic_and_does_not_complete_task() {
    let (_directory, repository) = fixture().await;
    persist_observation(
        &repository,
        REQUEST,
        &input(20, "Final public answer", true),
    )
    .await
    .unwrap();
    let stale = persist_observation(&repository, REQUEST, &input(21, "Delayed streaming", false))
        .await
        .unwrap();
    assert!(stale.snapshot.is_none());
    assert_eq!(stale.revision, 20);
    assert_eq!(status(&repository).await, "running");
    complete(&repository, "Final public answer").await;
    assert_eq!(status(&repository).await, "completed");
    assert_eq!(
        snapshot(&repository).await["messages"][0]["content"],
        "Final public answer"
    );
}

#[tokio::test]
async fn late_mcp_rehomes_both_sources_without_losing_browser_snapshot() {
    let (_directory, repository) = fixture().await;
    persist_observation(&repository, REQUEST, &input(1, "Before MCP", false))
        .await
        .unwrap();
    let link = crate::chatgpt_transcript::request_for_turn(&repository, TASK, "mcp-turn", "Hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(link.request_id, REQUEST);
    mcp_user(&repository, "mcp-turn", REQUEST, CREATED + 10).await;
    let next = persist_observation(&repository, REQUEST, &input(2, "After MCP", false))
        .await
        .unwrap();
    assert_eq!(next.turn_id, "mcp-turn");
    assert!(next.user.is_none());
    let turns: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT turn_id) FROM timeline_events WHERE task_id=?")
            .bind(TASK)
            .fetch_one(repository.pool())
            .await
            .unwrap();
    assert_eq!(turns, 1, "pagination must see one combined turn");
    assert!(
        crate::chatgpt_transcript::request_for_turn(&repository, TASK, "unrelated-turn", "Hello")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn browser_final_does_not_override_mcp_final() {
    let (_directory, repository) = fixture().await;
    mcp_user(&repository, "mcp-turn", REQUEST, CREATED + 10).await;
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('mcp-final',?,'mcp-turn','assistant','status','mcp-final',?,?)")
        .bind(TASK).bind(json!({"status":"completed","content":"Authoritative MCP result"}).to_string()).bind(CREATED + 20)
        .execute(repository.pool()).await.unwrap();
    persist_observation(
        &repository,
        REQUEST,
        &input(1, "Independent ChatGPT answer", true),
    )
    .await
    .unwrap();
    complete(&repository, "Independent ChatGPT answer").await;
    let final_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE kind='status'")
            .fetch_one(repository.pool())
            .await
            .unwrap();
    assert_eq!(final_count, 1);
    let raw: String =
        sqlx::query_scalar("SELECT assistant_content FROM chatgpt_bridge_requests WHERE id=?")
            .bind(REQUEST)
            .fetch_one(repository.pool())
            .await
            .unwrap();
    assert_eq!(raw, "Independent ChatGPT answer");
    assert_eq!(snapshot(&repository).await["messages"][0]["content"], raw);
}

#[tokio::test]
async fn old_browser_completion_cannot_end_a_newer_identical_question() {
    let (_directory, repository) = fixture().await;
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) SELECT 'new-request',task_id,'new-browser-turn',agent_id,model,user_content,submitted_content,'running',conversation_id,conversation_url,created_at_ms+20,updated_at_ms+20 FROM chatgpt_bridge_requests WHERE id=?")
        .bind(REQUEST).execute(repository.pool()).await.unwrap();
    mcp_user(&repository, "new-mcp-turn", "new-request", CREATED + 30).await;
    complete(&repository, "Old answer").await;
    assert_eq!(status(&repository).await, "running");
    let final_turn: String = sqlx::query_scalar(
        "SELECT turn_id FROM timeline_events WHERE event_id='chatgpt-result-request-think'",
    )
    .fetch_one(repository.pool())
    .await
    .unwrap();
    assert_eq!(final_turn, "browser-turn");
}

#[tokio::test]
async fn rejects_cross_conversation_oversized_invalid_and_duplicate_parts() {
    let (_directory, repository) = fixture().await;
    let mut wrong = input(1, "Text", false);
    wrong.conversation_id = "other-chat".into();
    wrong.conversation_url = "https://chatgpt.com/c/other-chat".into();
    assert!(
        persist_observation(&repository, REQUEST, &wrong)
            .await
            .is_err()
    );
    for invalid in [
        input(0, "Text", false),
        input(2, &"x".repeat(100_001), false),
        input(u64::MAX, "Text", false),
    ] {
        assert!(
            persist_observation(&repository, REQUEST, &invalid)
                .await
                .is_err()
        );
    }
    let mut duplicate = input(1, "Text", false);
    duplicate.messages.push(duplicate.messages[0].clone());
    assert!(
        persist_observation(&repository, REQUEST, &duplicate)
            .await
            .is_err()
    );
    assert_eq!(status(&repository).await, "running");
}

#[tokio::test]
async fn observation_route_works_through_extension_auth_and_replays_idempotently() {
    use super::chatgpt_router_tests::{expect_json, extension_request, fixture};
    let (_state, app, _directory) = fixture("running").await;
    expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/bridge/request-a/started",
            json!({
                "conversationId":"route-conv", "conversationUrl":"https://chatgpt.com/c/route-conv"
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let body = json!({"conversationId":"route-conv", "conversationUrl":"https://chatgpt.com/c/route-conv",
        "revision":123,"completed":false,"messages":[{"id":"public-1","kind":"commentary","content":"Visible text"}]});
    for _ in 0..2 {
        let value = expect_json(
            extension_request(
                &app,
                "POST",
                "/api/local/chatgpt/bridge/request-a/observation",
                body.clone(),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(value["revision"], 123);
    }
    let wrong_method = extension_request(
        &app,
        "GET",
        "/api/local/chatgpt/bridge/request-a/observation",
        Value::Null,
    )
    .await;
    assert_eq!(wrong_method.status(), StatusCode::FORBIDDEN);
    let mut malicious = body;
    malicious["taskId"] = json!("task-b");
    assert_eq!(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/bridge/request-a/observation",
            malicious
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn supports_encoded_provisional_and_project_conversation_urls() {
    let (_directory, repository) = fixture().await;
    sqlx::query("UPDATE chatgpt_bridge_requests SET conversation_id='WEB:test',conversation_url='https://chatgpt.com/g/project/c/WEB%3Atest' WHERE id=?")
        .bind(REQUEST).execute(repository.pool()).await.unwrap();
    let mut observation = input(1, "Visible summary", false);
    observation.conversation_id = "WEB:test".into();
    observation.conversation_url = "https://chatgpt.com/g/project/c/WEB%3Atest".into();
    assert!(
        persist_observation(&repository, REQUEST, &observation)
            .await
            .is_ok()
    );
    observation.revision = 2;
    observation.conversation_url = "https://chatgpt.com/".into();
    assert!(
        persist_observation(&repository, REQUEST, &observation)
            .await
            .is_err()
    );
    observation.conversation_url = "https://chatgpt.com/c/WEB%GGtest".into();
    assert!(
        persist_observation(&repository, REQUEST, &observation)
            .await
            .is_err()
    );
}
