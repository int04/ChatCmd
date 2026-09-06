use super::*;

async fn limit(host: &RuntimeHost, value: i64) {
    sqlx::query("UPDATE settings SET value_json=? WHERE key='ui_subagentConcurrency'")
        .bind(value.to_string())
        .execute(host.repository.pool())
        .await
        .expect("set limit");
}

#[tokio::test]
async fn default_zero_refuses_delegation_immediately_without_creating_a_child() {
    let (host, context, _dir) = parent_fixture().await;
    sqlx::query("DELETE FROM settings WHERE key='ui_subagentConcurrency'")
        .execute(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(host.subagent_concurrency_limit().await.unwrap(), 0);
    let result = timeout(
        Duration::from_secs(1),
        host.register_subagent(&context, "Disabled", "No child", None),
    )
    .await
    .expect("must not wait for a zero-capacity slot");
    assert_eq!(result.unwrap_err().code, "subagents_disabled");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn selecting_zero_blocks_new_children_but_does_not_cancel_running_work() {
    let (host, context, registration, id, _dir) = fallback_fixture().await;
    let child = claim_registered(&host, &context, &registration, &id).await;
    limit(&host, 0).await;
    claim_registered(&host, &context, &registration, &id).await;
    assert_eq!(
        host.register_subagent(&context, "Disabled", "No child", None)
            .await
            .unwrap_err()
            .code,
        "subagents_disabled"
    );
    assert!(
        host.finish_subagent_for_child(&child, "completed")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn root_wait_includes_grandchild_even_after_direct_child_finishes() {
    let (host, context, registration, id, _dir) = fallback_fixture().await;
    let child = claim_registered(&host, &context, &registration, &id).await;
    let mut nested_context = context.clone();
    nested_context.task_id = Some(child.clone());
    nested_context.turn_id = Some("turn-grandchild".to_owned());
    let nested = host
        .register_subagent(&nested_context, "Grandchild", "Nested work", None)
        .await
        .unwrap();
    let nested_id = nested["subagentId"].as_str().unwrap();
    let grandchild = claim_registered(&host, &nested_context, &nested, nested_id).await;
    host.finish_subagent_for_child(&child, "completed")
        .await
        .unwrap();
    let wait = host.wait_for_subagents(&context, 250).await.unwrap();
    assert_eq!(wait["subagents"].as_array().unwrap().len(), 2);
    assert_eq!(wait["allFinished"], false);
    let row = wait["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["taskId"] == grandchild)
        .unwrap();
    assert_eq!(row["rootTurnId"], PARENT_TURN_ID);
    assert_eq!(row["parentTaskId"], child);
    assert_eq!(row["parentTurnId"], "turn-grandchild");
    assert_eq!(
        host.ensure_subagents_finished(&context)
            .await
            .unwrap_err()
            .code,
        "subagents_still_running"
    );
    host.finish_subagent_for_child(&grandchild, "completed")
        .await
        .unwrap();
    assert_eq!(
        host.wait_for_subagents(&context, 250).await.unwrap()["allCompleted"],
        true
    );
}

#[tokio::test]
async fn nested_full_capacity_fails_fast_instead_of_deadlocking_its_parent() {
    let (host, context, registration, id, _dir) = fallback_fixture().await;
    let child = claim_registered(&host, &context, &registration, &id).await;
    limit(&host, 1).await;
    let mut nested_context = context.clone();
    nested_context.task_id = Some(child);
    nested_context.turn_id = Some("turn-nested-full".to_owned());
    let result = timeout(
        Duration::from_secs(1),
        host.register_subagent(&nested_context, "Nested", "No slot", None),
    )
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, "subagent_capacity_exhausted");
}

#[tokio::test]
async fn renewed_lease_wins_against_a_watchdog_using_an_old_snapshot() {
    let (host, context, registration, id, _dir) = fallback_fixture().await;
    let child = claim_registered(&host, &context, &registration, &id).await;
    let snapshot = sqlx::query("SELECT attempt,worker_id FROM subagent_runs WHERE id=?")
        .bind(&id)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    let now = now_ms();
    sqlx::query("UPDATE subagent_runs SET lease_expires_at_ms=? WHERE id=?")
        .bind(now - 1)
        .bind(&id)
        .execute(host.repository.pool())
        .await
        .unwrap();
    assert!(host.heartbeat_subagent(&child).await.unwrap());
    assert!(
        !host
            .transition_expired_subagent(
                &id,
                snapshot.get("attempt"),
                snapshot.get::<Option<String>, _>("worker_id").as_deref(),
                now_ms(),
                "stale snapshot"
            )
            .await
            .unwrap()
    );
    assert!(
        host.finish_subagent_for_child(&child, "completed")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn global_watchdog_reaps_unclaimed_children_without_a_parent_poll() {
    let (host, _context, _registration, id, _dir) = fallback_fixture().await;
    sqlx::query("UPDATE subagent_runs SET created_at_ms=? WHERE id=?")
        .bind(now_ms() - 70_000)
        .bind(&id)
        .execute(host.repository.pool())
        .await
        .unwrap();
    host.expire_stale_subagents(None).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM subagent_runs WHERE id=?")
        .bind(&id)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn browser_claim_and_mcp_heartbeat_use_browser_lease_but_respect_hard_runtime() {
    for (runtime, minimum) in [(1_800_000_i64, 170_000_i64), (60_000, 55_000)] {
        let (host, context, registration, id, _dir) = fallback_fixture().await;
        sqlx::query("UPDATE subagent_runs SET fallback_state='requested',fallback_attempts=1,max_runtime_ms=? WHERE id=?")
            .bind(runtime).bind(&id).execute(host.repository.pool()).await.unwrap();
        let child = claim_registered(&host, &context, &registration, &id).await;
        host.heartbeat_subagent(&child).await.unwrap();
        let row = sqlx::query(
            "SELECT lease_expires_at_ms,started_at_ms,max_runtime_ms FROM subagent_runs WHERE id=?",
        )
        .bind(&id)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
        let lease = row.get::<i64, _>("lease_expires_at_ms");
        assert!(lease >= now_ms() + minimum);
        assert!(lease <= row.get::<i64, _>("started_at_ms") + runtime);
    }
}
