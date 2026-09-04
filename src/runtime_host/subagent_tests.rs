use chatcmd_mcp::catalog_hash;
use chatcmd_runtime::{OperationContext, ShellCreateRequest};
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

#[tokio::test]
async fn inherited_read_grant_is_bounded_and_reserved_from_parent() {
    let (host, context, directory) = parent_fixture().await;
    let child_scope = directory.path().join("src");
    std::fs::create_dir_all(&child_scope).expect("child scope");
    let parent_id = insert_parent_read_grant(&host, &context, directory.path(), 5, 10, 8192).await;
    let grant = read_grant_request(&child_scope);
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Inherited reader", &grant).await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;

    let child = sqlx::query("SELECT inherited_from,child_attempt,allowed_tools_json,path_scopes_json,max_calls,max_files_scanned,max_bytes_read,state FROM approval_grants WHERE task_id=? AND inherited_from=?")
        .bind(&child_task_id)
        .bind(&parent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("child grant");
    assert_eq!(child.get::<String, _>("state"), "active");
    assert_eq!(
        child.get::<Option<String>, _>("inherited_from").as_deref(),
        Some(parent_id.as_str())
    );
    assert_eq!(child.get::<Option<i64>, _>("child_attempt"), Some(1));
    assert_eq!(child.get::<i64, _>("max_calls"), 2);
    assert_eq!(child.get::<Option<i64>, _>("max_files_scanned"), Some(4));
    assert_eq!(child.get::<Option<i64>, _>("max_bytes_read"), Some(4096));
    let tools: Value =
        serde_json::from_str(&child.get::<String, _>("allowed_tools_json")).expect("child tools");
    assert_eq!(tools, json!(["fs_read_text", "fs_stat"]));

    let parent = sqlx::query(
        "SELECT used_calls,used_files_scanned,used_bytes_read FROM approval_grants WHERE id=?",
    )
    .bind(&parent_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("parent grant budget");
    assert_eq!(parent.get::<i64, _>("used_calls"), 2);
    assert_eq!(parent.get::<i64, _>("used_files_scanned"), 4);
    assert_eq!(parent.get::<i64, _>("used_bytes_read"), 4096);

    host.finish_subagent_for_child(&child_task_id, "completed")
        .await
        .expect("finish child");
    let state: String = sqlx::query_scalar(
        "SELECT state FROM approval_grants WHERE task_id=? AND inherited_from=?",
    )
    .bind(&child_task_id)
    .bind(&parent_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("revoked child grant");
    assert_eq!(state, "revoked");
}

#[tokio::test]
async fn inherited_grant_rejects_tool_path_and_budget_escalation() {
    let (host, context, directory) = parent_fixture().await;
    let parent_scope = directory.path().join("src");
    std::fs::create_dir_all(&parent_scope).expect("parent scope");
    insert_parent_read_grant(&host, &context, &parent_scope, 2, 4, 4096).await;

    let mut tool_escalation = read_grant_request(&parent_scope);
    tool_escalation.allowed_tools = vec!["fs_write_text".to_owned()];
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Tool escalation", &tool_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-tool-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("tool escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
    host.finish_subagent_for_child(child_task_id, "failed")
        .await
        .expect("finish denied tool child");

    let mut path_escalation = read_grant_request(directory.path());
    path_escalation.max_calls = 1;
    path_escalation.max_files_scanned = 1;
    path_escalation.max_bytes_read = 1024;
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Path escalation", &path_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-path-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("path escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
    host.finish_subagent_for_child(child_task_id, "failed")
        .await
        .expect("finish denied path child");

    let mut budget_escalation = read_grant_request(&parent_scope);
    budget_escalation.max_calls = 3;
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Budget escalation", &budget_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-budget-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("budget escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
}

#[tokio::test]
async fn concurrent_child_reservations_do_not_oversubscribe_parent() {
    let (host, context, directory) = parent_fixture().await;
    std::fs::create_dir_all(directory.path().join("src")).expect("scope");
    insert_parent_read_grant(&host, &context, directory.path(), 2, 4, 4096).await;
    let grant = read_grant_request(&directory.path().join("src"));
    let (first_registration, first_id) =
        register_with_grant(&host, &context, "Concurrent reader A", &grant).await;
    let (second_registration, second_id) =
        register_with_grant(&host, &context, "Concurrent reader B", &grant).await;
    let first_task = first_registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("first task")
        .to_owned();
    let second_task = second_registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("second task")
        .to_owned();
    let first_prompt = delegated_prompt(&first_id);
    let second_prompt = delegated_prompt(&second_id);
    let first_context = OperationContext::new(
        "claim-concurrent-a",
        &context.agent_id,
        "agent_user_message",
    );
    let second_context = OperationContext::new(
        "claim-concurrent-b",
        &context.agent_id,
        "agent_user_message",
    );
    let (first, second) = tokio::join!(
        host.claim_subagent_from_message(&first_context, &first_task, Some(&first_prompt)),
        host.claim_subagent_from_message(&second_context, &second_task, Some(&second_prompt))
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let denied = if first.is_err() {
        first.expect_err("first denied")
    } else {
        second.expect_err("second denied")
    };
    assert_eq!(denied.code, "approval_grant_inheritance_denied");
    let used_calls: i64 = sqlx::query_scalar(
        "SELECT used_calls FROM approval_grants WHERE task_id=? AND inherited_from IS NULL",
    )
    .bind(PARENT_TASK_ID)
    .fetch_one(host.repository.pool())
    .await
    .expect("parent usage");
    assert_eq!(used_calls, 2);
}

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

#[tokio::test]
async fn fallback_marker_claims_reserved_child_and_parent_wait_finishes_after_completion() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    host.request_subagent_extension_fallback(
        &context,
        &registration,
        &delegated_prompt(&subagent_id),
    )
    .await
    .expect("queue fallback");
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task id")
        .to_owned();

    let child_context = OperationContext::new(
        "subagent-child-request",
        &context.agent_id,
        "agent_user_message",
    );
    host.claim_subagent_from_message(
        &child_context,
        &child_task_id,
        Some(&delegated_prompt(&subagent_id)),
    )
    .await
    .expect("claim fallback child");

    let claimed =
        sqlx::query("SELECT child_task_id,status,fallback_state FROM subagent_runs WHERE id=?")
            .bind(&subagent_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read claimed fallback");
    assert_eq!(
        claimed.get::<Option<String>, _>("child_task_id").as_deref(),
        Some(child_task_id.as_str())
    );
    assert_eq!(claimed.get::<String, _>("status"), "running");
    assert_eq!(claimed.get::<String, _>("fallback_state"), "claimed");

    let running = host
        .wait_for_subagents(&context, 250)
        .await
        .expect("wait running fallback");
    assert_eq!(running.get("runningCount"), Some(&Value::from(1)));
    assert_eq!(running.get("allFinished"), Some(&Value::Bool(false)));

    host.finish_subagent_for_child(&child_task_id, "completed")
        .await
        .expect("complete child");
    let finished = host
        .wait_for_subagents(&context, 250)
        .await
        .expect("wait completed fallback");
    assert_eq!(finished.get("activeCount"), Some(&Value::from(0)));
    assert_eq!(finished.get("allFinished"), Some(&Value::Bool(true)));
    assert_eq!(finished.get("allCompleted"), Some(&Value::Bool(true)));
}

#[tokio::test]
async fn fallback_claim_cannot_overwrite_browser_terminal_state() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    host.request_subagent_extension_fallback(
        &context,
        &registration,
        &delegated_prompt(&subagent_id),
    )
    .await
    .expect("queue fallback");
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task id")
        .to_owned();
    let now = now_ms();
    sqlx::query("UPDATE subagent_runs SET status='completed',fallback_state='exhausted',completed_at_ms=?,updated_at_ms=? WHERE id=?")
        .bind(now)
        .bind(now)
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("mark browser terminal");

    let child_context = OperationContext::new(
        "late-subagent-child-request",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            &child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("late claim must not reopen terminal child");
    assert_eq!(error.code, "subagent_not_active");

    let stored = sqlx::query("SELECT status,fallback_state FROM subagent_runs WHERE id=?")
        .bind(&subagent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read terminal state");
    assert_eq!(stored.get::<String, _>("status"), "completed");
    assert_eq!(stored.get::<String, _>("fallback_state"), "exhausted");
}

#[tokio::test]
async fn expired_running_lease_times_out_and_unblocks_parent() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    sqlx::query("UPDATE subagent_runs SET lease_expires_at_ms=? WHERE id=?")
        .bind(now_ms().saturating_sub(1))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("expire lease");

    let (first, second) = tokio::join!(
        host.expire_stale_subagents(Some((PARENT_TASK_ID, PARENT_TURN_ID))),
        host.expire_stale_subagents(Some((PARENT_TASK_ID, PARENT_TURN_ID)))
    );
    assert_eq!(
        first.expect("first watchdog") + second.expect("second watchdog"),
        1
    );
    let row = sqlx::query("SELECT status,terminal_reason FROM subagent_runs WHERE id=?")
        .bind(&subagent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read timed out run");
    assert_eq!(row.get::<String, _>("status"), "timedOut");
    assert!(
        row.get::<Option<String>, _>("terminal_reason")
            .is_some_and(|reason| reason.contains("lease expired"))
    );
    let task_status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(&child_task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read child task");
    assert_eq!(task_status, "interrupted");
    assert!(
        !host
            .finish_subagent_for_child(&child_task_id, "completed")
            .await
            .expect("stale completion check"),
        "stale worker completion must be rejected"
    );
    host.ensure_subagents_finished(&context)
        .await
        .expect("timed out child must not block parent");
    assert_eq!(host.active_subagent_count().await.expect("active count"), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn watchdog_timeout_force_closes_real_child_pty_process() {
    let (host, context, registration, subagent_id, directory) = fallback_fixture().await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    let mut child_context =
        OperationContext::new("subagent-child-shell", &context.agent_id, "shell_create");
    child_context.task_id = Some(child_task_id.clone());
    child_context.turn_id = Some("turn-subagent-child-shell".to_owned());
    let shell = host
        .shell
        .create(
            &child_context,
            ShellCreateRequest {
                request_id: child_context.request_id.clone(),
                working_directory: Some(directory.path().to_path_buf()),
                executable: Some(std::path::PathBuf::from("/bin/sh")),
                arguments: vec!["-c".to_owned(), "sleep 60".to_owned()],
                environment: std::collections::BTreeMap::new(),
                columns: Some(80),
                rows: Some(24),
            },
        )
        .await
        .expect("create real child shell");
    host.persist_shell_session(&child_context, &shell)
        .await
        .expect("persist child shell");
    let process_id = shell.process_id.expect("child shell pid");

    sqlx::query("UPDATE subagent_runs SET lease_expires_at_ms=? WHERE id=?")
        .bind(now_ms().saturating_sub(1))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("expire lease");
    assert_eq!(
        host.expire_stale_subagents(Some((PARENT_TASK_ID, PARENT_TURN_ID)))
            .await
            .expect("watchdog"),
        1
    );
    let terminal_status: String =
        sqlx::query_scalar("SELECT status FROM terminal_sessions WHERE id=?")
            .bind(&shell.session_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read terminal status");
    assert_eq!(terminal_status, "interrupted");
    let alive = std::process::Command::new("kill")
        .args(["-0", &process_id.to_string()])
        .status()
        .is_ok_and(|status| status.success());
    assert!(!alive, "watchdog must terminate the child PTY process");
}

#[tokio::test]
async fn persisted_deadlines_handle_backward_and_forward_clock_jumps() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let _child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    let base = now_ms();
    sqlx::query("UPDATE subagent_runs SET started_at_ms=?,max_runtime_ms=5000,lease_expires_at_ms=? WHERE id=?")
        .bind(base)
        .bind(base.saturating_add(3_000))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("set deterministic deadlines");

    assert_eq!(
        host.expire_stale_subagents_at(
            Some((PARENT_TASK_ID, PARENT_TURN_ID)),
            base.saturating_sub(60_000),
        )
        .await
        .expect("backward jump"),
        0,
        "backward wall-clock jumps must not expire a live lease early"
    );
    assert_eq!(
        host.expire_stale_subagents_at(
            Some((PARENT_TASK_ID, PARENT_TURN_ID)),
            base.saturating_add(60_000),
        )
        .await
        .expect("forward jump"),
        1,
        "forward jump/resume past persisted deadlines must expire the run"
    );
}

#[tokio::test]
async fn heartbeat_extends_lease_without_crossing_hard_deadline() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    let before: i64 =
        sqlx::query_scalar("SELECT lease_expires_at_ms FROM subagent_runs WHERE id=?")
            .bind(&subagent_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("read lease");
    sqlx::query("UPDATE subagent_runs SET lease_expires_at_ms=? WHERE id=?")
        .bind(now_ms().saturating_add(1_000))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("shorten lease");

    assert!(
        host.heartbeat_subagent(&child_task_id)
            .await
            .expect("heartbeat")
    );
    let after: i64 = sqlx::query_scalar("SELECT lease_expires_at_ms FROM subagent_runs WHERE id=?")
        .bind(&subagent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read renewed lease");
    assert!(after > now_ms().saturating_add(1_000));
    assert!(after <= before.saturating_add(1_000));
}

#[tokio::test]
async fn hard_runtime_and_old_boot_owner_are_reconciled() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    let old = now_ms().saturating_sub(2_000);
    sqlx::query("UPDATE subagent_runs SET started_at_ms=?,max_runtime_ms=1000,lease_expires_at_ms=?,worker_id='old-boot' WHERE id=?")
        .bind(old)
        .bind(now_ms().saturating_add(60_000))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("make stale run");

    assert_eq!(
        host.expire_stale_subagents(None).await.expect("reconcile"),
        1
    );
    assert!(
        !host
            .heartbeat_subagent(&child_task_id)
            .await
            .expect("late heartbeat")
    );
    let row = sqlx::query("SELECT status,terminal_reason FROM subagent_runs WHERE id=?")
        .bind(&subagent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read reconciled run");
    assert_eq!(row.get::<String, _>("status"), "timedOut");
    assert_eq!(
        row.get::<Option<String>, _>("terminal_reason").as_deref(),
        Some("worker process restarted before the child completed")
    );
}

#[tokio::test]
async fn terminal_compare_and_set_has_one_winner() {
    let (host, context, registration, subagent_id, _directory) = fallback_fixture().await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;
    host.finish_subagent_for_child(&child_task_id, "completed")
        .await
        .expect("completion wins");
    sqlx::query("UPDATE subagent_runs SET lease_expires_at_ms=? WHERE id=?")
        .bind(now_ms().saturating_sub(1))
        .bind(&subagent_id)
        .execute(host.repository.pool())
        .await
        .expect("simulate late watchdog scan");

    assert_eq!(
        host.expire_stale_subagents(None).await.expect("watchdog"),
        0
    );
    let status: String = sqlx::query_scalar("SELECT status FROM subagent_runs WHERE id=?")
        .bind(&subagent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read winner");
    assert_eq!(status, "completed");
}
