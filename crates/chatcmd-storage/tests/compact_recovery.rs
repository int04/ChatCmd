mod compact_support;
use chatcmd_storage::compact::{CompactCheckpoint, CompactPhase};
use compact_support::*;

#[tokio::test]
async fn compact_operation_collision_rolls_back_checkpoint_before_rebinding() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    assert_conflict(
        repo.compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: job.revision,
                phase: Some(CompactPhase::Completed),
                new_conversation_id: Some("new-chat".to_owned()),
                new_conversation_url: Some("https://chatgpt.com/c/new-chat".to_owned()),
                ..Default::default()
            },
            "start-a",
            20,
        )
        .await,
    );
    let unchanged = repo.compact_job(&job.id).await.expect("rollback job");
    assert_eq!(unchanged.revision, job.revision);
    assert_eq!(unchanged.phase, CompactPhase::OpeningNewChat);
    assert!(unchanged.new_conversation_id.is_none());
    let id: String = sqlx::query_scalar(
        "SELECT conversation_id FROM chatgpt_conversations WHERE task_id='task-a'",
    )
    .fetch_one(repo.pool())
    .await
    .expect("rollback binding");
    assert_eq!(id, "old-chat");
    let archives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_compact_archives")
        .fetch_one(repo.pool())
        .await
        .expect("no archive");
    assert_eq!(archives, 0);
    finish(&repo, &job).await;
}

#[tokio::test]
async fn compact_cancellation_remains_available_after_destination_ownership_changes() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    let reserved = repo
        .compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: job.revision,
                new_conversation_id: Some("new-chat".to_owned()),
                new_conversation_url: Some("https://chatgpt.com/c/new-chat".to_owned()),
                ..Default::default()
            },
            "reserve",
            20,
        )
        .await
        .expect("reserve");
    // A competing pre-existing bridge writer can still own its task; completion
    // must reject that owner, but a user cancellation must never get stuck.
    sqlx::query("UPDATE chatgpt_conversations SET conversation_id='new-chat',conversation_url='https://chatgpt.com/c/new-chat' WHERE task_id='task-b'")
        .execute(repo.pool()).await.expect("competing binding");
    assert_conflict(
        repo.compact_checkpoint(
            &reserved.id,
            &CompactCheckpoint {
                expected_revision: reserved.revision,
                phase: Some(CompactPhase::Completed),
                ..Default::default()
            },
            "conflicting-completion",
            21,
        )
        .await,
    );
    let cancelled = checkpoint(&repo, &reserved, CompactPhase::Cancelled, None).await;
    assert_eq!(cancelled.phase, CompactPhase::Cancelled);
    assert!(
        repo.compact_pending()
            .await
            .expect("no stuck worker")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_repeated_compaction_keeps_same_task_and_all_archived_guards() {
    let (repo, _dir) = fixture().await;
    let first = ready(&repo).await;
    let first = finish(&repo, &first).await;
    let second = repo
        .compact_start("task-a", "second-start", 30)
        .await
        .expect("second start");
    assert_eq!(second.old_conversation_id, "new-chat");
    let second = checkpoint(
        &repo,
        &second,
        CompactPhase::SavingHandoff,
        Some("second handoff"),
    )
    .await;
    let second = checkpoint(&repo, &second, CompactPhase::OpeningNewChat, None).await;
    let second = repo
        .compact_checkpoint(
            &second.id,
            &CompactCheckpoint {
                expected_revision: second.revision,
                phase: Some(CompactPhase::Completed),
                new_conversation_id: Some("third-chat".to_owned()),
                new_conversation_url: Some("https://chatgpt.com/c/third-chat".to_owned()),
                ..Default::default()
            },
            "second-complete",
            40,
        )
        .await
        .expect("second complete");
    let list = repo.compact_task_jobs("task-a").await.expect("history");
    assert!(list.active.is_none());
    assert_eq!(list.history.len(), 2);
    assert_eq!(list.history[0].id, second.id);
    assert_eq!(list.history[1].id, first.id);
    assert!(list.history.iter().all(|job| job.handoff_text.is_none()));
    let mut conn = repo.pool().acquire().await.expect("connection");
    for id in ["old-chat", "new-chat"] {
        assert_conflict(chatcmd_storage::compact::guard_native(&mut conn, id).await);
    }
    drop(conn);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool())
        .await
        .expect("tasks");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn compact_terminal_allows_same_phase_detail_but_not_cancellation_or_rebind() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    let done = finish(&repo, &job).await;
    let input = CompactCheckpoint {
        expected_revision: done.revision,
        phase: Some(CompactPhase::Completed),
        detail: Some(Some("worker recovered".to_owned())),
        ..Default::default()
    };
    let updated = repo
        .compact_checkpoint(&done.id, &input, "terminal-detail", 100)
        .await
        .expect("same-phase detail");
    assert_eq!(updated.phase, CompactPhase::Completed);
    assert_eq!(updated.revision, done.revision + 1);
    assert_eq!(updated.completed_at_ms, done.completed_at_ms);
    assert_eq!(updated.new_conversation_id, done.new_conversation_id);
    assert_conflict(
        repo.compact_checkpoint(&done.id, &input, "terminal-detail", 101)
            .await,
    );
    assert_conflict(
        repo.compact_checkpoint(
            &done.id,
            &CompactCheckpoint {
                expected_revision: updated.revision,
                phase: Some(CompactPhase::Cancelled),
                detail: Some(None),
                ..Default::default()
            },
            "cancel-completed",
            102,
        )
        .await,
    );
}
