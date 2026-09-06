mod compact_support;
use chatcmd_storage::{CURRENT_SCHEMA_VERSION, SqliteRepository};
use compact_support::fixture;

#[tokio::test]
async fn compact_continuation_choice_is_default_off_immutable_and_survives_database_reopen() {
    let (repo, dir) = fixture().await;
    let off = repo
        .compact_start("task-a", "off", 2)
        .await
        .expect("default start");
    let on = repo
        .compact_start_with_continuation("task-b", "on", true, 2)
        .await
        .expect("opt in");
    assert!(!off.continue_after_compact);
    assert!(on.continue_after_compact);
    let duplicate = repo
        .compact_start_with_continuation("task-a", "duplicate-on", true, 3)
        .await
        .expect("duplicate start");
    assert_eq!(duplicate.id, off.id);
    assert!(
        !duplicate.continue_after_compact,
        "a retry cannot change the original consent"
    );
    let duplicate = repo
        .compact_start("task-b", "duplicate-off", 3)
        .await
        .expect("default retry");
    assert_eq!(duplicate.id, on.id);
    assert!(duplicate.continue_after_compact);
    assert_eq!(
        serde_json::to_value(&off).expect("JSON")["continueAfterCompact"],
        false
    );
    assert_eq!(
        serde_json::to_value(&on).expect("JSON")["continueAfterCompact"],
        true
    );
    repo.pool().close().await;
    let (repo, _) = SqliteRepository::open(&dir.path().join("compact.db"), 2)
        .await
        .expect("reopen");
    assert!(
        !repo
            .compact_job(&off.id)
            .await
            .expect("off job")
            .continue_after_compact
    );
    assert!(
        repo.compact_job(&on.id)
            .await
            .expect("on job")
            .continue_after_compact
    );
    assert!(
        repo.compact_task_jobs("task-b")
            .await
            .expect("summary")
            .active
            .expect("active")
            .continue_after_compact
    );
    let pending = repo.compact_pending().await.expect("pending");
    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending
            .iter()
            .filter(|job| job.continue_after_compact)
            .count(),
        1
    );
}

#[tokio::test]
async fn schema_23_upgrade_never_opts_in_an_existing_compact_job() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("upgrade-23.db");
    let old = SqliteRepository::connect(&path, 1)
        .await
        .expect("connect old DB");
    let mut migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .expect("migrations");
    migrations.migrations = migrations
        .migrations
        .into_owned()
        .into_iter()
        .filter(|migration| migration.version <= 23)
        .collect::<Vec<_>>()
        .into();
    migrations.run(old.pool()).await.expect("schema 23");
    sqlx::query("INSERT INTO local_device(singleton_id,device_id,installation_id,name,platform,architecture,app_version,created_at_ms,updated_at_ms) VALUES(1,'device','installation','Test','windows','x64','test',1,1)")
        .execute(old.pool()).await.expect("old device");
    sqlx::query("INSERT INTO tasks(id,device_id,title,source,status,created_at_ms,updated_at_ms) VALUES('legacy-task','device','Keep this title','chatgpt_web','interrupted',1,1)")
        .execute(old.pool()).await.expect("old task");
    sqlx::query("INSERT INTO chatgpt_compact_jobs(id,task_id,phase,revision,start_operation_id,old_conversation_id,old_conversation_url,old_model,handoff_text,created_at_ms,updated_at_ms) VALUES('legacy-job','legacy-task','saving_handoff',3,'legacy-start','old-chat','https://chatgpt.com/c/old-chat','Auto','Keep the saved handoff',1,2)")
        .execute(old.pool()).await.expect("old job without opt-in column");
    old.pool().close().await;
    let (upgraded, report) = SqliteRepository::open(&path, 1).await.expect("upgrade");
    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    let job = upgraded
        .compact_job("legacy-job")
        .await
        .expect("retained job");
    assert!(!job.continue_after_compact);
    assert_eq!(job.task_id, "legacy-task");
    assert_eq!(job.revision, 3);
    assert_eq!(job.handoff_text.as_deref(), Some("Keep the saved handoff"));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(upgraded.pool())
        .await
        .expect("tasks");
    assert_eq!(count, 1);
}
