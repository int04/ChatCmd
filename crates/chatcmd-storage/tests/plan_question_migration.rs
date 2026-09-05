use chatcmd_storage::{CURRENT_SCHEMA_VERSION, SqliteRepository};
use tempfile::TempDir;

#[test]
fn checked_in_migrations_use_stable_lf_line_endings() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(directory).expect("migration directory") {
        let path = entry.expect("migration entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("sql") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read migration");
        assert!(
            !bytes.windows(2).any(|window| window == b"\r\n"),
            "{} contains CRLF bytes, which changes the SQLx migration checksum",
            path.display()
        );
    }
}

#[tokio::test]
async fn schema_twenty_upgrades_additively_without_inventing_consent() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("upgrade.db");
    let (repository, _) = SqliteRepository::open(&path, 1)
        .await
        .expect("bootstrap current schema");
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('migration-marker','42',1)",
    )
    .execute(repository.pool())
    .await
    .expect("legacy marker");
    sqlx::query("DROP TABLE plan_questions")
        .execute(repository.pool())
        .await
        .expect("simulate schema twenty");
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=21")
        .execute(repository.pool())
        .await
        .expect("remove migration marker");
    sqlx::query("UPDATE schema_version SET version=20 WHERE singleton_id=1")
        .execute(repository.pool())
        .await
        .expect("restore schema version");
    repository.pool().close().await;

    let (upgraded, report) = SqliteRepository::open(&path, 1)
        .await
        .expect("upgrade schema");
    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    let marker: String =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key='migration-marker'")
            .fetch_one(upgraded.pool())
            .await
            .expect("legacy data retained");
    assert_eq!(marker, "42");
    let questions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plan_questions")
        .fetch_one(upgraded.pool())
        .await
        .expect("plan question table");
    assert_eq!(questions, 0, "migration must not invent legacy consent");
}
