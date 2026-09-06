mod compact_support;
use chatcmd_core::StorageError;
use chatcmd_storage::{
    SqliteRepository,
    compact::{CompactCheckpoint, CompactPhase, MAX_HANDOFF_BYTES, openai_scope},
};
use compact_support::*;
use serde_json::{Value, json};
use sqlx::Row;

#[tokio::test]
async fn compact_start_is_idempotent_concurrent_and_snapshots_running_request() {
    let (repo, _dir) = fixture().await;
    let (a, b) = tokio::join!(
        repo.compact_start("task-a", "start-a", 2),
        repo.compact_start("task-a", "start-b", 2)
    );
    let a = a.expect("first start");
    let b = b.expect("second start");
    assert_eq!(a.id, b.id);
    assert_eq!(a.revision, 0);
    assert_eq!(a.old_model, "Pro");
    assert_eq!(a.old_request_id.as_deref(), Some("request-task-a"));
    assert_eq!(a.old_scope_hash, Some(openai_scope("old-chat")));
    assert_eq!(a.agent_name.as_deref(), Some("Compact Agent"));
    assert_eq!(a.project_folder.as_deref(), Some("D:\\project"));
    let snapshot: String =
        sqlx::query_scalar("SELECT old_request_params_json FROM chatgpt_compact_jobs WHERE id=?")
            .bind(&a.id)
            .fetch_one(repo.pool())
            .await
            .expect("snapshot");
    let snapshot: Value = serde_json::from_str(&snapshot).expect("JSON snapshot");
    assert_eq!(snapshot["submittedContent"], "wrapped question");
    assert_eq!(snapshot["status"], "running");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_compact_jobs WHERE task_id='task-a'")
            .fetch_one(repo.pool())
            .await
            .expect("count");
    assert_eq!(count, 1);
    let duplicate = sqlx::query("INSERT INTO chatgpt_compact_jobs(id,task_id,phase,start_operation_id,old_conversation_id,old_conversation_url,old_model,created_at_ms,updated_at_ms) VALUES('duplicate','task-a','preparing','direct','old-chat','https://chatgpt.com/c/old-chat','Pro',3,3)")
        .execute(repo.pool()).await;
    assert!(
        duplicate.is_err(),
        "partial unique index enforces single active job"
    );
    let winning_operation: String =
        sqlx::query_scalar("SELECT start_operation_id FROM chatgpt_compact_jobs WHERE id=?")
            .bind(&a.id)
            .fetch_one(repo.pool())
            .await
            .expect("winning operation");
    assert_conflict(repo.compact_start("task-b", &winning_operation, 3).await);
}

#[tokio::test]
async fn compact_cas_replay_monotonic_phases_and_explicit_null_detail() {
    let (repo, _dir) = fixture().await;
    let job = repo
        .compact_start("task-a", "start", 2)
        .await
        .expect("start");
    let input: CompactCheckpoint = serde_json::from_value(
        json!({"expectedRevision":0,"phase":"writing_handoff","detail":"working"}),
    )
    .expect("input");
    let next = repo
        .compact_checkpoint(&job.id, &input, "write", 3)
        .await
        .expect("advance");
    assert_eq!(next.revision, 1);
    assert_eq!(next.detail.as_deref(), Some("working"));
    assert_conflict(repo.compact_checkpoint(&job.id, &input, "write", 4).await);
    assert_eq!(repo.compact_job(&job.id).await.expect("reload").revision, 1);
    let clear: CompactCheckpoint =
        serde_json::from_value(json!({"expectedRevision":1,"detail":null})).expect("clear detail");
    let next = repo
        .compact_checkpoint(&job.id, &clear, "clear", 4)
        .await
        .expect("same phase");
    assert_eq!(next.phase, CompactPhase::WritingHandoff);
    assert!(next.detail.is_none());
    assert_conflict(
        repo.compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: 2,
                phase: Some(CompactPhase::Preparing),
                ..Default::default()
            },
            "backward",
            5,
        )
        .await,
    );
    let cancelled = checkpoint(&repo, &next, CompactPhase::Cancelled, None).await;
    assert_eq!(cancelled.phase, CompactPhase::Cancelled);
    assert!(cancelled.completed_at_ms.is_some());
    assert_conflict(
        repo.compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: cancelled.revision,
                ..Default::default()
            },
            "after-terminal",
            50,
        )
        .await,
    );
    let next_job = repo
        .compact_start("task-a", "second-start", 100)
        .await
        .expect("restart after cancel");
    assert_ne!(next_job.id, job.id);
    let list = repo.compact_task_jobs("task-a").await.expect("list");
    assert_eq!(list.active.expect("active").id, next_job.id);
    assert_eq!(list.history.len(), 1);
    assert!(
        serde_json::from_value::<CompactCheckpoint>(json!({"expectedRevision":0,"phase":"failed"}))
            .is_err()
    );
}

#[tokio::test]
async fn compact_requires_prior_durable_handoff_and_preserves_same_task() {
    let (repo, _dir) = fixture().await;
    let job = repo
        .compact_start("task-a", "start-a", 2)
        .await
        .expect("start");
    for phase in [CompactPhase::OpeningNewChat, CompactPhase::Completed] {
        assert_conflict(
            repo.compact_checkpoint(
                &job.id,
                &CompactCheckpoint {
                    expected_revision: 0,
                    phase: Some(phase),
                    handoff_text: Some("not yet durable".to_owned()),
                    new_conversation_id: Some("new-chat".to_owned()),
                    new_conversation_url: Some("https://chatgpt.com/c/new-chat".to_owned()),
                    ..Default::default()
                },
                "premature",
                3,
            )
            .await,
        );
    }
    let job = ready(&repo).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool())
        .await
        .expect("before");
    let done = finish(&repo, &job).await;
    assert_eq!(done.task_id, "task-a");
    assert_eq!(done.phase, CompactPhase::Completed);
    assert_eq!(done.handoff_text.as_deref(), Some("durable handoff"));
    let row=sqlx::query("SELECT t.*,c.conversation_id,c.conversation_url,c.active_request_id FROM tasks t JOIN chatgpt_conversations c ON c.task_id=t.id WHERE t.id='task-a'")
        .fetch_one(repo.pool()).await.expect("preserved task");
    assert_eq!(row.get::<String, _>("title"), "Preserve my title");
    assert_eq!(row.get::<String, _>("project_folder"), "D:\\project");
    assert_eq!(row.get::<String, _>("agent_id"), "agent");
    assert_eq!(row.get::<i64, _>("allow_execute"), 0);
    assert_eq!(row.get::<String, _>("conversation_id"), "new-chat");
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        openai_scope("new-chat")
    );
    assert!(row.get::<Option<String>, _>("active_request_id").is_none());
    assert!(row.get::<Option<String>, _>("active_session_id").is_none());
    assert_eq!(row.get::<i64, _>("generation"), 8);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool())
        .await
        .expect("after");
    assert_eq!(before, after);
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE event_id='keep-history'")
            .fetch_one(repo.pool())
            .await
            .expect("history");
    assert_eq!(history, 1);
    let permissions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_allowed_tools WHERE agent_id='agent'")
            .fetch_one(repo.pool())
            .await
            .expect("permissions");
    assert_eq!(permissions, 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM chatgpt_bridge_requests WHERE id='request-task-a'")
            .fetch_one(repo.pool())
            .await
            .expect("request status");
    assert_eq!(status, "stopped");
    let list = repo
        .compact_task_jobs("task-a")
        .await
        .expect("history list");
    assert!(list.active.is_none());
    assert_eq!(list.history.len(), 1);
    assert!(list.history[0].handoff_text.is_none());
    assert!(
        !serde_json::to_value(done)
            .expect("serialize")
            .as_object()
            .expect("object")
            .contains_key("oldRequestParamsJson")
    );
}

#[tokio::test]
async fn compact_validates_destinations_and_rejects_cross_task_ownership() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    for (id, url) in [
        ("WEB:temporary", "https://chatgpt.com/c/WEB:temporary"),
        ("new-chat", "https://chatgpt.com.evil/c/new-chat"),
        ("new-chat", "http://chatgpt.com/c/new-chat"),
        ("new-chat", "https://chatgpt.com/c/wrong"),
        ("new-chat", "https://chatgpt.com/share/new-chat"),
        ("new-chat", "https://chatgpt.com:443/c/new-chat"),
        ("new-chat", "https://chatgpt.com/c/%6eew-chat"),
        ("old-chat", "https://chatgpt.com/c/old-chat"),
        ("owned-chat", "https://chatgpt.com/c/owned-chat"),
    ] {
        let result = repo
            .compact_checkpoint(
                &job.id,
                &CompactCheckpoint {
                    expected_revision: job.revision,
                    phase: Some(CompactPhase::Completed),
                    new_conversation_id: Some(id.to_owned()),
                    new_conversation_url: Some(url.to_owned()),
                    ..Default::default()
                },
                "invalid-destination",
                20,
            )
            .await;
        assert!(
            matches!(
                result,
                Err(StorageError::Conflict(_) | StorageError::InvalidData(_))
            ),
            "{id} {result:?}"
        );
        assert_eq!(
            repo.compact_job(&job.id).await.expect("unchanged").revision,
            job.revision
        );
    }
    sqlx::query("UPDATE tasks SET conversation_scope_hash=? WHERE id='task-b'")
        .bind(openai_scope("scope-owned"))
        .execute(repo.pool())
        .await
        .expect("scope owner");
    assert_conflict(
        repo.compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: job.revision,
                new_conversation_id: Some("scope-owned".to_owned()),
                new_conversation_url: Some("https://chatgpt.com/c/scope-owned".to_owned()),
                ..Default::default()
            },
            "scope-conflict",
            20,
        )
        .await,
    );
}

#[tokio::test]
async fn compact_reopens_with_handoff_then_completes_without_a_new_task() {
    let (repo, dir) = fixture().await;
    let job = ready(&repo).await;
    repo.pool().close().await;
    let (reopened, _) = SqliteRepository::open(&dir.path().join("compact.db"), 2)
        .await
        .expect("reopen");
    let saved = reopened.compact_job(&job.id).await.expect("recover");
    assert_eq!(saved.revision, job.revision);
    assert_eq!(saved.handoff_text, job.handoff_text);
    let pending = reopened.compact_pending().await.expect("pending");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].handoff_text.is_none());
    let done = finish(&reopened, &saved).await;
    reopened.pool().close().await;
    let (again, _) = SqliteRepository::open(&dir.path().join("compact.db"), 1)
        .await
        .expect("second reopen");
    assert_eq!(
        again
            .compact_job(&done.id)
            .await
            .expect("durable completion")
            .phase,
        CompactPhase::Completed
    );
    assert!(
        again
            .compact_pending()
            .await
            .expect("pending empty")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_payload_bounds_and_competing_cas() {
    let (repo, _dir) = fixture().await;
    let job = repo
        .compact_start("task-a", "start", 2)
        .await
        .expect("start");
    let input = CompactCheckpoint {
        expected_revision: 0,
        detail: Some(Some("bounded".to_owned())),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        repo.compact_checkpoint(&job.id, &input, "cas-a", 3),
        repo.compact_checkpoint(&job.id, &input, "cas-b", 3)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let bad = CompactCheckpoint {
        expected_revision: 1,
        handoff_text: Some("x".repeat(MAX_HANDOFF_BYTES + 1)),
        ..Default::default()
    };
    assert!(matches!(
        repo.compact_checkpoint(&job.id, &bad, "large", 4).await,
        Err(StorageError::InvalidData(_))
    ));
    assert_eq!(
        repo.compact_job(&job.id).await.expect("revision").revision,
        1
    );
}
