use super::*;

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
