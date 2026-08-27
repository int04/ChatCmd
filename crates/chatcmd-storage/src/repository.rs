mod agents;
mod catalog;
mod mapping;
mod records;
mod settings;

use mapping::*;

use std::{path::Path, str::FromStr, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chatcmd_core::{
    AgentId, AgentName, AgentSecretResult, Approval, Artifact, ArtifactId, ArtifactStore,
    Bootstrap, BootstrapReport, DeviceId, EventId, EventKind, ExecutionMode, GeneratedSecret,
    LocalDevice, LocalDeviceStore, McpAgent, McpAgentPolicy, McpAgentStore, NewMcpAgent,
    PolicyLookup, Recovery, SecretHash, SessionId, Setting, SettingsStore, StorageError, Task,
    TaskExecutionMode, TaskId, TaskSession, TaskStatus, TaskStore, TerminalEventChunk,
    TerminalEventStore, TerminalSession, TimelineEvent, ToolCapability, ToolCatalogStore,
    ToolDefinition, ToolGroup, ToolPreset, TurnBinding,
};
use rand::{RngCore, rngs::OsRng};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub const CURRENT_SCHEMA_VERSION: i64 = 1;
pub const MAX_TERMINAL_CHUNK_BYTES: usize = 65_536;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// SQLite-backed implementation of all local domain stores.
#[derive(Debug, Clone)]
pub struct SqliteRepository {
    pool: SqlitePool,
}

impl SqliteRepository {
    /// Opens a bounded SQLite pool. Call [`Bootstrap::bootstrap`] before repository use.
    pub async fn connect(path: &Path, max_connections: u32) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| backend("create database directory", error))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5_000))
            .synchronous(SqliteSynchronous::Normal);
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections.clamp(1, 16))
            .min_connections(1)
            .acquire_timeout(Duration::from_millis(5_000))
            .connect_with(options)
            .await
            .map_err(|error| backend("open database", error))?;
        Ok(Self { pool })
    }

    /// Opens and transactionally bootstraps a database.
    pub async fn open(
        path: &Path,
        max_connections: u32,
    ) -> Result<(Self, BootstrapReport), StorageError> {
        let repository = Self::connect(path, max_connections).await?;
        let report = repository.bootstrap().await?;
        Ok((repository, report))
    }

    /// Exposes the pool only for host-level health checks and controlled integration tests.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn schema_version_if_present(&self) -> Result<Option<i64>, StorageError> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| backend("inspect schema", error))?;
        if exists == 0 {
            return Ok(None);
        }
        sqlx::query_scalar("SELECT version FROM schema_version WHERE singleton_id=1")
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| backend("read schema version", error))
    }

    async fn seed(transaction: &mut Transaction<'_, Sqlite>) -> Result<LocalDevice, StorageError> {
        let now = now_ms()?;
        let existing = sqlx::query("SELECT device_id, installation_id, name, platform, os_version, architecture, app_version, created_at_ms, updated_at_ms FROM local_device WHERE singleton_id=1")
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| backend("read local device", error))?;
        let device = if let Some(row) = existing {
            map_device(&row)?
        } else {
            let identifier = uuid::Uuid::new_v4().to_string();
            let device = LocalDevice {
                id: DeviceId::new(identifier.clone()).map_err(invalid_data)?,
                installation_id: identifier,
                name: std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_else(|_| "Local device".to_owned()),
                platform: std::env::consts::OS.to_owned(),
                os_version: None,
                architecture: std::env::consts::ARCH.to_owned(),
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                created_at_ms: now,
                updated_at_ms: now,
            };
            sqlx::query("INSERT INTO local_device(singleton_id, device_id, installation_id, name, platform, os_version, architecture, app_version, created_at_ms, updated_at_ms) VALUES(1,?,?,?,?,?,?,?,?,?)")
                .bind(device.id.as_str())
                .bind(&device.installation_id)
                .bind(&device.name)
                .bind(&device.platform)
                .bind(&device.os_version)
                .bind(&device.architecture)
                .bind(&device.app_version)
                .bind(device.created_at_ms)
                .bind(device.updated_at_ms)
                .execute(&mut **transaction)
                .await
                .map_err(|error| backend("seed local device", error))?;
            device
        };

        sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES('command_execution_mode','\"approval\"',?) ON CONFLICT(key) DO NOTHING")
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(|error| backend("seed settings", error))?;
        for (id, key, name, order) in [
            ("group-device", "device", "Device", 10_i32),
            ("group-terminal", "terminal", "Terminal", 20_i32),
            ("group-workspace", "workspace", "Workspace", 30_i32),
        ] {
            sqlx::query("INSERT INTO tool_groups(id,key,display_name,sort_order) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key, display_name=excluded.display_name, sort_order=excluded.sort_order")
                .bind(id).bind(key).bind(name).bind(order)
                .execute(&mut **transaction).await
                .map_err(|error| backend("seed tool groups", error))?;
        }
        for (id, key, group, title, capabilities) in [
            (
                "tool-device-list",
                "device_list",
                "group-device",
                "List local device",
                "[\"read\"]",
            ),
            (
                "tool-shell-create",
                "shell_create",
                "group-terminal",
                "Create terminal",
                "[\"execute\"]",
            ),
            (
                "tool-shell-read",
                "shell_read",
                "group-terminal",
                "Read terminal",
                "[\"read\"]",
            ),
            (
                "tool-shell-write",
                "shell_write",
                "group-terminal",
                "Write terminal",
                "[\"write\",\"execute\"]",
            ),
            (
                "tool-fs-read",
                "fs_read_text",
                "group-workspace",
                "Read text file",
                "[\"read\"]",
            ),
        ] {
            sqlx::query("INSERT INTO tools(id,key,group_id,title,description,input_schema_json,capabilities_json,enabled) VALUES(?,?,?,?,?,'{}',?,1) ON CONFLICT(id) DO UPDATE SET key=excluded.key, group_id=excluded.group_id, title=excluded.title, capabilities_json=excluded.capabilities_json, enabled=1")
                .bind(id).bind(key).bind(group).bind(title).bind(title).bind(capabilities)
                .execute(&mut **transaction).await
                .map_err(|error| backend("seed tools", error))?;
        }
        sqlx::query("INSERT INTO tool_presets(id,key,name,description) VALUES('preset-safe','safe','Safe local tools','Read-only and terminal basics') ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description")
            .execute(&mut **transaction).await
            .map_err(|error| backend("seed presets", error))?;
        for tool_id in [
            "tool-device-list",
            "tool-shell-create",
            "tool-shell-read",
            "tool-fs-read",
        ] {
            sqlx::query(
                "INSERT OR IGNORE INTO preset_tools(preset_id,tool_id) VALUES('preset-safe',?)",
            )
            .bind(tool_id)
            .execute(&mut **transaction)
            .await
            .map_err(|error| backend("seed preset tools", error))?;
        }
        for (id, name, order) in [
            ("agent-name-1", "Atlas", 10_i32),
            ("agent-name-2", "Nova", 20_i32),
        ] {
            sqlx::query("INSERT INTO agent_names(id,name,enabled,sort_order) VALUES(?,?,1,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name, sort_order=excluded.sort_order")
                .bind(id).bind(name).bind(order).execute(&mut **transaction).await
                .map_err(|error| backend("seed agent names", error))?;
        }
        Ok(device)
    }

    pub(crate) async fn insert_terminal_chunks_tx(
        transaction: &mut Transaction<'_, Sqlite>,
        chunks: &[TerminalEventChunk],
    ) -> Result<usize, StorageError> {
        let mut inserted = 0_usize;
        for chunk in chunks {
            if chunk.payload.len() > MAX_TERMINAL_CHUNK_BYTES {
                return Err(StorageError::InvalidData(format!(
                    "terminal chunk {} exceeds {MAX_TERMINAL_CHUNK_BYTES} bytes",
                    chunk.event_id
                )));
            }
            inserted += usize::try_from(sqlx::query("INSERT INTO terminal_event_chunks(session_id,sequence,event_id,task_id,turn_id,kind,stream,payload,payload_encoding,created_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(event_id) DO NOTHING")
                .bind(chunk.session_id.as_str()).bind(chunk.sequence).bind(chunk.event_id.as_str())
                .bind(chunk.task_id.as_ref().map(TaskId::as_str)).bind(chunk.turn_id.as_ref().map(chatcmd_core::TurnId::as_str))
                .bind(chunk.kind.as_str()).bind(&chunk.stream).bind(&chunk.payload).bind(&chunk.payload_encoding).bind(chunk.created_at_ms)
                .execute(&mut **transaction).await.map_err(|error| backend("append terminal chunk", error))?.rows_affected())
                .map_err(|error| backend("convert affected row count", error))?;
        }
        Ok(inserted)
    }

    pub(crate) async fn append_chunk_batch(
        &self,
        chunks: &[TerminalEventChunk],
    ) -> Result<usize, StorageError> {
        if chunks.len() > 250 {
            return Err(StorageError::InvalidData(
                "event batch exceeds 250 chunks".to_owned(),
            ));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin event batch", error))?;
        let inserted = Self::insert_terminal_chunks_tx(&mut transaction, chunks).await?;
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit event batch", error))?;
        Ok(inserted)
    }
}

impl Bootstrap for SqliteRepository {
    async fn bootstrap(&self) -> Result<BootstrapReport, StorageError> {
        if let Some(version) = self.schema_version_if_present().await?
            && version > CURRENT_SCHEMA_VERSION
        {
            return Err(StorageError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|error| backend("run migrations", error))?;
        let version = self
            .schema_version_if_present()
            .await?
            .ok_or_else(|| StorageError::InvalidData("schema version row is missing".to_owned()))?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(StorageError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin bootstrap", error))?;
        let device = Self::seed(&mut transaction).await?;
        let now = now_ms()?;
        let interrupted_sessions = sqlx::query("UPDATE terminal_sessions SET status='interrupted', updated_at_ms=?, closed_at_ms=COALESCE(closed_at_ms,?) WHERE status IN ('starting','running')")
            .bind(now).bind(now).execute(&mut *transaction).await
            .map_err(|error| backend("recover terminal sessions", error))?.rows_affected();
        sqlx::query("UPDATE task_sessions SET status='interrupted', updated_at_ms=? WHERE status IN ('starting','running')")
            .bind(now).execute(&mut *transaction).await
            .map_err(|error| backend("recover task sessions", error))?;
        let interrupted_tasks = sqlx::query("UPDATE tasks SET status='interrupted', active_session_id=NULL, updated_at_ms=? WHERE status='running'")
            .bind(now).execute(&mut *transaction).await
            .map_err(|error| backend("recover tasks", error))?.rows_affected();
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit bootstrap", error))?;
        Ok(BootstrapReport {
            schema_version: version,
            device,
            interrupted_tasks,
            interrupted_sessions,
        })
    }
}

impl Recovery for SqliteRepository {
    async fn recover_interrupted(&self) -> Result<(u64, u64), StorageError> {
        let report = self.bootstrap().await?;
        Ok((report.interrupted_tasks, report.interrupted_sessions))
    }
}

impl LocalDeviceStore for SqliteRepository {
    async fn local_device(&self) -> Result<LocalDevice, StorageError> {
        let row = sqlx::query("SELECT device_id, installation_id, name, platform, os_version, architecture, app_version, created_at_ms, updated_at_ms FROM local_device WHERE singleton_id=1")
            .fetch_optional(&self.pool).await.map_err(|error| backend("read local device", error))?
            .ok_or_else(|| StorageError::NotFound("local device".to_owned()))?;
        map_device(&row)
    }
}

fn generate_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn now_ms() -> Result<i64, StorageError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| backend("read system clock", error))?;
    i64::try_from(duration.as_millis()).map_err(|error| backend("convert system timestamp", error))
}

pub(crate) fn backend(context: &str, error: impl std::fmt::Display) -> StorageError {
    StorageError::Backend(format!("{context}: {error}"))
}

fn invalid_data(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}

fn map_sqlx_conflict(context: &str, error: sqlx::Error) -> StorageError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.is_foreign_key_violation()
            || database.is_check_violation())
    {
        return StorageError::Conflict(format!("{context}: {database}"));
    }
    backend(context, error)
}
