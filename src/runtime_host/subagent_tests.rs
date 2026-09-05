use chatcmd_mcp::catalog_hash;
use chatcmd_runtime::OperationContext;
use serde_json::{Value, json};
use sqlx::Row as _;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};

use super::{
    RuntimeHost, inputs::SubagentApprovalGrantInput, now_ms, user_message_tests::test_host,
};

const PARENT_TASK_ID: &str = "task-subagent-parent";
const PARENT_TURN_ID: &str = "turn-subagent-parent";

async fn parent_fixture() -> (RuntimeHost, OperationContext, TempDir) {
    let (host, agent_id, directory) = test_host().await;
    let now = now_ms();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,'Sub-agent parent','mcp','running',1,?,?)")
        .bind(PARENT_TASK_ID)
        .bind(&agent_id)
        .bind(host.device.id.as_str())
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert parent task");

    let mut context =
        OperationContext::new("subagent-parent-request", &agent_id, "agent_subagent_start");
    context.task_id = Some(PARENT_TASK_ID.to_owned());
    context.turn_id = Some(PARENT_TURN_ID.to_owned());
    (host, context, directory)
}

async fn fallback_fixture() -> (RuntimeHost, OperationContext, Value, String, TempDir) {
    let (host, context, directory) = parent_fixture().await;
    let registration = host
        .register_subagent(
            &context,
            "Fallback reader",
            "Read the delegated source",
            None,
        )
        .await
        .expect("register subagent");
    let subagent_id = registration
        .get("subagentId")
        .and_then(Value::as_str)
        .expect("subagent id")
        .to_owned();
    (host, context, registration, subagent_id, directory)
}

fn delegated_prompt(subagent_id: &str) -> String {
    format!("Read the delegated source\n\nCMDGPT_SUBAGENT_ID={subagent_id}")
}

async fn claim_registered(
    host: &RuntimeHost,
    context: &OperationContext,
    registration: &Value,
    subagent_id: &str,
) -> String {
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task id")
        .to_owned();
    let child_context = OperationContext::new(
        format!("claim-{subagent_id}"),
        &context.agent_id,
        "agent_user_message",
    );
    host.claim_subagent_from_message(
        &child_context,
        &child_task_id,
        Some(&delegated_prompt(subagent_id)),
    )
    .await
    .expect("claim child");
    child_task_id
}

fn canonical_scope_json(path: &std::path::Path) -> Value {
    let canonical = std::fs::canonicalize(path).expect("canonical grant scope");
    let normalized = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let normalized = normalized.to_ascii_lowercase();
    let metadata = std::fs::metadata(&canonical).expect("grant scope metadata");
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt as _;
        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = {
        let created = metadata
            .created()
            .expect("scope created time")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("scope created after epoch")
            .as_nanos();
        format!("created:{created}:dir:{}", metadata.is_dir())
    };
    json!({"path": normalized, "kind": "subtree", "identity": identity})
}

async fn insert_parent_read_grant(
    host: &RuntimeHost,
    context: &OperationContext,
    scope: &std::path::Path,
    max_calls: i64,
    max_files: i64,
    max_bytes: i64,
) -> String {
    let id = format!("grant-parent-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,turn_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read,expires_at_ms,catalog_hash,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,'active',?,?)")
        .bind(&id)
        .bind(&context.agent_id)
        .bind(PARENT_TASK_ID)
        .bind(PARENT_TURN_ID)
        .bind(json!(["fs_stat","fs_read_text","fs_search"]).to_string())
        .bind(json!([canonical_scope_json(scope)]).to_string())
        .bind(json!({"includeIgnored":false,"includeHidden":false}).to_string())
        .bind(max_calls)
        .bind(max_files)
        .bind(max_bytes)
        .bind(now.saturating_add(600_000))
        .bind(catalog_hash())
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert parent grant");
    id
}

fn read_grant_request(scope: &std::path::Path) -> SubagentApprovalGrantInput {
    SubagentApprovalGrantInput {
        allowed_tools: vec!["fs_stat".to_owned(), "fs_read_text".to_owned()],
        path_scopes: vec![scope.to_string_lossy().into_owned()],
        max_calls: 2,
        max_files_scanned: 4,
        max_bytes_read: 4096,
    }
}

async fn register_with_grant(
    host: &RuntimeHost,
    context: &OperationContext,
    name: &str,
    grant: &SubagentApprovalGrantInput,
) -> (Value, String) {
    let registration = host
        .register_subagent(context, name, "Read delegated files", Some(grant))
        .await
        .expect("register inherited child");
    let subagent_id = registration
        .get("subagentId")
        .and_then(Value::as_str)
        .expect("subagent id")
        .to_owned();
    (registration, subagent_id)
}

mod fallback;
mod grants;
mod lifecycle;

#[tokio::test]
async fn extension_fallback_stays_pending_and_parent_wait_remains_active() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let mut events = host.events.subscribe();
    let fallback = host
        .request_subagent_extension_fallback(
            &context,
            &registration,
            &delegated_prompt(&subagent_id),
        )
        .await
        .expect("queue fallback");
    assert_eq!(fallback.get("attempt"), Some(&Value::from(1)));

    let row =
        sqlx::query("SELECT status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=?")
            .bind(&subagent_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read fallback state");
    assert_eq!(row.get::<String, _>("status"), "pending");
    assert_eq!(row.get::<String, _>("fallback_state"), "requested");
    assert_eq!(row.get::<i64, _>("fallback_attempts"), 1);

    let wait = host
        .wait_for_subagents(&context, 250)
        .await
        .expect("wait pending fallback");
    assert_eq!(wait.get("pendingCount"), Some(&Value::from(1)));
    assert_eq!(wait.get("runningCount"), Some(&Value::from(0)));
    assert_eq!(wait.get("activeCount"), Some(&Value::from(1)));
    assert_eq!(wait.get("allFinished"), Some(&Value::Bool(false)));

    let requested = timeout(Duration::from_secs(1), async {
        loop {
            let event = events.recv().await.expect("fallback event");
            if event.event_type == "subagent.fallback_requested" {
                break event;
            }
        }
    })
    .await
    .expect("fallback event timeout");
    assert_eq!(requested.task_id.as_deref(), Some(PARENT_TASK_ID));
    assert_eq!(requested.turn_id.as_deref(), Some(PARENT_TURN_ID));
    assert_eq!(
        requested
            .payload
            .get("parentTaskId")
            .and_then(Value::as_str),
        Some(PARENT_TASK_ID)
    );
    assert_eq!(
        requested
            .payload
            .get("parentTurnId")
            .and_then(Value::as_str),
        Some(PARENT_TURN_ID)
    );
    assert!(
        requested
            .payload
            .get("submittedContent")
            .and_then(Value::as_str)
            .is_some_and(|value| value
                .starts_with("Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:"))
    );
}
