use super::*;

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
