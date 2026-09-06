mod compact_support;
use chatcmd_storage::{
    CURRENT_SCHEMA_VERSION, SqliteRepository,
    compact::{CompactCheckpoint, CompactPhase, guard_callback, guard_native, openai_scope},
};
use compact_support::*;
use sqlx::Row;

#[tokio::test]
async fn compact_archive_rejects_stale_callbacks_and_identity_writes() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    finish(&repo, &job).await;
    let mut conn = repo.pool().acquire().await.expect("connection");
    assert_conflict(guard_native(&mut conn, "old-chat").await);
    for id in [None, Some("old-chat"), Some("new-chat"), Some("third-chat")] {
        assert_conflict(guard_callback(&mut conn, "request-task-a", id).await);
    }
    guard_native(&mut conn, "new-chat")
        .await
        .expect("new conversation can capture after completion");
    guard_callback(&mut conn, "request-task-b", Some("owned-chat"))
        .await
        .expect("unrelated callback unaffected");
    drop(conn);
    let statements = [
        "UPDATE chatgpt_conversations SET conversation_id='old-chat',conversation_url='https://chatgpt.com/c/old-chat' WHERE task_id='task-a'",
        "UPDATE chatgpt_bridge_requests SET status='running',conversation_id='old-chat' WHERE id='request-task-a'",
        "UPDATE tasks SET generation=7 WHERE id='task-a'",
        "UPDATE tasks SET active_session_id='old-session' WHERE id='task-a'",
    ];
    for sql in statements {
        assert!(
            sqlx::query(sql).execute(repo.pool()).await.is_err(),
            "{sql}"
        );
    }
    assert!(
        sqlx::query("UPDATE tasks SET conversation_scope_hash=? WHERE id='task-a'")
            .bind(openai_scope("old-chat"))
            .execute(repo.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,status,created_at_ms,updated_at_ms) SELECT 'ghost','agent',device_id,?,'running',2,2 FROM local_device")
        .bind(openai_scope("old-chat")).execute(repo.pool()).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool())
        .await
        .expect("task count");
    assert_eq!(count, 2);
    assert!(
        repo.compact_scope_archived(&openai_scope("old-chat"))
            .await
            .expect("scope guard")
    );
}

#[tokio::test]
async fn compact_active_pauses_queue_and_preserves_content_order() {
    let (repo, _dir) = fixture().await;
    let job = repo
        .compact_start("task-a", "start", 2)
        .await
        .expect("start");
    let mut conn = repo.pool().acquire().await.expect("connection");
    assert_conflict(guard_native(&mut conn, "old-chat").await);
    assert_conflict(guard_callback(&mut conn, "request-task-a", None).await);
    drop(conn);
    let row =
        sqlx::query("SELECT mode,content,sort_order FROM chatgpt_message_queue WHERE id='queue-a'")
            .fetch_one(repo.pool())
            .await
            .expect("queue retained");
    assert_eq!(row.get::<String, _>("mode"), "queued");
    assert_eq!(row.get::<String, _>("content"), "do not lose me");
    assert_eq!(row.get::<i64, _>("sort_order"), 100);
    sqlx::query("UPDATE chatgpt_message_queue SET mode='immediate' WHERE id='queue-a'")
        .execute(repo.pool())
        .await
        .expect("coerce paused mode");
    sqlx::query("INSERT INTO chatgpt_message_queue(id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms) VALUES('queue-b','task-a','second','immediate',200,2,2)")
        .execute(repo.pool()).await.expect("preserve new queued content");
    let immediate: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chatgpt_message_queue WHERE task_id='task-a' AND mode='immediate'",
    )
    .fetch_one(repo.pool())
    .await
    .expect("no immediate claims");
    assert_eq!(immediate, 0);
    let dispatch=sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES('new-send','task-a','turn-new','agent','Pro','message','message','queued',2,2)")
        .execute(repo.pool()).await;
    assert!(dispatch.is_err());
    checkpoint(&repo, &job, CompactPhase::Cancelled, None).await;
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_message_queue WHERE task_id='task-a'")
            .fetch_one(repo.pool())
            .await
            .expect("queue survives cancellation");
    assert_eq!(remaining, 2);
}

#[tokio::test]
async fn compact_reserves_destination_and_rejects_rebinding_between_workers() {
    let (repo, _dir) = fixture().await;
    let job = ready(&repo).await;
    let reserve = CompactCheckpoint {
        expected_revision: job.revision,
        new_conversation_id: Some("reserved-chat".to_owned()),
        new_conversation_url: Some("https://chatgpt.com/c/reserved-chat".to_owned()),
        ..Default::default()
    };
    let reserved = repo
        .compact_checkpoint(&job.id, &reserve, "reserve", 20)
        .await
        .expect("reserve destination");
    let mut conn = repo.pool().acquire().await.expect("connection");
    assert_conflict(guard_native(&mut conn, "reserved-chat").await);
    drop(conn);
    let other = repo
        .compact_start("task-b", "start-other", 20)
        .await
        .expect("other job");
    assert_conflict(
        repo.compact_checkpoint(
            &other.id,
            &CompactCheckpoint {
                expected_revision: 0,
                ..reserve.clone()
            },
            "reserve-other",
            21,
        )
        .await,
    );
    assert_conflict(
        repo.compact_checkpoint(
            &reserved.id,
            &CompactCheckpoint {
                expected_revision: reserved.revision,
                new_conversation_id: Some("changed-chat".to_owned()),
                new_conversation_url: Some("https://chatgpt.com/c/changed-chat".to_owned()),
                ..Default::default()
            },
            "change-reservation",
            22,
        )
        .await,
    );
    assert!(
        sqlx::query(
            "UPDATE chatgpt_conversations SET conversation_id='changed-old' WHERE task_id='task-a'"
        )
        .execute(repo.pool())
        .await
        .is_err()
    );
}

#[tokio::test]
async fn compact_migration_upgrades_schema_22_and_registry_matches() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("upgrade-22.db");
    let old = SqliteRepository::connect(&path, 1)
        .await
        .expect("old connection");
    let mut migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .expect("load migrations");
    migrations.migrations = migrations
        .migrations
        .into_owned()
        .into_iter()
        .filter(|m| m.version <= 22)
        .collect::<Vec<_>>()
        .into();
    migrations.run(old.pool()).await.expect("install schema 22");
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('preserve-marker','42',1)",
    )
    .execute(old.pool())
    .await
    .expect("marker");
    old.pool().close().await;
    let (upgraded, report) = SqliteRepository::open(&path, 1).await.expect("upgrade");
    assert_eq!(CURRENT_SCHEMA_VERSION, 24);
    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(upgraded.pool())
        .await
        .expect("registry");
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let metadata: String =
        sqlx::query_scalar("SELECT value FROM app_metadata WHERE key='schema_version'")
            .fetch_one(upgraded.pool())
            .await
            .expect("metadata");
    assert_eq!(metadata, CURRENT_SCHEMA_VERSION.to_string());
    let marker: String =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key='preserve-marker'")
            .fetch_one(upgraded.pool())
            .await
            .expect("marker retained");
    assert_eq!(marker, "42");
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_compact_jobs")
        .fetch_one(upgraded.pool())
        .await
        .expect("no invented jobs");
    assert_eq!(jobs, 0);
}
