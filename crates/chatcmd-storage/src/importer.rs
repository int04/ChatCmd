use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use chatcmd_core::{
    ActorKind, Bootstrap, EventKind, ImportReport, LegacyImport, MigrationSource,
    MigrationSourceStatus, SecretHash, StorageError,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteRepository,
    repository::{backend, now_ms},
};

/// Read-only importer for data written by pre-SQLite ChatCMD versions.
#[derive(Debug, Clone)]
pub struct LegacyImporter {
    repository: SqliteRepository,
}

impl LegacyImporter {
    #[must_use]
    pub fn new(repository: SqliteRepository) -> Self {
        Self { repository }
    }

    #[must_use]
    pub fn repository(&self) -> &SqliteRepository {
        &self.repository
    }

    async fn is_current(&self, source_key: &str, fingerprint: &str) -> Result<bool, StorageError> {
        let stored: Option<(String, String)> =
            sqlx::query_as("SELECT fingerprint,status FROM migration_sources WHERE source_key=?")
                .bind(source_key)
                .fetch_optional(self.repository.pool())
                .await
                .map_err(|error| backend("inspect legacy source", error))?;
        Ok(matches!(stored, Some((stored_fingerprint, status))
            if stored_fingerprint == fingerprint
                && matches!(status.as_str(), "imported" | "imported_with_warnings")))
    }

    async fn record_source(
        &self,
        source_key: &str,
        path: &Path,
        fingerprint: &str,
        warning_count: usize,
    ) -> Result<(), StorageError> {
        let status = if warning_count == 0 {
            "imported"
        } else {
            "imported_with_warnings"
        };
        sqlx::query("INSERT INTO migration_sources(source_key,path,fingerprint,status,warning_count,imported_at_ms,error) VALUES(?,?,?,?,?,?,NULL) ON CONFLICT(source_key) DO UPDATE SET path=excluded.path,fingerprint=excluded.fingerprint,status=excluded.status,warning_count=excluded.warning_count,imported_at_ms=excluded.imported_at_ms,error=NULL")
            .bind(source_key)
            .bind(path.to_string_lossy().as_ref())
            .bind(fingerprint)
            .bind(status)
            .bind(i64::try_from(warning_count).map_err(|error| backend("convert warning count", error))?)
            .bind(now_ms()?)
            .execute(self.repository.pool())
            .await
            .map_err(|error| backend("record legacy source", error))?;
        Ok(())
    }

    async fn import_device(&self, path: &Path, bytes: &[u8]) -> Result<Vec<String>, StorageError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            StorageError::InvalidData(format!("parse {}: {error}", path.display()))
        })?;
        let object = value.as_object().ok_or_else(|| {
            StorageError::InvalidData(format!("{} must contain a JSON object", path.display()))
        })?;
        let id = string_field(object, &["deviceId", "device_id", "id"]);
        let installation_id = string_field(object, &["installationId", "installation_id"]);
        let name = string_field(object, &["name", "deviceName", "device_name"]);
        let platform = string_field(object, &["platform", "os"]);
        let architecture = string_field(object, &["architecture", "arch"]);
        let os_version = string_field(object, &["osVersion", "os_version"]);
        let mut warnings = Vec::new();
        if id.is_none() && installation_id.is_none() && name.is_none() {
            warnings.push(format!("{}: no recognized device fields", path.display()));
            return Ok(warnings);
        }
        sqlx::query("UPDATE local_device SET device_id=COALESCE(?,device_id),installation_id=COALESCE(?,installation_id),name=COALESCE(?,name),platform=COALESCE(?,platform),os_version=COALESCE(?,os_version),architecture=COALESCE(?,architecture),updated_at_ms=? WHERE singleton_id=1")
            .bind(id)
            .bind(installation_id)
            .bind(name)
            .bind(platform)
            .bind(os_version)
            .bind(architecture)
            .bind(now_ms()?)
            .execute(self.repository.pool())
            .await
            .map_err(|error| backend("import legacy device", error))?;
        Ok(warnings)
    }

    async fn import_access(&self, path: &Path, bytes: &[u8]) -> Result<Vec<String>, StorageError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            StorageError::InvalidData(format!("parse {}: {error}", path.display()))
        })?;
        let entries: Vec<&Value> = match &value {
            Value::Array(items) => items.iter().collect(),
            Value::Object(object) => object
                .get("agents")
                .and_then(Value::as_array)
                .map_or_else(|| vec![&value], |items| items.iter().collect()),
            _ => Vec::new(),
        };
        let mut warnings = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let Some(object) = entry.as_object() else {
                warnings.push(format!(
                    "{}: access entry {} is not an object",
                    path.display(),
                    index + 1
                ));
                continue;
            };
            let Some(secret) =
                string_field(object, &["secret", "token", "accessToken", "access_token"])
            else {
                warnings.push(format!(
                    "{}: access entry {} has no secret",
                    path.display(),
                    index + 1
                ));
                continue;
            };
            let id = string_field(object, &["agentId", "agent_id", "id"])
                .unwrap_or_else(|| format!("legacy-agent-{}", index + 1));
            let name = string_field(object, &["name", "agentName", "agent_name"])
                .unwrap_or_else(|| format!("Legacy agent {}", index + 1));
            let enabled = object
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let project_folder = string_field(object, &["projectFolder", "project_folder"]);
            let digest = SecretHash::from_bearer(&secret);
            let last4 = suffix4(&secret);
            if last4.chars().count() != 4 {
                warnings.push(format!(
                    "{}: access entry {} secret is shorter than 4 characters",
                    path.display(),
                    index + 1
                ));
                continue;
            }
            let now = now_ms()?;
            sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,project_folder,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,secret_hash=excluded.secret_hash,secret_last4=excluded.secret_last4,enabled=excluded.enabled,project_folder=excluded.project_folder,updated_at_ms=excluded.updated_at_ms")
                .bind(&id)
                .bind(name)
                .bind(digest.as_bytes().as_slice())
                .bind(last4)
                .bind(enabled)
                .bind(project_folder)
                .bind(now)
                .bind(now)
                .execute(self.repository.pool())
                .await
                .map_err(|error| backend("import legacy access", error))?;
        }
        Ok(warnings)
    }

    async fn import_jsonl(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(u64, Vec<String>), StorageError> {
        let text = String::from_utf8_lossy(bytes);
        let device_id: String =
            sqlx::query_scalar("SELECT device_id FROM local_device WHERE singleton_id=1")
                .fetch_one(self.repository.pool())
                .await
                .map_err(|error| backend("read import device", error))?;
        let mut imported = 0_u64;
        let mut warnings = Vec::new();
        for (offset, line) in text.lines().enumerate() {
            let line_number = offset + 1;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(line) {
                Ok(value) => value,
                Err(error) => {
                    warnings.push(format!(
                        "{}:{line_number}: malformed JSONL: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let Some(object) = value.as_object() else {
                warnings.push(format!(
                    "{}:{line_number}: JSONL value is not an object",
                    path.display()
                ));
                continue;
            };
            let task_id = string_field(object, &["taskId", "task_id"])
                .unwrap_or_else(|| "legacy-import".to_owned());
            let event_id = string_field(object, &["eventId", "event_id", "id"])
                .unwrap_or_else(|| deterministic_id(path, line_number, line));
            let idempotency_key = string_field(object, &["idempotencyKey", "idempotency_key"])
                .unwrap_or_else(|| format!("legacy:{event_id}"));
            let created_at = integer_field(object, &["createdAtMs", "created_at_ms", "timestamp"])
                .unwrap_or(now_ms()?);
            sqlx::query("INSERT INTO tasks(id,device_id,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,'interrupted',1,?,?) ON CONFLICT(id) DO NOTHING")
                .bind(&task_id).bind(&device_id).bind(created_at).bind(created_at)
                .execute(self.repository.pool()).await.map_err(|error| backend("create imported task", error))?;
            let payload = object
                .get("payload")
                .cloned()
                .unwrap_or_else(|| value.clone());
            let payload_json = serde_json::to_string(&payload)
                .map_err(|error| backend("serialize imported event", error))?;
            let actor = string_field(object, &["actor"])
                .and_then(|raw| raw.parse::<ActorKind>().ok())
                .unwrap_or(ActorKind::System);
            let kind = string_field(object, &["kind", "type"])
                .and_then(|raw| raw.parse::<EventKind>().ok())
                .unwrap_or(EventKind::Message);
            let affected = sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,?,?,?,?)")
                .bind(&event_id).bind(&task_id).bind(actor.as_str()).bind(kind.as_str())
                .bind(idempotency_key).bind(payload_json).bind(created_at)
                .execute(self.repository.pool()).await.map_err(|error| backend("import legacy event", error))?.rows_affected();
            imported += affected;
        }
        Ok((imported, warnings))
    }

    async fn import_artifact(&self, root: &Path, path: &Path) -> Result<(), StorageError> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|error| backend("read legacy artifact metadata", error))?;
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let device_id: String =
            sqlx::query_scalar("SELECT device_id FROM local_device WHERE singleton_id=1")
                .fetch_one(self.repository.pool())
                .await
                .map_err(|error| backend("read artifact import device", error))?;
        let now = now_ms()?;
        sqlx::query("INSERT INTO tasks(id,device_id,status,generation,created_at_ms,updated_at_ms) VALUES('legacy-import',?,'interrupted',1,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(device_id).bind(now).bind(now).execute(self.repository.pool()).await
            .map_err(|error| backend("create artifact import task", error))?;
        let id = deterministic_id(
            path,
            usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            &relative,
        );
        sqlx::query("INSERT OR IGNORE INTO artifact_registry(id,task_id,relative_path,size_bytes,created_at_ms,updated_at_ms) VALUES(?,'legacy-import',?,?,?,?)")
            .bind(id).bind(relative).bind(i64::try_from(metadata.len()).map_err(|error| backend("convert artifact size", error))?)
            .bind(now).bind(now).execute(self.repository.pool()).await
            .map_err(|error| backend("import legacy artifact", error))?;
        Ok(())
    }
}

impl LegacyImport for LegacyImporter {
    async fn import_legacy(&self, root: &Path) -> Result<ImportReport, StorageError> {
        self.repository.bootstrap().await?;
        let mut report = ImportReport::default();
        for (kind, file_name) in [
            ("device", "device.json"),
            ("access", "access.json"),
            ("events", "events.jsonl"),
            ("timeline", "timeline.jsonl"),
        ] {
            let path = root.join(file_name);
            if !path
                .try_exists()
                .map_err(|error| backend("inspect legacy path", error))?
            {
                continue;
            }
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|error| backend("read legacy source", error))?;
            let fingerprint = digest_hex(&bytes);
            let source_key = format!("{kind}:{file_name}");
            if self.is_current(&source_key, &fingerprint).await? {
                report.skipped_sources += 1;
                continue;
            }
            let warnings_before = report.warnings.len();
            match kind {
                "device" => report
                    .warnings
                    .extend(self.import_device(&path, &bytes).await?),
                "access" => report
                    .warnings
                    .extend(self.import_access(&path, &bytes).await?),
                _ => {
                    let (count, warnings) = self.import_jsonl(&path, &bytes).await?;
                    report.imported_events += count;
                    report.warnings.extend(warnings);
                }
            }
            self.record_source(
                &source_key,
                &path,
                &fingerprint,
                report.warnings.len() - warnings_before,
            )
            .await?;
            report.imported_sources += 1;
        }

        let artifacts = root.join("artifacts");
        if artifacts
            .try_exists()
            .map_err(|error| backend("inspect legacy artifacts", error))?
        {
            let files = collect_files(&artifacts).await?;
            for path in files {
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|error| backend("read legacy artifact", error))?;
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let source_key = format!("artifact:{relative}");
                let fingerprint = digest_hex(&bytes);
                if self.is_current(&source_key, &fingerprint).await? {
                    report.skipped_sources += 1;
                    continue;
                }
                self.import_artifact(root, &path).await?;
                self.record_source(&source_key, &path, &fingerprint, 0)
                    .await?;
                report.imported_sources += 1;
            }
        }
        Ok(report)
    }

    async fn migration_sources(&self) -> Result<Vec<MigrationSource>, StorageError> {
        let rows = sqlx::query("SELECT source_key,path,fingerprint,status,warning_count,imported_at_ms,error FROM migration_sources ORDER BY source_key")
            .fetch_all(self.repository.pool()).await.map_err(|error| backend("list legacy sources", error))?;
        rows.iter()
            .map(|row| {
                let status: String = row
                    .try_get("status")
                    .map_err(|error| backend("map source status", error))?;
                Ok(MigrationSource {
                    source_key: row
                        .try_get("source_key")
                        .map_err(|error| backend("map source key", error))?,
                    path: row
                        .try_get("path")
                        .map_err(|error| backend("map source path", error))?,
                    fingerprint: row
                        .try_get("fingerprint")
                        .map_err(|error| backend("map source fingerprint", error))?,
                    status: status
                        .parse::<MigrationSourceStatus>()
                        .map_err(|error| StorageError::InvalidData(error.to_string()))?,
                    warning_count: row
                        .try_get("warning_count")
                        .map_err(|error| backend("map warning count", error))?,
                    imported_at_ms: row
                        .try_get("imported_at_ms")
                        .map_err(|error| backend("map import time", error))?,
                    error: row
                        .try_get("error")
                        .map_err(|error| backend("map source error", error))?,
                })
            })
            .collect()
    }
}

async fn collect_files(root: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .map_err(|error| backend("read legacy artifact directory", error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| backend("read legacy artifact entry", error))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| backend("inspect legacy artifact", error))?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn string_field(object: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str).map(str::to_owned))
}

fn integer_field(object: &serde_json::Map<String, Value>, names: &[&str]) -> Option<i64> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_i64))
}

fn suffix4(value: &str) -> String {
    value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn deterministic_id(path: &Path, ordinal: usize, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(ordinal.to_le_bytes());
    hasher.update(value.as_bytes());
    format!("legacy-{}", &digest_bytes_hex(&hasher.finalize())[..24])
}

fn digest_hex(bytes: &[u8]) -> String {
    digest_bytes_hex(&Sha256::digest(bytes))
}

fn digest_bytes_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}
