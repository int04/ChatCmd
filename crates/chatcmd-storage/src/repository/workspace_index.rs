use super::*;
use sha2::{Digest as _, Sha256};

const MAX_INDEX_ENTRIES: usize = 1_000_000;
const MAX_INDEX_METADATA_BYTES: usize = 512 * 1024 * 1024;
const MAX_INDEX_SQLITE_GROWTH_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedWorkspaceIndexEntry {
    pub relative_path_bytes: Vec<u8>,
    pub display_path: String,
    pub entry_type: String,
    pub size_bytes: u64,
    pub modified_at_ns: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedWorkspaceIndex {
    pub root_path: String,
    pub schema_version: u32,
    pub generation: u64,
    pub state: String,
    pub indexed_bytes: u64,
    pub last_error: Option<String>,
    pub entries: Vec<PersistedWorkspaceIndexEntry>,
}

impl SqliteRepository {
    pub async fn replace_workspace_index(
        &self,
        index: &PersistedWorkspaceIndex,
    ) -> Result<(), StorageError> {
        self.replace_workspace_index_with_growth_limit(index, MAX_INDEX_SQLITE_GROWTH_BYTES)
            .await
    }

    async fn replace_workspace_index_with_growth_limit(
        &self,
        index: &PersistedWorkspaceIndex,
        max_sqlite_growth_bytes: u64,
    ) -> Result<(), StorageError> {
        validate_index(index)?;
        let workspace_id = workspace_id(&index.root_path);
        let now = now_ms()?;
        let generation = i64::try_from(index.generation)
            .map_err(|error| backend("convert repository index generation", error))?;
        let indexed_bytes = i64::try_from(index.indexed_bytes)
            .map_err(|error| backend("convert repository indexed bytes", error))?;
        let entry_count = i64::try_from(index.entries.len())
            .map_err(|error| backend("convert repository index entry count", error))?;
        let schema_version = i64::from(index.schema_version);
        let page_count_before = sqlite_pragma_u64(&self.pool, "PRAGMA page_count").await?;
        let page_size = sqlite_pragma_u64(&self.pool, "PRAGMA page_size").await?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin repository index replacement", error))?;

        sqlx::query(
            "INSERT INTO workspace_repository_indexes(workspace_id,root_path,schema_version,generation,state,ignore_fingerprint,entry_count,indexed_bytes,last_reconciled_at_ms,last_error,updated_at_ms) VALUES(?,?,?,?,?,'default-v1',?,?,?, ?,?) ON CONFLICT(workspace_id) DO UPDATE SET root_path=excluded.root_path,schema_version=excluded.schema_version,generation=excluded.generation,state=excluded.state,ignore_fingerprint=excluded.ignore_fingerprint,entry_count=excluded.entry_count,indexed_bytes=excluded.indexed_bytes,last_reconciled_at_ms=excluded.last_reconciled_at_ms,last_error=excluded.last_error,updated_at_ms=excluded.updated_at_ms",
        )
        .bind(&workspace_id)
        .bind(&index.root_path)
        .bind(schema_version)
        .bind(generation)
        .bind(&index.state)
        .bind(entry_count)
        .bind(indexed_bytes)
        .bind(now)
        .bind(&index.last_error)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| backend("upsert repository index root", error))?;

        sqlx::query("DELETE FROM workspace_repository_index_entries WHERE workspace_id=?")
            .bind(&workspace_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("clear repository index entries", error))?;

        for entry in &index.entries {
            let size_bytes = i64::try_from(entry.size_bytes)
                .map_err(|error| backend("convert repository entry size", error))?;
            sqlx::query(
                "INSERT INTO workspace_repository_index_entries(workspace_id,relative_path_bytes,display_path,normalized_path,normalized_extension,entry_type,size_bytes,modified_at_ns,file_identity,version_token_metadata,ignored_state,last_seen_generation) VALUES(?,?,?,?,?,?,?,?,NULL,NULL,0,?)",
            )
            .bind(&workspace_id)
            .bind(&entry.relative_path_bytes)
            .bind(&entry.display_path)
            .bind(normalized_path(&entry.display_path))
            .bind(normalized_extension(&entry.display_path))
            .bind(&entry.entry_type)
            .bind(size_bytes)
            .bind(entry.modified_at_ns.to_string())
            .bind(generation)
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("insert repository index entry", error))?;
        }

        let page_count_after = sqlx::query_scalar::<_, i64>("PRAGMA page_count")
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| backend("read repository index page count", error))?;
        let page_count_after = u64::try_from(page_count_after)
            .map_err(|error| backend("convert repository index page count", error))?;
        let growth_bytes = page_count_after
            .saturating_sub(page_count_before)
            .saturating_mul(page_size);
        if growth_bytes > max_sqlite_growth_bytes {
            return Err(StorageError::InvalidData(format!(
                "repository index SQLite growth {growth_bytes} exceeds hard quota {max_sqlite_growth_bytes} bytes"
            )));
        }

        transaction
            .commit()
            .await
            .map_err(|error| backend("commit repository index replacement", error))?;

        // Keep WAL growth bounded without forcing a blocking truncate or VACUUM on every rebuild.
        // PASSIVE checkpoints whatever readers allow and leaves correctness independent of success.
        let _ = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&self.pool)
            .await;
        Ok(())
    }

    pub async fn load_workspace_index(
        &self,
        root_path: &str,
    ) -> Result<Option<PersistedWorkspaceIndex>, StorageError> {
        let Some(root) = sqlx::query("SELECT workspace_id,schema_version,generation,state,indexed_bytes,last_error FROM workspace_repository_indexes WHERE root_path=?")
            .bind(root_path)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| backend("read repository index root", error))?
        else {
            return Ok(None);
        };
        let workspace_id: String = root
            .try_get("workspace_id")
            .map_err(|error| backend("decode repository workspace id", error))?;
        let rows = sqlx::query("SELECT relative_path_bytes,display_path,entry_type,size_bytes,modified_at_ns FROM workspace_repository_index_entries WHERE workspace_id=? ORDER BY normalized_path,relative_path_bytes")
            .bind(&workspace_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("read repository index entries", error))?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let modified_at_ns: Option<String> = row
                .try_get("modified_at_ns")
                .map_err(|error| backend("decode repository modified time", error))?;
            entries.push(PersistedWorkspaceIndexEntry {
                relative_path_bytes: row
                    .try_get("relative_path_bytes")
                    .map_err(|error| backend("decode repository path bytes", error))?,
                display_path: row
                    .try_get("display_path")
                    .map_err(|error| backend("decode repository display path", error))?,
                entry_type: row
                    .try_get("entry_type")
                    .map_err(|error| backend("decode repository entry type", error))?,
                size_bytes: u64::try_from(
                    row.try_get::<i64, _>("size_bytes")
                        .map_err(|error| backend("decode repository entry size", error))?,
                )
                .map_err(|error| backend("convert repository entry size", error))?,
                modified_at_ns: modified_at_ns
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<u128>()
                    .map_err(|error| backend("parse repository modified time", error))?,
            });
        }
        Ok(Some(PersistedWorkspaceIndex {
            root_path: root_path.to_owned(),
            schema_version: u32::try_from(
                root.try_get::<i64, _>("schema_version")
                    .map_err(|error| backend("decode repository schema version", error))?,
            )
            .map_err(|error| backend("convert repository schema version", error))?,
            generation: u64::try_from(
                root.try_get::<i64, _>("generation")
                    .map_err(|error| backend("decode repository generation", error))?,
            )
            .map_err(|error| backend("convert repository generation", error))?,
            state: root
                .try_get("state")
                .map_err(|error| backend("decode repository state", error))?,
            indexed_bytes: u64::try_from(
                root.try_get::<i64, _>("indexed_bytes")
                    .map_err(|error| backend("decode repository indexed bytes", error))?,
            )
            .map_err(|error| backend("convert repository indexed bytes", error))?,
            last_error: root
                .try_get("last_error")
                .map_err(|error| backend("decode repository last error", error))?,
            entries,
        }))
    }

    pub async fn mark_workspace_index_stale(&self, root_path: &str) -> Result<(), StorageError> {
        sqlx::query("UPDATE workspace_repository_indexes SET state='stale',updated_at_ms=? WHERE root_path=?")
            .bind(now_ms()?)
            .bind(root_path)
            .execute(&self.pool)
            .await
            .map_err(|error| backend("mark repository index stale", error))?;
        Ok(())
    }

    pub async fn cleanup_workspace_indexes(
        &self,
        active_roots: &[String],
    ) -> Result<u64, StorageError> {
        let rows = sqlx::query("SELECT root_path FROM workspace_repository_indexes")
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list repository index roots", error))?;
        let mut removed = 0_u64;
        for row in rows {
            let root: String = row
                .try_get("root_path")
                .map_err(|error| backend("decode repository root path", error))?;
            if !active_roots.iter().any(|candidate| candidate == &root) {
                removed = removed.saturating_add(
                    sqlx::query("DELETE FROM workspace_repository_indexes WHERE root_path=?")
                        .bind(root)
                        .execute(&self.pool)
                        .await
                        .map_err(|error| backend("remove inactive repository index", error))?
                        .rows_affected(),
                );
            }
        }
        Ok(removed)
    }
}

async fn sqlite_pragma_u64(pool: &SqlitePool, pragma: &str) -> Result<u64, StorageError> {
    let value = sqlx::query_scalar::<_, i64>(pragma)
        .fetch_one(pool)
        .await
        .map_err(|error| backend("read repository index SQLite pragma", error))?;
    u64::try_from(value).map_err(|error| backend("convert repository index SQLite pragma", error))
}

fn validate_index(index: &PersistedWorkspaceIndex) -> Result<(), StorageError> {
    if index.entries.len() > MAX_INDEX_ENTRIES {
        return Err(StorageError::InvalidData(format!(
            "repository index exceeds {MAX_INDEX_ENTRIES} entries"
        )));
    }
    if !matches!(
        index.state.as_str(),
        "building" | "fresh" | "stale" | "unknown" | "failed"
    ) {
        return Err(StorageError::InvalidData(
            "invalid repository index state".to_owned(),
        ));
    }
    let metadata_bytes = index.entries.iter().try_fold(0_usize, |total, entry| {
        let next = entry
            .relative_path_bytes
            .len()
            .saturating_add(entry.display_path.len())
            .saturating_add(entry.entry_type.len())
            .saturating_add(128);
        Ok::<_, StorageError>(total.saturating_add(next))
    })?;
    if metadata_bytes > MAX_INDEX_METADATA_BYTES {
        return Err(StorageError::InvalidData(format!(
            "repository index metadata exceeds {MAX_INDEX_METADATA_BYTES} bytes"
        )));
    }
    Ok(())
}

fn workspace_id(root_path: &str) -> String {
    let digest = Sha256::digest(root_path.as_bytes());
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{byte:02x}");
    }
    value
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn normalized_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repository_index_round_trips_transactionally_and_cleans_roots() {
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("index.sqlite3");
        let (repository, _) = SqliteRepository::open(&database, 2)
            .await
            .expect("open repository");
        let index = PersistedWorkspaceIndex {
            root_path: "/workspace/example".to_owned(),
            schema_version: 1,
            generation: 7,
            state: "fresh".to_owned(),
            indexed_bytes: 123,
            last_error: None,
            entries: vec![PersistedWorkspaceIndexEntry {
                relative_path_bytes: b"src/lib.rs".to_vec(),
                display_path: "src/lib.rs".to_owned(),
                entry_type: "file".to_owned(),
                size_bytes: 123,
                modified_at_ns: 456,
            }],
        };
        repository
            .replace_workspace_index(&index)
            .await
            .expect("persist index");
        assert_eq!(
            repository
                .load_workspace_index(&index.root_path)
                .await
                .expect("load index"),
            Some(index.clone())
        );
        repository
            .mark_workspace_index_stale(&index.root_path)
            .await
            .expect("mark stale");
        assert_eq!(
            repository
                .load_workspace_index(&index.root_path)
                .await
                .expect("reload index")
                .expect("persisted index")
                .state,
            "stale"
        );
        assert_eq!(
            repository
                .cleanup_workspace_indexes(&[])
                .await
                .expect("cleanup indexes"),
            1
        );
        assert!(
            repository
                .load_workspace_index(&index.root_path)
                .await
                .expect("load cleaned index")
                .is_none()
        );
    }

    #[tokio::test]
    async fn sqlite_growth_quota_rejects_replacement_without_losing_previous_snapshot() {
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("quota.sqlite3");
        let (repository, _) = SqliteRepository::open(&database, 2)
            .await
            .expect("open repository");
        let root_path = "/workspace/quota".to_owned();
        let original = PersistedWorkspaceIndex {
            root_path: root_path.clone(),
            schema_version: 1,
            generation: 1,
            state: "fresh".to_owned(),
            indexed_bytes: 1,
            last_error: None,
            entries: vec![PersistedWorkspaceIndexEntry {
                relative_path_bytes: b"one.txt".to_vec(),
                display_path: "one.txt".to_owned(),
                entry_type: "file".to_owned(),
                size_bytes: 1,
                modified_at_ns: 1,
            }],
        };
        repository
            .replace_workspace_index(&original)
            .await
            .expect("persist original");

        let entries = (0..2_000)
            .map(|index| {
                let display_path = format!("src/deep/path/file-{index:04}.txt");
                PersistedWorkspaceIndexEntry {
                    relative_path_bytes: display_path.as_bytes().to_vec(),
                    display_path,
                    entry_type: "file".to_owned(),
                    size_bytes: 32,
                    modified_at_ns: u128::try_from(index).unwrap_or(u128::MAX),
                }
            })
            .collect::<Vec<_>>();
        let replacement = PersistedWorkspaceIndex {
            root_path: root_path.clone(),
            schema_version: 1,
            generation: 2,
            state: "fresh".to_owned(),
            indexed_bytes: 64_000,
            last_error: None,
            entries,
        };
        let error = repository
            .replace_workspace_index_with_growth_limit(&replacement, 0)
            .await
            .expect_err("growth quota must reject replacement");
        assert!(error.to_string().contains("SQLite growth"));
        assert_eq!(
            repository
                .load_workspace_index(&root_path)
                .await
                .expect("reload previous snapshot"),
            Some(original)
        );
    }

    #[tokio::test]
    #[ignore = "manual Plan 20 SQLite benchmark: set CHATCMD_PLAN20_DB_ENTRIES=100000 or 1000000"]
    async fn repository_index_sqlite_size_and_load_benchmark() {
        let count = std::env::var("CHATCMD_PLAN20_DB_ENTRIES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100_000);
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("index-benchmark.sqlite3");
        let (repository, _) = SqliteRepository::open(&database, 4)
            .await
            .expect("open repository");
        let entries = (0..count)
            .map(|index| {
                let display_path = format!("src/d{:03}/file-{index:07}.rs", index % 100);
                PersistedWorkspaceIndexEntry {
                    relative_path_bytes: display_path.as_bytes().to_vec(),
                    display_path,
                    entry_type: "file".to_owned(),
                    size_bytes: 128,
                    modified_at_ns: u128::try_from(index).unwrap_or(u128::MAX),
                }
            })
            .collect::<Vec<_>>();
        let index = PersistedWorkspaceIndex {
            root_path: "/benchmark/workspace".to_owned(),
            schema_version: 1,
            generation: 1,
            state: "fresh".to_owned(),
            indexed_bytes: u64::try_from(count).unwrap_or(u64::MAX).saturating_mul(128),
            last_error: None,
            entries,
        };
        let write_started = std::time::Instant::now();
        repository
            .replace_workspace_index(&index)
            .await
            .expect("persist benchmark index");
        let write_elapsed = write_started.elapsed();
        let mut load_samples = Vec::with_capacity(10);
        for _ in 0..10 {
            let started = std::time::Instant::now();
            let loaded = repository
                .load_workspace_index(&index.root_path)
                .await
                .expect("load benchmark index")
                .expect("stored benchmark index");
            assert_eq!(loaded.entries.len(), count);
            load_samples.push(started.elapsed());
        }
        load_samples.sort_unstable();
        let p50 = load_samples[load_samples.len() / 2];
        let p95 = load_samples[load_samples.len().saturating_sub(1) * 95 / 100];
        let db_bytes = std::fs::metadata(&database).map_or(0, |value| value.len());
        let wal_bytes = std::fs::metadata(database.with_extension("sqlite3-wal"))
            .map_or(0, |value| value.len());
        eprintln!(
            "plan20 sqlite entries={count} write_ms={:.3} load_p50_ms={:.3} load_p95_ms={:.3} db_bytes={db_bytes} wal_bytes={wal_bytes}",
            write_elapsed.as_secs_f64() * 1000.0,
            p50.as_secs_f64() * 1000.0,
            p95.as_secs_f64() * 1000.0,
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[tokio::test]
    async fn corrupt_repository_index_row_is_reported_without_destroying_root_record() {
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("corrupt-index.sqlite3");
        let (repository, _) = SqliteRepository::open(&database, 2)
            .await
            .expect("open repository");
        let root_path = "/workspace/corrupt".to_owned();
        repository
            .replace_workspace_index(&PersistedWorkspaceIndex {
                root_path: root_path.clone(),
                schema_version: 1,
                generation: 3,
                state: "fresh".to_owned(),
                indexed_bytes: 4,
                last_error: None,
                entries: vec![PersistedWorkspaceIndexEntry {
                    relative_path_bytes: b"bad.txt".to_vec(),
                    display_path: "bad.txt".to_owned(),
                    entry_type: "file".to_owned(),
                    size_bytes: 4,
                    modified_at_ns: 7,
                }],
            })
            .await
            .expect("persist index");
        let workspace_id = workspace_id(&root_path);
        sqlx::query(
            "UPDATE workspace_repository_index_entries SET modified_at_ns='not-a-number' WHERE workspace_id=?",
        )
        .bind(workspace_id)
        .execute(repository.pool())
        .await
        .expect("corrupt row");

        assert!(repository.load_workspace_index(&root_path).await.is_err());
        repository
            .mark_workspace_index_stale(&root_path)
            .await
            .expect("root record remains writable");
    }
}
