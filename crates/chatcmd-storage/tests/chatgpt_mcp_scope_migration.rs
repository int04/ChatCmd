use chatcmd_storage::{CURRENT_SCHEMA_VERSION, SqliteRepository};

#[tokio::test]
async fn schema_25_upgrade_preserves_tasks_and_persists_scoped_aliases() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("scope-upgrade.db");
    let old = SqliteRepository::connect(&path, 1).await.unwrap();
    let mut migrator = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .unwrap();
    migrator.migrations = migrator
        .migrations
        .into_owned()
        .into_iter()
        .filter(|migration| migration.version <= 25)
        .collect::<Vec<_>>()
        .into();
    migrator.run(old.pool()).await.unwrap();
    sqlx::query("INSERT INTO local_device(singleton_id,device_id,installation_id,name,platform,architecture,app_version,created_at_ms,updated_at_ms) VALUES(1,'device','installation','Test','windows','x64','test',1,1)")
        .execute(old.pool()).await.unwrap();
    sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,created_at_ms,updated_at_ms) VALUES('agent','Agent',randomblob(32),'test',1,1,1)")
        .execute(old.pool()).await.unwrap();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES('task','agent','device','Keep this history','chatgpt_web',0,'completed',1,1,1)")
        .execute(old.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('preserve-setting','42',1)",
    )
    .execute(old.pool())
    .await
    .unwrap();
    old.pool().close().await;

    let (repo, report) = SqliteRepository::open(&path, 1).await.unwrap();
    assert_eq!(CURRENT_SCHEMA_VERSION, 26);
    assert_eq!(report.schema_version, 26);
    let original: (String, i64, String) =
        sqlx::query_as("SELECT title,allow_execute,status FROM tasks WHERE id='task'")
            .fetch_one(repo.pool())
            .await
            .unwrap();
    assert_eq!(
        original,
        ("Keep this history".to_owned(), 0, "completed".to_owned())
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_mcp_scope_bindings")
        .fetch_one(repo.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "migration never guesses a legacy task mapping");
    let setting: String =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key='preserve-setting'")
            .fetch_one(repo.pool())
            .await
            .unwrap();
    assert_eq!(setting, "42");
    sqlx::query("INSERT INTO chatgpt_mcp_scope_bindings(agent_id,device_id,scope_hash,task_id,generation,created_at_ms,updated_at_ms) VALUES('agent','device','openai:session','task',1,2,2)")
        .execute(repo.pool()).await.unwrap();
    assert!(sqlx::query("INSERT INTO chatgpt_mcp_scope_bindings(agent_id,device_id,scope_hash,task_id,generation,created_at_ms,updated_at_ms) VALUES('agent','device','openai:session','task',1,3,3)")
        .execute(repo.pool()).await.is_err(), "one scope has exactly one owner");
    repo.pool().close().await;

    let (reopened, _) = SqliteRepository::open(&path, 1).await.unwrap();
    let alias: (String, i64) = sqlx::query_as("SELECT task_id,generation FROM chatgpt_mcp_scope_bindings WHERE agent_id='agent' AND device_id='device' AND scope_hash='openai:session'")
        .fetch_one(reopened.pool()).await.unwrap();
    assert_eq!(alias, ("task".to_owned(), 1));
    sqlx::query("DELETE FROM tasks WHERE id='task'")
        .execute(reopened.pool())
        .await
        .unwrap();
    let aliases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_mcp_scope_bindings")
        .fetch_one(reopened.pool())
        .await
        .unwrap();
    assert_eq!(
        aliases, 0,
        "deleted tasks cannot leave reusable identity aliases"
    );
}
