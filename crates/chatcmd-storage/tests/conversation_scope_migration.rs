use chatcmd_storage::{CURRENT_SCHEMA_VERSION, SqliteRepository};

#[tokio::test]
async fn schema_24_upgrade_repairs_split_chatgpt_scope_and_enforces_one_top_level_owner() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let path = directory.path().join("scope-upgrade.db");
    let old = SqliteRepository::connect(&path, 1)
        .await
        .expect("connect schema 24 database");
    let mut migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .expect("load migrations");
    migrations.migrations = migrations
        .migrations
        .into_owned()
        .into_iter()
        .filter(|migration| migration.version <= 24)
        .collect::<Vec<_>>()
        .into();
    migrations.run(old.pool()).await.expect("schema 24");

    sqlx::query("INSERT INTO local_device(singleton_id,device_id,installation_id,name,platform,architecture,app_version,created_at_ms,updated_at_ms) VALUES(1,'device','installation','Test','windows','x64','test',1,1)")
        .execute(old.pool())
        .await
        .expect("device");
    sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,created_at_ms,updated_at_ms) VALUES('agent','Agent',randomblob(32),'test',1,1,1)")
        .execute(old.pool())
        .await
        .expect("agent");
    for (id, source, created_at_ms) in [
        ("task-web", "chatgpt_web", 1_i64),
        ("task-split", "mcp", 2_i64),
    ] {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,created_at_ms,updated_at_ms) VALUES(?,'agent','device','openai:same-scope',?,?, 'running',?,?)")
            .bind(id)
            .bind(id)
            .bind(source)
            .bind(created_at_ms)
            .bind(created_at_ms)
            .execute(old.pool())
            .await
            .expect("legacy duplicate task");
    }
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,created_at_ms,updated_at_ms) VALUES('task-web','chatgpt-conversation','https://chatgpt.com/c/chatgpt-conversation','Auto',1,1)")
        .execute(old.pool())
        .await
        .expect("browser conversation binding");
    old.pool().close().await;

    let (upgraded, report) = SqliteRepository::open(&path, 1).await.expect("upgrade");
    assert_eq!(CURRENT_SCHEMA_VERSION, 26);
    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);

    let web_scope: Option<String> =
        sqlx::query_scalar("SELECT conversation_scope_hash FROM tasks WHERE id='task-web'")
            .fetch_one(upgraded.pool())
            .await
            .expect("web task scope");
    let split_scope: Option<String> =
        sqlx::query_scalar("SELECT conversation_scope_hash FROM tasks WHERE id='task-split'")
            .fetch_one(upgraded.pool())
            .await
            .expect("split task scope");
    assert_eq!(web_scope.as_deref(), Some("openai:same-scope"));
    assert_eq!(split_scope, None);

    let retained: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id IN ('task-web','task-split')")
            .fetch_one(upgraded.pool())
            .await
            .expect("retained task history");
    assert_eq!(retained, 2);

    let duplicate = sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,created_at_ms,updated_at_ms) VALUES('task-duplicate','agent','device','openai:same-scope','duplicate','mcp','running',3,3)")
        .execute(upgraded.pool())
        .await;
    assert!(
        duplicate.is_err(),
        "top-level scope must have only one owner"
    );
}
