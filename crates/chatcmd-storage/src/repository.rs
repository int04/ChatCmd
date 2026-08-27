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

impl McpAgentStore for SqliteRepository {
    async fn create_agent(&self, input: NewMcpAgent) -> Result<AgentSecretResult, StorageError> {
        let raw = generate_secret();
        let digest = SecretHash::from_bearer(&raw);
        let generated = GeneratedSecret::new(raw);
        let id = input.id.unwrap_or_else(|| {
            AgentId::new(uuid::Uuid::new_v4().to_string()).expect("UUID is non-empty")
        });
        let now = now_ms()?;
        let result = sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,project_folder,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?)")
            .bind(id.as_str()).bind(input.name.trim()).bind(digest.as_bytes().as_slice()).bind(generated.last4())
            .bind(input.enabled).bind(&input.project_folder).bind(now).bind(now)
            .execute(&self.pool).await;
        if let Err(error) = result {
            return Err(map_sqlx_conflict("create MCP agent", error));
        }
        let agent = McpAgent {
            id,
            name: input.name.trim().to_owned(),
            enabled: input.enabled,
            project_folder: input.project_folder,
            secret_last4: generated.last4().to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
            last_used_at_ms: None,
        };
        Ok(AgentSecretResult {
            agent,
            secret: generated,
        })
    }

    async fn list_agents(&self) -> Result<Vec<McpAgent>, StorageError> {
        let rows = sqlx::query("SELECT id,name,enabled,project_folder,secret_last4,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents ORDER BY name COLLATE NOCASE")
            .fetch_all(&self.pool).await.map_err(|error| backend("list MCP agents", error))?;
        rows.iter().map(map_agent).collect()
    }

    async fn agent(&self, id: &AgentId) -> Result<Option<McpAgent>, StorageError> {
        let row = sqlx::query("SELECT id,name,enabled,project_folder,secret_last4,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read MCP agent", error))?;
        row.as_ref().map(map_agent).transpose()
    }

    async fn rotate_agent_secret(&self, id: &AgentId) -> Result<AgentSecretResult, StorageError> {
        let raw = generate_secret();
        let digest = SecretHash::from_bearer(&raw);
        let generated = GeneratedSecret::new(raw);
        let now = now_ms()?;
        let affected = sqlx::query(
            "UPDATE mcp_agents SET secret_hash=?, secret_last4=?, updated_at_ms=? WHERE id=?",
        )
        .bind(digest.as_bytes().as_slice())
        .bind(generated.last4())
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| backend("rotate MCP agent secret", error))?
        .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        let agent = self
            .agent(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("MCP agent {id}")))?;
        Ok(AgentSecretResult {
            agent,
            secret: generated,
        })
    }

    async fn set_agent_enabled(&self, id: &AgentId, enabled: bool) -> Result<(), StorageError> {
        let affected = sqlx::query("UPDATE mcp_agents SET enabled=?,updated_at_ms=? WHERE id=?")
            .bind(enabled)
            .bind(now_ms()?)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| backend("set MCP agent state", error))?
            .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        Ok(())
    }

    async fn update_agent(
        &self,
        id: &AgentId,
        input: NewMcpAgent,
    ) -> Result<McpAgent, StorageError> {
        let affected = sqlx::query(
            "UPDATE mcp_agents SET name=?,enabled=?,project_folder=?,updated_at_ms=? WHERE id=?",
        )
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(input.project_folder)
        .bind(now_ms()?)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| map_sqlx_conflict("update MCP agent", error))?
        .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        self.agent(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("MCP agent {id}")))
    }

    async fn delete_agent(&self, id: &AgentId) -> Result<(), StorageError> {
        let affected = sqlx::query("DELETE FROM mcp_agents WHERE id=?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| map_sqlx_conflict("delete MCP agent", error))?
            .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        Ok(())
    }
}

impl PolicyLookup for SqliteRepository {
    async fn lookup_policy_by_bearer(
        &self,
        raw_bearer: &str,
    ) -> Result<Option<McpAgentPolicy>, StorageError> {
        let candidate = SecretHash::from_bearer(raw_bearer);
        let row = sqlx::query("SELECT id,name,enabled,project_folder,secret_last4,secret_hash,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents WHERE secret_hash=? AND enabled=1")
            .bind(candidate.as_bytes().as_slice()).fetch_optional(&self.pool).await
            .map_err(|error| backend("lookup MCP bearer", error))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored = SecretHash::from_bytes(
            row.try_get::<Vec<u8>, _>("secret_hash")
                .map_err(|error| backend("map secret hash", error))?
                .as_slice(),
        )
        .map_err(invalid_data)?;
        if !stored.constant_time_eq(&candidate) {
            return Ok(None);
        }
        let agent = map_agent(&row)?;
        let tool_rows = sqlx::query("SELECT tools.key FROM tools JOIN agent_allowed_tools ON agent_allowed_tools.tool_id=tools.id WHERE agent_allowed_tools.agent_id=? AND tools.enabled=1 ORDER BY tools.key")
            .bind(agent.id.as_str()).fetch_all(&self.pool).await.map_err(|error| backend("read agent allowlist", error))?;
        let allowed_tool_keys = tool_rows
            .iter()
            .map(|item| {
                item.try_get::<String, _>("key")
                    .map_err(|error| backend("map tool key", error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        sqlx::query("UPDATE mcp_agents SET last_used_at_ms=? WHERE id=?")
            .bind(now_ms()?)
            .bind(agent.id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| backend("update agent last use", error))?;
        Ok(Some(McpAgentPolicy {
            agent,
            allowed_tool_keys,
        }))
    }
}

impl ToolCatalogStore for SqliteRepository {
    async fn replace_catalog(
        &self,
        groups: &[ToolGroup],
        tools: &[ToolDefinition],
        presets: &[ToolPreset],
    ) -> Result<(), StorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin catalog sync", error))?;
        for group in groups {
            sqlx::query("INSERT INTO tool_groups(id,key,display_name,sort_order) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,display_name=excluded.display_name,sort_order=excluded.sort_order")
                .bind(&group.id).bind(&group.key).bind(&group.display_name).bind(group.sort_order)
                .execute(&mut *transaction).await.map_err(|error| backend("sync tool group", error))?;
        }
        for tool in tools {
            let capabilities = serde_json::to_string(&tool.capabilities)
                .map_err(|error| backend("serialize tool capabilities", error))?;
            sqlx::query("INSERT INTO tools(id,key,group_id,title,description,input_schema_json,capabilities_json,enabled) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,group_id=excluded.group_id,title=excluded.title,description=excluded.description,input_schema_json=excluded.input_schema_json,capabilities_json=excluded.capabilities_json,enabled=excluded.enabled")
                .bind(&tool.id).bind(&tool.key).bind(&tool.group_id).bind(&tool.title).bind(&tool.description)
                .bind(&tool.input_schema_json).bind(capabilities).bind(tool.enabled)
                .execute(&mut *transaction).await.map_err(|error| backend("sync tool", error))?;
        }
        for preset in presets {
            sqlx::query("INSERT INTO tool_presets(id,key,name,description) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,name=excluded.name,description=excluded.description")
                .bind(&preset.id).bind(&preset.key).bind(&preset.name).bind(&preset.description)
                .execute(&mut *transaction).await.map_err(|error| backend("sync preset", error))?;
            sqlx::query("DELETE FROM preset_tools WHERE preset_id=?")
                .bind(&preset.id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| backend("clear preset tools", error))?;
            for tool_id in &preset.tool_ids {
                sqlx::query("INSERT INTO preset_tools(preset_id,tool_id) VALUES(?,?)")
                    .bind(&preset.id)
                    .bind(tool_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| backend("sync preset tool", error))?;
            }
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit catalog sync", error))
    }

    async fn list_tools(&self) -> Result<Vec<ToolDefinition>, StorageError> {
        let rows = sqlx::query("SELECT id,key,group_id,title,description,input_schema_json,capabilities_json,enabled FROM tools ORDER BY key")
            .fetch_all(&self.pool).await.map_err(|error| backend("list tools", error))?;
        rows.iter()
            .map(|row| {
                let raw: String = row
                    .try_get("capabilities_json")
                    .map_err(|error| backend("map capabilities", error))?;
                let capabilities: Vec<ToolCapability> = serde_json::from_str(&raw)
                    .map_err(|error| backend("parse capabilities", error))?;
                Ok(ToolDefinition {
                    id: row
                        .try_get("id")
                        .map_err(|error| backend("map tool id", error))?,
                    key: row
                        .try_get("key")
                        .map_err(|error| backend("map tool key", error))?,
                    group_id: row
                        .try_get("group_id")
                        .map_err(|error| backend("map tool group", error))?,
                    title: row
                        .try_get("title")
                        .map_err(|error| backend("map tool title", error))?,
                    description: row
                        .try_get("description")
                        .map_err(|error| backend("map tool description", error))?,
                    input_schema_json: row
                        .try_get("input_schema_json")
                        .map_err(|error| backend("map tool schema", error))?,
                    capabilities,
                    enabled: row
                        .try_get("enabled")
                        .map_err(|error| backend("map tool state", error))?,
                })
            })
            .collect()
    }

    async fn list_presets(&self) -> Result<Vec<ToolPreset>, StorageError> {
        let rows = sqlx::query("SELECT id,key,name,description FROM tool_presets ORDER BY name,id")
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list tool presets", error))?;
        let mut presets = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row
                .try_get("id")
                .map_err(|error| backend("map preset id", error))?;
            let tool_ids = sqlx::query_scalar(
                "SELECT tool_id FROM preset_tools WHERE preset_id=? ORDER BY tool_id",
            )
            .bind(&id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list preset tools", error))?;
            presets.push(ToolPreset {
                id,
                key: row
                    .try_get("key")
                    .map_err(|error| backend("map preset key", error))?,
                name: row
                    .try_get("name")
                    .map_err(|error| backend("map preset name", error))?,
                description: row
                    .try_get("description")
                    .map_err(|error| backend("map preset description", error))?,
                tool_ids,
            });
        }
        Ok(presets)
    }

    async fn agent_allowed_tool_ids(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<String>, StorageError> {
        sqlx::query_scalar(
            "SELECT tool_id FROM agent_allowed_tools WHERE agent_id=? ORDER BY tool_id",
        )
        .bind(agent_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| backend("list agent allowlist", error))
    }

    async fn set_agent_allowed_tools(
        &self,
        agent_id: &AgentId,
        tool_ids: &[String],
    ) -> Result<(), StorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin allowlist update", error))?;
        sqlx::query("DELETE FROM agent_allowed_tools WHERE agent_id=?")
            .bind(agent_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("clear allowlist", error))?;
        for tool_id in tool_ids {
            sqlx::query("INSERT INTO agent_allowed_tools(agent_id,tool_id) VALUES(?,?)")
                .bind(agent_id.as_str())
                .bind(tool_id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| map_sqlx_conflict("set agent allowlist", error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit allowlist update", error))
    }

    async fn list_agent_names(&self) -> Result<Vec<AgentName>, StorageError> {
        let rows = sqlx::query(
            "SELECT id,name,enabled,sort_order FROM agent_names ORDER BY sort_order,id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| backend("list agent names", error))?;
        rows.iter()
            .map(|row| {
                Ok(AgentName {
                    id: row
                        .try_get("id")
                        .map_err(|error| backend("map agent name id", error))?,
                    name: row
                        .try_get("name")
                        .map_err(|error| backend("map agent name", error))?,
                    enabled: row
                        .try_get("enabled")
                        .map_err(|error| backend("map agent name state", error))?,
                    sort_order: row
                        .try_get("sort_order")
                        .map_err(|error| backend("map agent name order", error))?,
                })
            })
            .collect()
    }
}

impl TaskStore for SqliteRepository {
    async fn upsert_task(&self, task: &Task) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,device_id=excluded.device_id,conversation_scope_hash=excluded.conversation_scope_hash,title=excluded.title,source=excluded.source,status=excluded.status,active_session_id=excluded.active_session_id,generation=excluded.generation,stopped_at_ms=excluded.stopped_at_ms,updated_at_ms=excluded.updated_at_ms")
            .bind(task.id.as_str()).bind(task.agent_id.as_ref().map(AgentId::as_str)).bind(task.device_id.as_str())
            .bind(&task.conversation_scope_hash).bind(&task.title).bind(&task.source).bind(task.status.as_str())
            .bind(task.active_session_id.as_ref().map(SessionId::as_str)).bind(task.generation).bind(task.stopped_at_ms)
            .bind(task.created_at_ms).bind(task.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert task", error))?;
        Ok(())
    }

    async fn task(&self, id: &TaskId) -> Result<Option<Task>, StorageError> {
        let row = sqlx::query("SELECT id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms FROM tasks WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read task", error))?;
        row.as_ref().map(map_task).transpose()
    }

    async fn list_tasks(&self, limit: u32) -> Result<Vec<Task>, StorageError> {
        let rows = sqlx::query("SELECT id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms FROM tasks ORDER BY updated_at_ms DESC,id LIMIT ?")
            .bind(i64::from(limit.clamp(1, 1000)))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list tasks", error))?;
        rows.iter().map(map_task).collect()
    }

    async fn upsert_task_session(&self, session: &TaskSession) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO task_sessions(task_id,session_id,generation,replaced_session_id,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(task_id,session_id) DO UPDATE SET generation=excluded.generation,replaced_session_id=excluded.replaced_session_id,status=excluded.status,updated_at_ms=excluded.updated_at_ms")
            .bind(session.task_id.as_str()).bind(session.session_id.as_str()).bind(session.generation)
            .bind(session.replaced_session_id.as_ref().map(SessionId::as_str)).bind(session.status.as_str())
            .bind(session.created_at_ms).bind(session.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert task session", error))?;
        Ok(())
    }

    async fn bind_turn(&self, binding: &TurnBinding) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO turn_bindings(agent_id,device_id,turn_id,task_id,last_used_at_ms) VALUES(?,?,?,?,?) ON CONFLICT(agent_id,device_id,turn_id) DO UPDATE SET task_id=excluded.task_id,last_used_at_ms=excluded.last_used_at_ms")
            .bind(binding.agent_id.as_str()).bind(binding.device_id.as_str()).bind(binding.turn_id.as_str())
            .bind(binding.task_id.as_str()).bind(binding.last_used_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("bind turn", error))?;
        Ok(())
    }

    async fn save_approval(&self, approval: &Approval) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO approvals(id,task_id,session_id,state,request_json,decision_json,created_at_ms,resolved_at_ms) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state,decision_json=excluded.decision_json,resolved_at_ms=excluded.resolved_at_ms")
            .bind(&approval.id).bind(approval.task_id.as_str()).bind(approval.session_id.as_ref().map(SessionId::as_str))
            .bind(approval.state.as_str()).bind(&approval.request_json).bind(&approval.decision_json)
            .bind(approval.created_at_ms).bind(approval.resolved_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("save approval", error))?;
        Ok(())
    }

    async fn set_execution_mode(&self, mode: &TaskExecutionMode) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,?,?) ON CONFLICT(task_id) DO UPDATE SET mode=excluded.mode,updated_at_ms=excluded.updated_at_ms")
            .bind(mode.task_id.as_str()).bind(mode.mode.as_str()).bind(mode.updated_at_ms)
            .execute(&self.pool).await.map_err(|error| backend("set task execution mode", error))?;
        Ok(())
    }
}

impl TerminalEventStore for SqliteRepository {
    async fn upsert_terminal_session(&self, session: &TerminalSession) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO terminal_sessions(id,task_id,turn_id,executable,working_directory,columns,rows,process_id,status,exit_code,created_at_ms,updated_at_ms,closed_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET task_id=excluded.task_id,turn_id=excluded.turn_id,executable=excluded.executable,working_directory=excluded.working_directory,columns=excluded.columns,rows=excluded.rows,process_id=excluded.process_id,status=excluded.status,exit_code=excluded.exit_code,updated_at_ms=excluded.updated_at_ms,closed_at_ms=excluded.closed_at_ms")
            .bind(session.id.as_str()).bind(session.task_id.as_ref().map(TaskId::as_str)).bind(session.turn_id.as_ref().map(chatcmd_core::TurnId::as_str))
            .bind(&session.executable).bind(&session.working_directory).bind(session.columns).bind(session.rows)
            .bind(session.process_id).bind(session.status.as_str()).bind(session.exit_code).bind(session.created_at_ms)
            .bind(session.updated_at_ms).bind(session.closed_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert terminal session", error))?;
        Ok(())
    }

    async fn append_terminal_chunks(
        &self,
        chunks: &[TerminalEventChunk],
    ) -> Result<usize, StorageError> {
        if chunks.len() > 250 {
            return Err(StorageError::InvalidData(
                "event batch exceeds 250 chunks".to_owned(),
            ));
        }
        self.append_chunk_batch(chunks).await
    }

    async fn terminal_chunks(
        &self,
        session_id: &SessionId,
        after_sequence: Option<i64>,
        limit: u32,
    ) -> Result<Vec<TerminalEventChunk>, StorageError> {
        let rows = sqlx::query("SELECT session_id,sequence,event_id,task_id,turn_id,kind,stream,payload,payload_encoding,created_at_ms FROM terminal_event_chunks WHERE session_id=? AND sequence>? ORDER BY sequence LIMIT ?")
            .bind(session_id.as_str()).bind(after_sequence.unwrap_or(-1)).bind(i64::from(limit.clamp(1, 1000)))
            .fetch_all(&self.pool).await.map_err(|error| backend("read terminal chunks", error))?;
        rows.iter().map(map_chunk).collect()
    }

    async fn append_timeline_events(
        &self,
        events: &[TimelineEvent],
    ) -> Result<usize, StorageError> {
        if events.len() > 250 {
            return Err(StorageError::InvalidData(
                "timeline batch exceeds 250 events".to_owned(),
            ));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin timeline batch", error))?;
        let mut inserted = 0_usize;
        for event in events {
            inserted += usize::try_from(sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?)")
                .bind(event.id.as_str()).bind(event.task_id.as_str()).bind(event.turn_id.as_ref().map(chatcmd_core::TurnId::as_str))
                .bind(event.session_id.as_ref().map(SessionId::as_str)).bind(event.actor.as_str()).bind(event.kind.as_str())
                .bind(&event.idempotency_key).bind(&event.payload_json).bind(&event.metadata_json).bind(event.created_at_ms)
                .execute(&mut *transaction).await.map_err(|error| map_sqlx_conflict("append timeline event", error))?.rows_affected())
                .map_err(|error| backend("convert affected row count", error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit timeline batch", error))?;
        Ok(inserted)
    }
}

impl SettingsStore for SqliteRepository {
    async fn setting(&self, key: &str) -> Result<Option<Setting>, StorageError> {
        let row = sqlx::query("SELECT key,value_json,updated_at_ms FROM settings WHERE key=?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| backend("read setting", error))?;
        row.as_ref()
            .map(|item| {
                Ok(Setting {
                    key: item
                        .try_get("key")
                        .map_err(|error| backend("map setting key", error))?,
                    value_json: item
                        .try_get("value_json")
                        .map_err(|error| backend("map setting value", error))?,
                    updated_at_ms: item
                        .try_get("updated_at_ms")
                        .map_err(|error| backend("map setting timestamp", error))?,
                })
            })
            .transpose()
    }

    async fn set_setting(&self, setting: &Setting) -> Result<(), StorageError> {
        serde_json::from_str::<serde_json::Value>(&setting.value_json)
            .map_err(|error| backend("validate setting JSON", error))?;
        sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at_ms=excluded.updated_at_ms")
            .bind(&setting.key).bind(&setting.value_json).bind(setting.updated_at_ms).execute(&self.pool).await
            .map_err(|error| backend("write setting", error))?;
        Ok(())
    }

    async fn execution_mode(
        &self,
        task_id: Option<&TaskId>,
    ) -> Result<ExecutionMode, StorageError> {
        if let Some(task_id) = task_id {
            let mode: Option<String> =
                sqlx::query_scalar("SELECT mode FROM task_execution_modes WHERE task_id=?")
                    .bind(task_id.as_str())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| backend("read task execution mode", error))?;
            if let Some(mode) = mode {
                return ExecutionMode::from_str(&mode).map_err(invalid_data);
            }
        }
        let setting = self.setting("command_execution_mode").await?;
        match setting {
            Some(value) => {
                let mode: String = serde_json::from_str(&value.value_json)
                    .map_err(|error| backend("parse execution mode setting", error))?;
                ExecutionMode::from_str(&mode).map_err(invalid_data)
            }
            None => Ok(ExecutionMode::Approval),
        }
    }
}

impl ArtifactStore for SqliteRepository {
    async fn register_artifact(&self, artifact: &Artifact) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO artifact_registry(id,task_id,session_id,relative_path,media_type,size_bytes,sha256_hex,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET relative_path=excluded.relative_path,media_type=excluded.media_type,size_bytes=excluded.size_bytes,sha256_hex=excluded.sha256_hex,updated_at_ms=excluded.updated_at_ms")
            .bind(artifact.id.as_str()).bind(artifact.task_id.as_str()).bind(artifact.session_id.as_ref().map(SessionId::as_str))
            .bind(&artifact.relative_path).bind(&artifact.media_type).bind(artifact.size_bytes).bind(&artifact.sha256_hex)
            .bind(artifact.created_at_ms).bind(artifact.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("register artifact", error))?;
        Ok(())
    }

    async fn artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>, StorageError> {
        let row = sqlx::query("SELECT id,task_id,session_id,relative_path,media_type,size_bytes,sha256_hex,created_at_ms,updated_at_ms FROM artifact_registry WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read artifact", error))?;
        row.as_ref()
            .map(|item| {
                Ok(Artifact {
                    id: ArtifactId::new(
                        item.try_get::<String, _>("id")
                            .map_err(|error| backend("map artifact id", error))?,
                    )
                    .map_err(invalid_data)?,
                    task_id: TaskId::new(
                        item.try_get::<String, _>("task_id")
                            .map_err(|error| backend("map artifact task", error))?,
                    )
                    .map_err(invalid_data)?,
                    session_id: item
                        .try_get::<Option<String>, _>("session_id")
                        .map_err(|error| backend("map artifact session", error))?
                        .map(SessionId::new)
                        .transpose()
                        .map_err(invalid_data)?,
                    relative_path: item
                        .try_get("relative_path")
                        .map_err(|error| backend("map artifact path", error))?,
                    media_type: item
                        .try_get("media_type")
                        .map_err(|error| backend("map artifact media type", error))?,
                    size_bytes: item
                        .try_get("size_bytes")
                        .map_err(|error| backend("map artifact size", error))?,
                    sha256_hex: item
                        .try_get("sha256_hex")
                        .map_err(|error| backend("map artifact hash", error))?,
                    created_at_ms: item
                        .try_get("created_at_ms")
                        .map_err(|error| backend("map artifact timestamp", error))?,
                    updated_at_ms: item
                        .try_get("updated_at_ms")
                        .map_err(|error| backend("map artifact timestamp", error))?,
                })
            })
            .transpose()
    }
}

fn map_device(row: &sqlx::sqlite::SqliteRow) -> Result<LocalDevice, StorageError> {
    Ok(LocalDevice {
        id: DeviceId::new(
            row.try_get::<String, _>("device_id")
                .map_err(|error| backend("map device id", error))?,
        )
        .map_err(invalid_data)?,
        installation_id: row
            .try_get("installation_id")
            .map_err(|error| backend("map installation id", error))?,
        name: row
            .try_get("name")
            .map_err(|error| backend("map device name", error))?,
        platform: row
            .try_get("platform")
            .map_err(|error| backend("map device platform", error))?,
        os_version: row
            .try_get("os_version")
            .map_err(|error| backend("map OS version", error))?,
        architecture: row
            .try_get("architecture")
            .map_err(|error| backend("map architecture", error))?,
        app_version: row
            .try_get("app_version")
            .map_err(|error| backend("map app version", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map device timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map device timestamp", error))?,
    })
}

fn map_agent(row: &sqlx::sqlite::SqliteRow) -> Result<McpAgent, StorageError> {
    Ok(McpAgent {
        id: AgentId::new(
            row.try_get::<String, _>("id")
                .map_err(|error| backend("map agent id", error))?,
        )
        .map_err(invalid_data)?,
        name: row
            .try_get("name")
            .map_err(|error| backend("map agent name", error))?,
        enabled: row
            .try_get("enabled")
            .map_err(|error| backend("map agent state", error))?,
        project_folder: row
            .try_get("project_folder")
            .map_err(|error| backend("map project folder", error))?,
        secret_last4: row
            .try_get("secret_last4")
            .map_err(|error| backend("map secret suffix", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map agent timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map agent timestamp", error))?,
        last_used_at_ms: row
            .try_get("last_used_at_ms")
            .map_err(|error| backend("map last use", error))?,
    })
}

fn map_task(row: &sqlx::sqlite::SqliteRow) -> Result<Task, StorageError> {
    Ok(Task {
        id: TaskId::new(
            row.try_get::<String, _>("id")
                .map_err(|error| backend("map task id", error))?,
        )
        .map_err(invalid_data)?,
        agent_id: row
            .try_get::<Option<String>, _>("agent_id")
            .map_err(|error| backend("map task agent", error))?
            .map(AgentId::new)
            .transpose()
            .map_err(invalid_data)?,
        device_id: DeviceId::new(
            row.try_get::<String, _>("device_id")
                .map_err(|error| backend("map task device", error))?,
        )
        .map_err(invalid_data)?,
        conversation_scope_hash: row
            .try_get("conversation_scope_hash")
            .map_err(|error| backend("map conversation scope", error))?,
        title: row
            .try_get("title")
            .map_err(|error| backend("map task title", error))?,
        source: row
            .try_get("source")
            .map_err(|error| backend("map task source", error))?,
        status: TaskStatus::from_str(
            &row.try_get::<String, _>("status")
                .map_err(|error| backend("map task status", error))?,
        )
        .map_err(invalid_data)?,
        active_session_id: row
            .try_get::<Option<String>, _>("active_session_id")
            .map_err(|error| backend("map active session", error))?
            .map(SessionId::new)
            .transpose()
            .map_err(invalid_data)?,
        generation: row
            .try_get("generation")
            .map_err(|error| backend("map task generation", error))?,
        stopped_at_ms: row
            .try_get("stopped_at_ms")
            .map_err(|error| backend("map task stop time", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map task timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map task timestamp", error))?,
    })
}

fn map_chunk(row: &sqlx::sqlite::SqliteRow) -> Result<TerminalEventChunk, StorageError> {
    Ok(TerminalEventChunk {
        session_id: SessionId::new(
            row.try_get::<String, _>("session_id")
                .map_err(|error| backend("map chunk session", error))?,
        )
        .map_err(invalid_data)?,
        sequence: row
            .try_get("sequence")
            .map_err(|error| backend("map chunk sequence", error))?,
        event_id: EventId::new(
            row.try_get::<String, _>("event_id")
                .map_err(|error| backend("map chunk event", error))?,
        )
        .map_err(invalid_data)?,
        task_id: row
            .try_get::<Option<String>, _>("task_id")
            .map_err(|error| backend("map chunk task", error))?
            .map(TaskId::new)
            .transpose()
            .map_err(invalid_data)?,
        turn_id: row
            .try_get::<Option<String>, _>("turn_id")
            .map_err(|error| backend("map chunk turn", error))?
            .map(chatcmd_core::TurnId::new)
            .transpose()
            .map_err(invalid_data)?,
        kind: EventKind::from_str(
            &row.try_get::<String, _>("kind")
                .map_err(|error| backend("map chunk kind", error))?,
        )
        .map_err(invalid_data)?,
        stream: row
            .try_get("stream")
            .map_err(|error| backend("map chunk stream", error))?,
        payload: row
            .try_get("payload")
            .map_err(|error| backend("map chunk payload", error))?,
        payload_encoding: row
            .try_get("payload_encoding")
            .map_err(|error| backend("map chunk encoding", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map chunk timestamp", error))?,
    })
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
