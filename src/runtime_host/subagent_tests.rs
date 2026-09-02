use chatcmd_runtime::OperationContext;
use serde_json::Value;
use sqlx::Row as _;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};

use super::{RuntimeHost, now_ms, user_message_tests::test_host};

const PARENT_TASK_ID: &str = "task-subagent-parent";
const PARENT_TURN_ID: &str = "turn-subagent-parent";

async fn fallback_fixture() -> (RuntimeHost, OperationContext, Value, String, TempDir) {
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
    let registration = host
        .register_subagent(&context, "Fallback reader", "Read the delegated source")
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
            .is_some_and(|value| value.starts_with("Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:"))
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
