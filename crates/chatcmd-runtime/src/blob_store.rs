//! Bounded, owner-scoped storage for large tool payloads.

use crate::{
    BudgetTracker, IoResourceGovernor, OperationContext, RuntimeError, RuntimeResult, ToolBudget,
    ToolUsage,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const DEFAULT_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_INLINE_BYTES: usize = 256 * 1024;
const MIN_CHUNK_BYTES: usize = 4 * 1024;
const DEFAULT_TTL_SECONDS: u64 = 30 * 60;
const MAX_TTL_SECONDS: u64 = 24 * 60 * 60;
const MAX_BLOB_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_OWNER_BYTES: u64 = 2 * MAX_BLOB_BYTES;
const MAX_GLOBAL_BYTES: u64 = 8 * MAX_BLOB_BYTES;
const MAX_OWNER_UPLOADS: usize = 16;
const BLOB_OPERATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_BLOB_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_BLOB_MAX_OPEN_FILES: u32 = 4;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobToolBudget {
    #[serde(default = "default_blob_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_blob_max_bytes")]
    pub max_bytes_read: u64,
    #[serde(default = "default_blob_max_bytes")]
    pub max_bytes_written: u64,
    #[serde(default = "default_blob_max_open_files")]
    pub max_open_files: u32,
}

impl Default for BlobToolBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_blob_timeout_ms(),
            max_bytes_read: default_blob_max_bytes(),
            max_bytes_written: default_blob_max_bytes(),
            max_open_files: default_blob_max_open_files(),
        }
    }
}

const fn default_blob_timeout_ms() -> u64 {
    DEFAULT_BLOB_TIMEOUT_MS
}
const fn default_blob_max_bytes() -> u64 {
    MAX_BLOB_BYTES
}
const fn default_blob_max_open_files() -> u32 {
    DEFAULT_BLOB_MAX_OPEN_FILES
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobBeginRequest {
    pub purpose: String,
    #[serde(default)]
    pub expected_size_bytes: Option<u64>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub chunk_size_bytes: Option<usize>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub budget: BlobToolBudget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobBeginResult {
    pub upload_id: String,
    pub content_ref: String,
    pub chunk_size_bytes: usize,
    pub expires_at_ms: u64,
    pub max_size_bytes: u64,
    pub usage: ToolUsage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobChunkRequest {
    pub upload_id: String,
    pub offset: u64,
    pub data_base64: String,
    #[serde(default)]
    pub chunk_sha256: Option<String>,
    #[serde(default)]
    pub budget: BlobToolBudget,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobSealRequest {
    pub upload_id: String,
    pub final_size_bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub budget: BlobToolBudget,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobIdRequest {
    pub upload_id: String,
    #[serde(default)]
    pub budget: BlobToolBudget,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlobStatus {
    pub upload_id: String,
    pub content_ref: String,
    pub purpose: String,
    pub state: String,
    pub received_size_bytes: u64,
    pub next_offset: u64,
    pub expected_size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub expires_at_ms: u64,
    pub usage: ToolUsage,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedArtifactRef {
    pub content_ref: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedArtifactRead {
    pub content: String,
    pub truncated: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub expires_at_ms: u64,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub usage: ToolUsage,
}

#[derive(Clone)]
pub struct BlobStore {
    root: Arc<PathBuf>,
    entries: Arc<Mutex<HashMap<String, BlobEntry>>>,
    io_resources: IoResourceGovernor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct BlobOwner {
    agent_id: String,
    task_id: Option<String>,
    turn_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum BlobState {
    Uploading,
    Sealed,
    Consuming,
    Consumed,
    Aborted,
}

#[derive(Debug)]
struct BlobEntry {
    upload_id: String,
    content_ref: String,
    owner: BlobOwner,
    purpose: String,
    content_type: Option<String>,
    expected_size: Option<u64>,
    expected_sha256: Option<String>,
    chunk_size: usize,
    expires_at_ms: u64,
    path: PathBuf,
    received: u64,
    chunks: BTreeMap<u64, (u64, String)>,
    sha256: Option<String>,
    state: BlobState,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlobMetadata {
    upload_id: String,
    content_ref: String,
    owner: BlobOwner,
    purpose: String,
    content_type: Option<String>,
    expected_size: Option<u64>,
    expected_sha256: Option<String>,
    chunk_size: usize,
    expires_at_ms: u64,
    received: u64,
    chunks: BTreeMap<u64, (u64, String)>,
    sha256: Option<String>,
    state: BlobState,
}

pub struct BlobLease {
    store: BlobStore,
    upload_id: String,
    path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    finished: bool,
}

impl BlobLease {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn finish(mut self, consumed: bool) -> RuntimeResult<()> {
        self.store.finish_consume(&self.upload_id, consumed)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for BlobLease {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.store.finish_consume(&self.upload_id, false);
        }
    }
}

impl BlobStore {
    /// Returns the bounded in-memory byte count without exposing blob metadata.
    #[must_use]
    pub fn usage_bytes(&self) -> u64 {
        self.entries.lock().map_or(0, |entries| {
            entries
                .values()
                .filter(|entry| !matches!(entry.state, BlobState::Consumed | BlobState::Aborted))
                .fold(0_u64, |total, entry| total.saturating_add(entry.received))
        })
    }

    fn acquire_open_files(
        &self,
        budget: &BlobToolBudget,
        required: u32,
    ) -> RuntimeResult<tokio::sync::OwnedSemaphorePermit> {
        if budget.max_open_files < required {
            return Err(RuntimeError::new(
                "openFileBudgetExceeded",
                format!("blob operation requires at least {required} open-file slots"),
            ));
        }
        self.io_resources.try_open_files(required)
    }

    pub fn new(root: PathBuf) -> RuntimeResult<Self> {
        fs::create_dir_all(&root).map_err(io_error)?;
        let entries = recover_entries(&root)?;
        Ok(Self {
            root: Arc::new(root),
            entries: Arc::new(Mutex::new(entries)),
            io_resources: IoResourceGovernor::new(128, MAX_GLOBAL_BYTES),
        })
    }

    pub fn temporary() -> RuntimeResult<Self> {
        Self::new(std::env::temp_dir().join("chatcmd-blobs-v1"))
    }

    /// Stores an immutable, owner-scoped JSON artifact without Base64 expansion or an
    /// additional in-memory serialized copy. The payload is serialized directly to a
    /// temporary file, hashed, quota-checked, and atomically published before metadata
    /// becomes visible.
    pub fn store_artifact_json(
        &self,
        context: &OperationContext,
        value: &Value,
        ttl_seconds: u64,
    ) -> RuntimeResult<ManagedArtifactRef> {
        let mut counter = JsonByteCounter::default();
        serde_json::to_writer(&mut counter, value)
            .map_err(|error| RuntimeError::new("artifactSerializationFailed", error.to_string()))?;
        let size_bytes = u64::try_from(counter.bytes)
            .map_err(|_| quota_error("artifact size cannot be represented"))?;
        if size_bytes > MAX_BLOB_BYTES {
            return Err(quota_error("artifact exceeds the maximum size"));
        }
        let _open_files = self.io_resources.try_open_files(2)?;
        let _disk_reservation = self.io_resources.try_reserve_disk(size_bytes)?;

        let ttl_seconds = ttl_seconds.clamp(1, MAX_TTL_SECONDS);
        let upload_id = Uuid::new_v4().to_string();
        let content_ref = format!("blob:v1:{}", Uuid::new_v4());
        let target = self.root.join(format!("{upload_id}.blob"));
        let temporary = self
            .root
            .join(format!("{upload_id}.blob.tmp-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(io_error)?;
        if let Err(error) = serde_json::to_writer(&mut file, value)
            .map_err(|error| RuntimeError::new("artifactSerializationFailed", error.to_string()))
            .and_then(|_| file.flush().map_err(io_error))
            .and_then(|_| file.sync_all().map_err(io_error))
        {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        let sha256 = match sha256_file(&temporary) {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let expires_at_ms = now_ms().saturating_add(ttl_seconds.saturating_mul(1000));
        let owner = owner(context);
        let entry = BlobEntry {
            upload_id: upload_id.clone(),
            content_ref: content_ref.clone(),
            owner: owner.clone(),
            purpose: "artifact".to_owned(),
            content_type: Some("application/json".to_owned()),
            expected_size: Some(size_bytes),
            expected_sha256: Some(sha256.clone()),
            chunk_size: DEFAULT_CHUNK_BYTES,
            expires_at_ms,
            path: target.clone(),
            received: size_bytes,
            chunks: BTreeMap::new(),
            sha256: Some(sha256.clone()),
            state: BlobState::Sealed,
        };

        let mut entries = lock(&self.entries)?;
        purge_expired(&self.root, &mut entries);
        let (owner_bytes, owner_uploads, global_bytes) = usage(&entries, &owner);
        if owner_uploads >= MAX_OWNER_UPLOADS
            || owner_bytes.saturating_add(size_bytes) > MAX_OWNER_BYTES
            || global_bytes.saturating_add(size_bytes) > MAX_GLOBAL_BYTES
        {
            let _ = fs::remove_file(&temporary);
            return Err(quota_error("artifact quota exceeded"));
        }
        if let Err(error) = fs::rename(&temporary, &target).map_err(io_error) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = persist_entry(&self.root, &entry) {
            let _ = fs::remove_file(&target);
            return Err(error);
        }
        entries.insert(upload_id, entry);
        Ok(ManagedArtifactRef {
            content_ref,
            size_bytes,
            sha256,
            expires_at_ms,
        })
    }

    /// Imports an existing file into immutable, owner-scoped artifact storage without
    /// loading the whole file into memory. The source remains untouched so callers can
    /// delete temporary producer output only after this method succeeds.
    pub fn store_artifact_file(
        &self,
        context: &OperationContext,
        source: &Path,
        content_type: Option<String>,
        ttl_seconds: u64,
    ) -> RuntimeResult<ManagedArtifactRef> {
        let size_bytes = fs::metadata(source).map_err(io_error)?.len();
        if size_bytes > MAX_BLOB_BYTES {
            return Err(quota_error("artifact exceeds the maximum size"));
        }
        let _open_files = self.io_resources.try_open_files(2)?;
        let _disk_reservation = self.io_resources.try_reserve_disk(size_bytes)?;
        let sha256 = sha256_file(source)?;
        let ttl_seconds = ttl_seconds.clamp(1, MAX_TTL_SECONDS);
        let upload_id = Uuid::new_v4().to_string();
        let content_ref = format!("blob:v1:{}", Uuid::new_v4());
        let target = self.root.join(format!("{upload_id}.blob"));
        let temporary = self
            .root
            .join(format!("{upload_id}.blob.tmp-{}", Uuid::new_v4()));
        let owner = owner(context);
        let expires_at_ms = now_ms().saturating_add(ttl_seconds.saturating_mul(1000));
        let entry = BlobEntry {
            upload_id: upload_id.clone(),
            content_ref: content_ref.clone(),
            owner: owner.clone(),
            purpose: "artifact".to_owned(),
            content_type,
            expected_size: Some(size_bytes),
            expected_sha256: Some(sha256.clone()),
            chunk_size: DEFAULT_CHUNK_BYTES,
            expires_at_ms,
            path: target.clone(),
            received: size_bytes,
            chunks: BTreeMap::new(),
            sha256: Some(sha256.clone()),
            state: BlobState::Sealed,
        };

        let mut entries = lock(&self.entries)?;
        purge_expired(&self.root, &mut entries);
        let (owner_bytes, owner_uploads, global_bytes) = usage(&entries, &owner);
        if owner_uploads >= MAX_OWNER_UPLOADS
            || owner_bytes.saturating_add(size_bytes) > MAX_OWNER_BYTES
            || global_bytes.saturating_add(size_bytes) > MAX_GLOBAL_BYTES
        {
            return Err(quota_error("artifact quota exceeded"));
        }
        if let Err(error) = fs::copy(source, &temporary).map_err(io_error) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if fs::metadata(&temporary).map_err(io_error)?.len() != size_bytes
            || sha256_file(&temporary)? != sha256
        {
            let _ = fs::remove_file(&temporary);
            return Err(RuntimeError::new(
                "artifactIntegrityMismatch",
                "artifact source changed while being imported",
            ));
        }
        if let Err(error) = fs::rename(&temporary, &target).map_err(io_error) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = persist_entry(&self.root, &entry) {
            let _ = fs::remove_file(&target);
            return Err(error);
        }
        entries.insert(upload_id, entry);
        Ok(ManagedArtifactRef {
            content_ref,
            size_bytes,
            sha256,
            expires_at_ms,
        })
    }

    /// Lazily reads an immutable managed artifact from the beginning.
    pub fn read_artifact_text(
        &self,
        context: &OperationContext,
        content_ref: &str,
        max_bytes: usize,
    ) -> RuntimeResult<ManagedArtifactRead> {
        self.read_artifact_text_range(context, content_ref, 0, max_bytes)
    }

    /// Reads one bounded byte range from a managed artifact. The next offset is explicit so
    /// callers can page through very large Git output without reloading earlier bytes.
    pub fn read_artifact_text_range(
        &self,
        context: &OperationContext,
        content_ref: &str,
        offset: u64,
        max_bytes: usize,
    ) -> RuntimeResult<ManagedArtifactRead> {
        let tracker = internal_blob_tracker(context);
        tracker.set_phase("readingArtifact");
        tracker.checkpoint()?;
        let _open_files = self.io_resources.try_open_files(1)?;
        let max_bytes = max_bytes.clamp(1, MAX_INLINE_BYTES);
        let (path, size_bytes, sha256, expires_at_ms) = {
            let mut entries = lock(&self.entries)?;
            let entry = entries
                .values_mut()
                .find(|entry| entry.content_ref == content_ref)
                .ok_or_else(|| RuntimeError::new("artifact_not_found", "artifact was not found"))?;
            check_artifact_owner_and_expiry(entry, context)?;
            if entry.purpose != "artifact" || entry.state != BlobState::Sealed {
                return Err(RuntimeError::new(
                    "artifact_not_found",
                    "artifact is not available for lazy reading",
                ));
            }
            (
                entry.path.clone(),
                entry.received,
                entry.sha256.clone().unwrap_or_default(),
                entry.expires_at_ms,
            )
        };
        if offset > size_bytes {
            return Err(RuntimeError::new(
                "artifact_range_invalid",
                "artifact offset exceeds artifact size",
            ));
        }
        let mut file = File::open(path).map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let take_limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(4);
        let mut reader = BufReader::with_capacity(64 * 1024, file).take(take_limit);
        let mut bytes = Vec::with_capacity(max_bytes.min(256 * 1024));
        reader.read_to_end(&mut bytes).map_err(io_error)?;
        tracker.consume_read_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
        tracker.checkpoint()?;
        let available = size_bytes.saturating_sub(offset);
        let truncated = available > u64::try_from(max_bytes).unwrap_or(u64::MAX);
        if bytes.len() > max_bytes {
            bytes.truncate(max_bytes);
        }
        while std::str::from_utf8(&bytes).is_err() && !bytes.is_empty() {
            bytes.pop();
        }
        let consumed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let next = offset.saturating_add(consumed);
        let next_offset = (next < size_bytes).then_some(next);
        let content = String::from_utf8(bytes).map_err(|_| {
            RuntimeError::new("artifactCorrupt", "managed artifact is not valid UTF-8")
        })?;
        Ok(ManagedArtifactRead {
            content,
            truncated,
            size_bytes,
            sha256,
            expires_at_ms,
            offset,
            next_offset,
            usage: tracker.finish_usage().into(),
        })
    }

    pub fn begin(
        &self,
        context: &OperationContext,
        request: BlobBeginRequest,
    ) -> RuntimeResult<BlobBeginResult> {
        let tracker = blob_tracker(context, &request.budget);
        tracker.set_phase("reservingBlob");
        tracker.checkpoint()?;
        let _open_files = self.acquire_open_files(&request.budget, 1)?;
        validate_purpose(&request.purpose)?;
        if request
            .expected_size_bytes
            .is_some_and(|size| size > MAX_BLOB_BYTES)
        {
            return Err(quota_error("blob exceeds the maximum size"));
        }
        let chunk_size = request
            .chunk_size_bytes
            .unwrap_or(DEFAULT_CHUNK_BYTES)
            .clamp(MIN_CHUNK_BYTES, MAX_CHUNK_BYTES);
        let ttl = request
            .ttl_seconds
            .unwrap_or(DEFAULT_TTL_SECONDS)
            .clamp(1, MAX_TTL_SECONDS);
        let owner = owner(context);
        let mut entries = lock(&self.entries)?;
        purge_expired(&self.root, &mut entries);
        let (owner_bytes, owner_uploads, global_bytes) = usage(&entries, &owner);
        let reserved = request.expected_size_bytes.unwrap_or(MAX_BLOB_BYTES);
        if owner_uploads >= MAX_OWNER_UPLOADS
            || owner_bytes.saturating_add(reserved) > MAX_OWNER_BYTES
            || global_bytes.saturating_add(reserved) > MAX_GLOBAL_BYTES
        {
            return Err(quota_error("blob upload quota exceeded"));
        }
        let upload_id = Uuid::new_v4().to_string();
        let content_ref = format!("blob:v1:{}", Uuid::new_v4());
        let path = self.root.join(format!("{upload_id}.blob"));
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(io_error)?;
        let expires_at_ms = now_ms().saturating_add(ttl.saturating_mul(1000));
        let entry = BlobEntry {
            upload_id: upload_id.clone(),
            content_ref: content_ref.clone(),
            owner,
            purpose: request.purpose,
            content_type: request.content_type,
            expected_size: request.expected_size_bytes,
            expected_sha256: normalize_hash(request.expected_sha256.as_deref())?,
            chunk_size,
            expires_at_ms,
            path,
            received: 0,
            chunks: BTreeMap::new(),
            sha256: None,
            state: BlobState::Uploading,
        };
        if let Err(error) = persist_entry(&self.root, &entry) {
            let _ = fs::remove_file(&entry.path);
            return Err(error);
        }
        entries.insert(upload_id.clone(), entry);
        tracker.checkpoint()?;
        Ok(BlobBeginResult {
            upload_id,
            content_ref,
            chunk_size_bytes: chunk_size,
            expires_at_ms,
            max_size_bytes: MAX_BLOB_BYTES,
            usage: tracker.finish_usage().into(),
        })
    }

    pub fn write_chunk(
        &self,
        context: &OperationContext,
        request: BlobChunkRequest,
    ) -> RuntimeResult<BlobStatus> {
        let tracker = blob_tracker(context, &request.budget);
        tracker.set_phase("decodingChunk");
        tracker.checkpoint()?;
        let _open_files = self.acquire_open_files(&request.budget, 2)?;
        let bytes = STANDARD
            .decode(&request.data_base64)
            .map_err(|_| RuntimeError::new("invalid_base64", "blob chunk is not valid Base64"))?;
        let hash = sha256_hex(&bytes);
        if let Some(expected) = normalize_hash(request.chunk_sha256.as_deref())?
            && expected != hash
        {
            return Err(RuntimeError::new(
                "blobIntegrityMismatch",
                "blob chunk SHA-256 does not match",
            ));
        }
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, &request.upload_id)?;
        if entry.state != BlobState::Uploading {
            return Err(state_error("blob is not accepting chunks"));
        }
        if bytes.len() > entry.chunk_size {
            return Err(RuntimeError::new(
                "blobChunkTooLarge",
                "chunk exceeds negotiated chunkSizeBytes",
            ));
        }
        if request.offset < entry.received {
            return match entry.chunks.get(&request.offset) {
                Some((length, old_hash))
                    if *length == u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                        && old_hash == &hash =>
                {
                    Ok(status(entry, tracker.finish_usage().into()))
                }
                _ => Err(RuntimeError::new(
                    "blobChunkConflict",
                    "chunk offset was already written with different bytes",
                )),
            };
        }
        if request.offset != entry.received {
            return Err(RuntimeError::new(
                "blobOffsetMismatch",
                format!("expected next offset {}", entry.received),
            ));
        }
        let new_size = entry
            .received
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| quota_error("blob size overflow"))?;
        let maximum = entry.expected_size.unwrap_or(MAX_BLOB_BYTES);
        if new_size > maximum || new_size > MAX_BLOB_BYTES {
            return Err(quota_error("chunk would exceed the blob size limit"));
        }
        let _disk_reservation = self
            .io_resources
            .try_reserve_disk(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
        tracker.set_phase("writingChunk");
        tracker.consume_write_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
        let mut file = OpenOptions::new()
            .append(true)
            .open(&entry.path)
            .map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.flush().map_err(io_error)?;
        entry.chunks.insert(
            request.offset,
            (u64::try_from(bytes.len()).unwrap_or(u64::MAX), hash),
        );
        entry.received = new_size;
        persist_entry(&self.root, entry)?;
        tracker.checkpoint()?;
        Ok(status(entry, tracker.finish_usage().into()))
    }

    pub fn status(&self, context: &OperationContext, upload_id: &str) -> RuntimeResult<BlobStatus> {
        self.status_with_budget(context, upload_id, &BlobToolBudget::default())
    }

    pub fn status_with_budget(
        &self,
        context: &OperationContext,
        upload_id: &str,
        budget: &BlobToolBudget,
    ) -> RuntimeResult<BlobStatus> {
        let tracker = blob_tracker(context, budget);
        tracker.set_phase("readingBlobStatus");
        tracker.checkpoint()?;
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, upload_id)?;
        Ok(status(entry, tracker.finish_usage().into()))
    }

    pub fn seal(
        &self,
        context: &OperationContext,
        request: BlobSealRequest,
    ) -> RuntimeResult<BlobStatus> {
        let tracker = blob_tracker(context, &request.budget);
        tracker.set_phase("verifyingBlob");
        tracker.checkpoint()?;
        let _open_files = self.acquire_open_files(&request.budget, 1)?;
        let expected_hash = normalize_hash(Some(&request.sha256))?.ok_or_else(|| {
            RuntimeError::new("blobIntegrityRequired", "final SHA-256 is required")
        })?;
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, &request.upload_id)?;
        if entry.state == BlobState::Sealed && entry.sha256.as_deref() == Some(&expected_hash) {
            return Ok(status(entry, tracker.finish_usage().into()));
        }
        if entry.state != BlobState::Uploading {
            return Err(state_error("blob cannot be sealed from its current state"));
        }
        if request.final_size_bytes != entry.received
            || entry
                .expected_size
                .is_some_and(|size| size != entry.received)
        {
            return Err(RuntimeError::new(
                "blobSizeMismatch",
                "final blob size does not match uploaded or expected size",
            ));
        }
        let actual_hash = sha256_file_tracked(&entry.path, &tracker)?;
        if actual_hash != expected_hash
            || entry
                .expected_sha256
                .as_ref()
                .is_some_and(|hash| hash != &actual_hash)
        {
            return Err(RuntimeError::new(
                "blobIntegrityMismatch",
                "final blob SHA-256 does not match",
            ));
        }
        entry.sha256 = Some(actual_hash);
        entry.state = BlobState::Sealed;
        persist_entry(&self.root, entry)?;
        tracker.checkpoint()?;
        Ok(status(entry, tracker.finish_usage().into()))
    }

    pub fn lease(
        &self,
        context: &OperationContext,
        content_ref: &str,
        purpose: &str,
    ) -> RuntimeResult<BlobLease> {
        let mut entries = lock(&self.entries)?;
        let entry = entries
            .values_mut()
            .find(|entry| entry.content_ref == content_ref)
            .ok_or_else(|| RuntimeError::new("blobNotFound", "contentRef was not found"))?;
        check_owner_and_expiry(entry, context)?;
        if entry.purpose != purpose {
            return Err(RuntimeError::new(
                "blobPurposeMismatch",
                "contentRef purpose does not match this tool",
            ));
        }
        if entry.state != BlobState::Sealed {
            return Err(state_error(
                "contentRef is not sealed or is already consumed",
            ));
        }
        entry.state = BlobState::Consuming;
        persist_entry(&self.root, entry)?;
        Ok(BlobLease {
            store: self.clone(),
            upload_id: entry.upload_id.clone(),
            path: entry.path.clone(),
            size_bytes: entry.received,
            sha256: entry.sha256.clone().unwrap_or_default(),
            finished: false,
        })
    }

    pub fn abort(&self, context: &OperationContext, upload_id: &str) -> RuntimeResult<BlobStatus> {
        self.abort_with_budget(context, upload_id, &BlobToolBudget::default())
    }

    pub fn abort_with_budget(
        &self,
        context: &OperationContext,
        upload_id: &str,
        budget: &BlobToolBudget,
    ) -> RuntimeResult<BlobStatus> {
        let tracker = blob_tracker(context, budget);
        tracker.set_phase("abortingBlob");
        tracker.checkpoint()?;
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, upload_id)?;
        if entry.state != BlobState::Consumed {
            entry.state = BlobState::Aborted;
            let _ = fs::remove_file(&entry.path);
            let _ = fs::remove_file(metadata_path(&self.root, &entry.upload_id));
        }
        Ok(status(entry, tracker.finish_usage().into()))
    }

    pub fn gc(&self) -> RuntimeResult<usize> {
        let mut entries = lock(&self.entries)?;
        let before = entries.len();
        purge_expired(&self.root, &mut entries);
        Ok(before.saturating_sub(entries.len()))
    }

    /// Removes every tracked blob owned by a deleted task.
    pub fn cleanup_task(&self, task_id: &str) -> RuntimeResult<usize> {
        let mut entries = lock(&self.entries)?;
        let mut removed_entries = Vec::new();
        entries.retain(|_, entry| {
            let remove = entry.owner.task_id.as_deref() == Some(task_id);
            if remove {
                removed_entries.push((entry.upload_id.clone(), entry.path.clone()));
            }
            !remove
        });
        let removed = removed_entries.len();
        for (upload_id, path) in removed_entries {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(metadata_path(&self.root, &upload_id));
        }
        Ok(removed)
    }

    /// Removes all tracked blob bytes after a successful full user-data purge.
    pub fn cleanup_all(&self) -> RuntimeResult<usize> {
        let mut entries = lock(&self.entries)?;
        let removed = entries.len();
        for entry in entries.values() {
            let _ = fs::remove_file(&entry.path);
            let _ = fs::remove_file(metadata_path(&self.root, &entry.upload_id));
        }
        entries.clear();
        Ok(removed)
    }

    fn finish_consume(&self, upload_id: &str, consumed: bool) -> RuntimeResult<()> {
        let mut entries = lock(&self.entries)?;
        let entry = entries
            .get_mut(upload_id)
            .ok_or_else(|| RuntimeError::new("blobNotFound", "blob upload was not found"))?;
        if entry.state == BlobState::Consuming {
            entry.state = if consumed {
                BlobState::Consumed
            } else {
                BlobState::Sealed
            };
            if consumed {
                let _ = fs::remove_file(&entry.path);
                let _ = fs::remove_file(metadata_path(&self.root, &entry.upload_id));
            } else {
                persist_entry(&self.root, entry)?;
            }
        }
        Ok(())
    }
}

fn blob_tracker(context: &OperationContext, requested: &BlobToolBudget) -> BudgetTracker {
    let hard_timeout_ms = u64::try_from(BLOB_OPERATION_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
    let timeout_ms = requested.timeout_ms.min(hard_timeout_ms).max(1);
    BudgetTracker::new(
        context.cancellation.clone(),
        ToolBudget {
            max_bytes_read: Some(requested.max_bytes_read.min(MAX_BLOB_BYTES)),
            max_bytes_written: Some(requested.max_bytes_written.min(MAX_BLOB_BYTES)),
            max_output_bytes: Some(MAX_INLINE_BYTES as u64),
            max_open_files: Some(requested.max_open_files.min(128)),
            memory_reservation_bytes: Some(MAX_CHUNK_BYTES as u64),
            ..ToolBudget::default()
        }
        .with_timeout(Duration::from_millis(timeout_ms)),
    )
}

fn internal_blob_tracker(context: &OperationContext) -> BudgetTracker {
    blob_tracker(context, &BlobToolBudget::default())
}

fn validate_purpose(purpose: &str) -> RuntimeResult<()> {
    if matches!(
        purpose,
        "fsWriteText" | "fsWriteRaw" | "fsApplyEdits" | "artifact"
    ) {
        Ok(())
    } else {
        Err(RuntimeError::new(
            "invalidBlobPurpose",
            "purpose must be fsWriteText, fsWriteRaw, fsApplyEdits, or artifact",
        ))
    }
}

fn owner(context: &OperationContext) -> BlobOwner {
    BlobOwner {
        agent_id: context.agent_id.clone(),
        task_id: context.task_id.clone(),
        turn_id: context.turn_id.clone(),
    }
}

fn checked_entry_mut<'a>(
    entries: &'a mut HashMap<String, BlobEntry>,
    context: &OperationContext,
    upload_id: &str,
) -> RuntimeResult<&'a mut BlobEntry> {
    let entry = entries
        .get_mut(upload_id)
        .ok_or_else(|| RuntimeError::new("blobNotFound", "blob upload was not found"))?;
    check_owner_and_expiry(entry, context)?;
    Ok(entry)
}

fn check_owner_and_expiry(entry: &mut BlobEntry, context: &OperationContext) -> RuntimeResult<()> {
    if entry.owner != owner(context) {
        return Err(RuntimeError::new(
            "blobAccessDenied",
            "blob belongs to a different agent, task, or turn",
        ));
    }
    if entry.expires_at_ms <= now_ms() {
        entry.state = BlobState::Aborted;
        let _ = fs::remove_file(&entry.path);
        return Err(RuntimeError::new("blobExpired", "blob has expired"));
    }
    Ok(())
}

fn check_artifact_owner_and_expiry(
    entry: &mut BlobEntry,
    context: &OperationContext,
) -> RuntimeResult<()> {
    let same_agent = entry.owner.agent_id == context.agent_id;
    let same_task = entry.owner.task_id.as_deref() == context.task_id.as_deref();
    if !same_agent || !same_task || context.task_id.is_none() {
        return Err(RuntimeError::new(
            "artifactAccessDenied",
            "artifact belongs to a different agent or task",
        ));
    }
    if entry.expires_at_ms <= now_ms() {
        entry.state = BlobState::Aborted;
        let _ = fs::remove_file(&entry.path);
        return Err(RuntimeError::new("artifactExpired", "artifact has expired"));
    }
    Ok(())
}

fn usage(entries: &HashMap<String, BlobEntry>, owner: &BlobOwner) -> (u64, usize, u64) {
    let active = entries.values().filter(|entry| {
        matches!(
            entry.state,
            BlobState::Uploading | BlobState::Sealed | BlobState::Consuming
        )
    });
    let mut owner_bytes = 0_u64;
    let mut owner_uploads = 0_usize;
    let mut global_bytes = 0_u64;
    for entry in active {
        let reserved = entry.expected_size.unwrap_or(MAX_BLOB_BYTES);
        global_bytes = global_bytes.saturating_add(reserved);
        if &entry.owner == owner {
            owner_bytes = owner_bytes.saturating_add(reserved);
            owner_uploads += 1;
        }
    }
    (owner_bytes, owner_uploads, global_bytes)
}

fn metadata_path(root: &Path, upload_id: &str) -> PathBuf {
    root.join(format!("{upload_id}.meta.json"))
}

fn metadata_from_entry(entry: &BlobEntry) -> BlobMetadata {
    BlobMetadata {
        upload_id: entry.upload_id.clone(),
        content_ref: entry.content_ref.clone(),
        owner: entry.owner.clone(),
        purpose: entry.purpose.clone(),
        content_type: entry.content_type.clone(),
        expected_size: entry.expected_size,
        expected_sha256: entry.expected_sha256.clone(),
        chunk_size: entry.chunk_size,
        expires_at_ms: entry.expires_at_ms,
        received: entry.received,
        chunks: entry.chunks.clone(),
        sha256: entry.sha256.clone(),
        state: entry.state,
    }
}

fn persist_entry(root: &Path, entry: &BlobEntry) -> RuntimeResult<()> {
    let bytes = serde_json::to_vec(&metadata_from_entry(entry))
        .map_err(|error| RuntimeError::new("blobMetadataError", error.to_string()))?;
    let target = metadata_path(root, &entry.upload_id);
    let temporary = root.join(format!("{}.meta.tmp-{}", entry.upload_id, Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io_error)?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, &target))
    {
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    Ok(())
}

fn recover_entries(root: &Path) -> RuntimeResult<HashMap<String, BlobEntry>> {
    let mut entries = HashMap::new();
    let now = now_ms();
    for item in fs::read_dir(root).map_err(io_error)? {
        let path = item.map_err(io_error)?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.contains(".meta.tmp-") || name.contains(".blob.tmp-") {
            let _ = fs::remove_file(path);
            continue;
        }
        if !name.ends_with(".meta.json") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                let _ = fs::remove_file(path);
                continue;
            }
        };
        let mut metadata = match serde_json::from_slice::<BlobMetadata>(&bytes) {
            Ok(metadata) => metadata,
            Err(_) => {
                let _ = fs::remove_file(path);
                continue;
            }
        };
        let blob_path = root.join(format!("{}.blob", metadata.upload_id));
        if metadata.expires_at_ms <= now
            || matches!(metadata.state, BlobState::Aborted | BlobState::Consumed)
            || !blob_path.is_file()
            || !metadata.content_ref.starts_with("blob:v1:")
            || validate_purpose(&metadata.purpose).is_err()
        {
            let _ = fs::remove_file(&blob_path);
            let _ = fs::remove_file(&path);
            continue;
        }
        let disk_len = match fs::metadata(&blob_path) {
            Ok(value) => value.len(),
            Err(_) => {
                let _ = fs::remove_file(&path);
                continue;
            }
        };
        if disk_len < metadata.received {
            let _ = fs::remove_file(&blob_path);
            let _ = fs::remove_file(&path);
            continue;
        }
        if disk_len > metadata.received
            && OpenOptions::new()
                .write(true)
                .open(&blob_path)
                .and_then(|file| file.set_len(metadata.received))
                .is_err()
        {
            let _ = fs::remove_file(&blob_path);
            let _ = fs::remove_file(&path);
            continue;
        }
        if metadata.state == BlobState::Consuming {
            metadata.state = BlobState::Sealed;
        }
        let entry = BlobEntry {
            upload_id: metadata.upload_id.clone(),
            content_ref: metadata.content_ref,
            owner: metadata.owner,
            purpose: metadata.purpose,
            content_type: metadata.content_type,
            expected_size: metadata.expected_size,
            expected_sha256: metadata.expected_sha256,
            chunk_size: metadata.chunk_size.clamp(MIN_CHUNK_BYTES, MAX_CHUNK_BYTES),
            expires_at_ms: metadata.expires_at_ms,
            path: blob_path,
            received: metadata.received,
            chunks: metadata.chunks,
            sha256: metadata.sha256,
            state: metadata.state,
        };
        persist_entry(root, &entry)?;
        entries.insert(entry.upload_id.clone(), entry);
    }

    for item in fs::read_dir(root).map_err(io_error)? {
        let path = item.map_err(io_error)?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("blob") {
            continue;
        }
        let keep = entries.values().any(|entry| entry.path == path);
        if !keep {
            let _ = fs::remove_file(path);
        }
    }
    Ok(entries)
}

fn purge_expired(root: &Path, entries: &mut HashMap<String, BlobEntry>) {
    let now = now_ms();
    entries.retain(|_, entry| {
        let keep = entry.expires_at_ms > now
            && !matches!(entry.state, BlobState::Aborted | BlobState::Consumed);
        if !keep {
            let _ = fs::remove_file(&entry.path);
            let _ = fs::remove_file(metadata_path(root, &entry.upload_id));
        }
        keep
    });
}

fn status(entry: &BlobEntry, usage: ToolUsage) -> BlobStatus {
    let _content_type = &entry.content_type;
    BlobStatus {
        upload_id: entry.upload_id.clone(),
        content_ref: entry.content_ref.clone(),
        purpose: entry.purpose.clone(),
        state: match entry.state {
            BlobState::Uploading => "uploading",
            BlobState::Sealed => "sealed",
            BlobState::Consuming => "consuming",
            BlobState::Consumed => "consumed",
            BlobState::Aborted => "aborted",
        }
        .to_owned(),
        received_size_bytes: entry.received,
        next_offset: entry.received,
        expected_size_bytes: entry.expected_size,
        sha256: entry.sha256.clone(),
        expires_at_ms: entry.expires_at_ms,
        usage,
    }
}

#[derive(Default)]
struct JsonByteCounter {
    bytes: usize,
}

impl Write for JsonByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sha256_file(path: &Path) -> RuntimeResult<String> {
    let file = File::open(path).map_err(io_error)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    loop {
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_file_tracked(path: &Path, tracker: &BudgetTracker) -> RuntimeResult<String> {
    let file = File::open(path).map_err(io_error)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    loop {
        tracker.checkpoint()?;
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        tracker.consume_read_bytes(u64::try_from(read).unwrap_or(u64::MAX))?;
        hasher.update(&buffer[..read]);
    }
    tracker.checkpoint()?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn normalize_hash(value: Option<&str>) -> RuntimeResult<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    let value = value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .to_ascii_lowercase();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RuntimeError::new(
            "invalidSha256",
            "SHA-256 must contain exactly 64 hexadecimal characters",
        ));
    }
    Ok(Some(value))
}

fn lock<T>(mutex: &Mutex<T>) -> RuntimeResult<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| RuntimeError::new("blobStoreUnavailable", "blob store lock was poisoned"))
}

fn state_error(message: &str) -> RuntimeError {
    RuntimeError::new("blobStateConflict", message)
}

fn quota_error(message: &str) -> RuntimeError {
    RuntimeError::new("blobQuotaExceeded", message)
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("blobIoError", error.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn context(agent: &str) -> OperationContext {
        let mut context = OperationContext::new("request", agent, "blob");
        context.task_id = Some("task".into());
        context.turn_id = Some("turn".into());
        context
    }

    #[test]
    fn cancelled_blob_operation_returns_structured_budget_error() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        ctx.cancellation.cancel();
        let error = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(1),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect_err("cancelled begin");
        assert_eq!(error.code, "operationCancelled");
        assert_eq!(error.phase.as_deref(), Some("reservingBlob"));
        assert!(error.usage.is_some());
    }

    #[test]
    fn caller_blob_budget_can_only_tighten_hard_caps_and_zero_is_not_unlimited() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");

        let oversized = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(MAX_BLOB_BYTES.saturating_add(1)),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: BlobToolBudget {
                        timeout_ms: u64::MAX,
                        max_bytes_read: u64::MAX,
                        max_bytes_written: u64::MAX,
                        max_open_files: u32::MAX,
                    },
                },
            )
            .expect_err("caller budget must not raise the hard blob size cap");
        assert_eq!(oversized.code, "blobQuotaExceeded");

        let bytes = b"abcd";
        let begin = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(bytes.len() as u64),
                    content_type: None,
                    expected_sha256: Some(sha256_hex(bytes)),
                    chunk_size_bytes: Some(MIN_CHUNK_BYTES),
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin");
        let limited = store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 0,
                    data_base64: STANDARD.encode(bytes),
                    chunk_sha256: Some(sha256_hex(bytes)),
                    budget: BlobToolBudget {
                        max_bytes_written: 3,
                        ..BlobToolBudget::default()
                    },
                },
            )
            .expect_err("caller write cap must tighten the hard cap");
        assert_eq!(limited.code, "byteBudgetExceeded");
        assert_eq!(
            store
                .status(&ctx, &begin.upload_id)
                .expect("status")
                .next_offset,
            0
        );

        let zero = store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id,
                    offset: 0,
                    data_base64: STANDARD.encode(bytes),
                    chunk_sha256: Some(sha256_hex(bytes)),
                    budget: BlobToolBudget {
                        max_bytes_written: 0,
                        ..BlobToolBudget::default()
                    },
                },
            )
            .expect_err("zero caller cap must not mean unlimited");
        assert_eq!(zero.code, "byteBudgetExceeded");

        let zero_timeout = blob_tracker(
            &ctx,
            &BlobToolBudget {
                timeout_ms: 0,
                ..BlobToolBudget::default()
            },
        );
        assert_eq!(
            zero_timeout
                .checkpoint_at(std::time::Instant::now() + Duration::from_secs(1))
                .expect_err("zero timeout must clamp to a bounded deadline")
                .code,
            "timeBudgetExceeded"
        );
    }

    #[test]
    fn sequential_upload_resumes_and_duplicate_is_idempotent() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        let bytes = b"large content";
        let begin = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(bytes.len() as u64),
                    content_type: None,
                    expected_sha256: Some(sha256_hex(bytes)),
                    chunk_size_bytes: Some(MIN_CHUNK_BYTES),
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin");
        let request = BlobChunkRequest {
            upload_id: begin.upload_id.clone(),
            offset: 0,
            data_base64: STANDARD.encode(bytes),
            chunk_sha256: Some(sha256_hex(bytes)),
            budget: Default::default(),
        };
        let first = store.write_chunk(&ctx, request.clone()).expect("chunk");
        let duplicate = store.write_chunk(&ctx, request).expect("duplicate");
        assert_eq!(first.next_offset, bytes.len() as u64);
        assert_eq!(duplicate.next_offset, first.next_offset);
        assert_eq!(
            store
                .status(&ctx, &begin.upload_id)
                .expect("status")
                .next_offset,
            bytes.len() as u64
        );
        assert_eq!(
            store
                .seal(
                    &ctx,
                    BlobSealRequest {
                        upload_id: begin.upload_id,
                        final_size_bytes: bytes.len() as u64,
                        sha256: sha256_hex(bytes),
                        budget: Default::default(),
                    },
                )
                .expect("seal")
                .state,
            "sealed"
        );
    }

    #[test]
    fn conflicting_duplicate_and_cross_owner_access_are_rejected() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        let begin = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteText".into(),
                    expected_size_bytes: Some(3),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin");
        store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 0,
                    data_base64: STANDARD.encode(b"abc"),
                    chunk_sha256: None,
                    budget: Default::default(),
                },
            )
            .expect("chunk");
        let conflict = store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 0,
                    data_base64: STANDARD.encode(b"xyz"),
                    chunk_sha256: None,
                    budget: Default::default(),
                },
            )
            .expect_err("conflict");
        assert_eq!(conflict.code, "blobChunkConflict");
        let denied = store
            .status(&context("other"), &begin.upload_id)
            .expect_err("denied");
        assert_eq!(denied.code, "blobAccessDenied");
    }

    #[test]
    fn integrity_mismatch_does_not_seal() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        let begin = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(3),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin");
        store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 0,
                    data_base64: STANDARD.encode(b"abc"),
                    chunk_sha256: None,
                    budget: Default::default(),
                },
            )
            .expect("chunk");
        let error = store
            .seal(
                &ctx,
                BlobSealRequest {
                    upload_id: begin.upload_id.clone(),
                    final_size_bytes: 3,
                    sha256: "0".repeat(64),
                    budget: Default::default(),
                },
            )
            .expect_err("integrity mismatch");
        assert_eq!(error.code, "blobIntegrityMismatch");
        assert_eq!(
            store.status(&ctx, &begin.upload_id).expect("status").state,
            "uploading"
        );
    }

    #[test]
    fn task_cleanup_removes_only_matching_owner_bytes() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let first = context("agent");
        let mut second = context("agent");
        second.task_id = Some("other-task".into());
        let a = store
            .begin(
                &first,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(1),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin first");
        let b = store
            .begin(
                &second,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(1),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin second");
        assert_eq!(store.cleanup_task("task").expect("cleanup"), 1);
        assert_eq!(
            store
                .status(&first, &a.upload_id)
                .expect_err("removed")
                .code,
            "blobNotFound"
        );
        assert_eq!(
            store.status(&second, &b.upload_id).expect("kept").state,
            "uploading"
        );
    }

    #[test]
    fn only_one_consumer_can_lease_a_sealed_blob() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        let bytes = b"abc";
        let begin = store
            .begin(
                &ctx,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(bytes.len() as u64),
                    content_type: None,
                    expected_sha256: None,
                    chunk_size_bytes: None,
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin");
        store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 0,
                    data_base64: STANDARD.encode(bytes),
                    chunk_sha256: None,
                    budget: Default::default(),
                },
            )
            .expect("chunk");
        store
            .seal(
                &ctx,
                BlobSealRequest {
                    upload_id: begin.upload_id,
                    final_size_bytes: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                    budget: Default::default(),
                },
            )
            .expect("seal");
        let first = store
            .lease(&ctx, &begin.content_ref, "fsWriteRaw")
            .expect("first lease");
        let error = match store.lease(&ctx, &begin.content_ref, "fsWriteRaw") {
            Ok(_) => panic!("second lease must conflict"),
            Err(error) => error,
        };
        assert_eq!(error.code, "blobStateConflict");
        first.finish(false).expect("release");
        store
            .lease(&ctx, &begin.content_ref, "fsWriteRaw")
            .expect("lease after rollback");
    }

    #[test]
    fn upload_metadata_recovers_after_restart_and_resumes() {
        let directory = tempdir().expect("tempdir");
        let root = directory.path().to_path_buf();
        let ctx = context("agent");
        let begin = {
            let store = BlobStore::new(root.clone()).expect("store");
            let begin = store
                .begin(
                    &ctx,
                    BlobBeginRequest {
                        purpose: "fsWriteRaw".into(),
                        expected_size_bytes: Some(6),
                        content_type: None,
                        expected_sha256: None,
                        chunk_size_bytes: None,
                        ttl_seconds: None,
                        budget: Default::default(),
                    },
                )
                .expect("begin");
            store
                .write_chunk(
                    &ctx,
                    BlobChunkRequest {
                        upload_id: begin.upload_id.clone(),
                        offset: 0,
                        data_base64: STANDARD.encode(b"abc"),
                        chunk_sha256: None,
                        budget: Default::default(),
                    },
                )
                .expect("first chunk");
            begin
        };

        let store = BlobStore::new(root).expect("recovered store");
        assert_eq!(
            store
                .status(&ctx, &begin.upload_id)
                .expect("recovered status")
                .next_offset,
            3
        );
        store
            .write_chunk(
                &ctx,
                BlobChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    offset: 3,
                    data_base64: STANDARD.encode(b"def"),
                    chunk_sha256: None,
                    budget: Default::default(),
                },
            )
            .expect("resumed chunk");
        store
            .seal(
                &ctx,
                BlobSealRequest {
                    upload_id: begin.upload_id,
                    final_size_bytes: 6,
                    sha256: sha256_hex(b"abcdef"),
                    budget: Default::default(),
                },
            )
            .expect("seal after resume");
    }

    #[test]
    fn crash_during_consume_recovers_blob_to_sealed() {
        let directory = tempdir().expect("tempdir");
        let root = directory.path().to_path_buf();
        let ctx = context("agent");
        let begin = {
            let store = BlobStore::new(root.clone()).expect("store");
            let begin = store
                .begin(
                    &ctx,
                    BlobBeginRequest {
                        purpose: "fsWriteRaw".into(),
                        expected_size_bytes: Some(3),
                        content_type: None,
                        expected_sha256: None,
                        chunk_size_bytes: None,
                        ttl_seconds: None,
                        budget: Default::default(),
                    },
                )
                .expect("begin");
            store
                .write_chunk(
                    &ctx,
                    BlobChunkRequest {
                        upload_id: begin.upload_id.clone(),
                        offset: 0,
                        data_base64: STANDARD.encode(b"abc"),
                        chunk_sha256: None,
                        budget: Default::default(),
                    },
                )
                .expect("chunk");
            store
                .seal(
                    &ctx,
                    BlobSealRequest {
                        upload_id: begin.upload_id.clone(),
                        final_size_bytes: 3,
                        sha256: sha256_hex(b"abc"),
                        budget: Default::default(),
                    },
                )
                .expect("seal");
            let lease = store
                .lease(&ctx, &begin.content_ref, "fsWriteRaw")
                .expect("lease");
            std::mem::forget(lease);
            begin
        };

        let store = BlobStore::new(root).expect("recovered store");
        assert_eq!(
            store.status(&ctx, &begin.upload_id).expect("status").state,
            "sealed"
        );
        store
            .lease(&ctx, &begin.content_ref, "fsWriteRaw")
            .expect("re-lease after crash");
    }

    #[test]
    fn managed_artifact_is_task_scoped_lazy_readable_and_restart_safe() {
        let directory = tempdir().expect("tempdir");
        let root = directory.path().to_path_buf();
        let mut writer = context("agent");
        writer.turn_id = Some("turn-one".into());
        let marker = "MANAGED-ARTIFACT-MARKER";
        let artifact = {
            let store = BlobStore::new(root.clone()).expect("store");
            store
                .store_artifact_json(&writer, &serde_json::json!({"content": marker}), 60)
                .expect("store artifact")
        };

        let store = BlobStore::new(root.clone()).expect("recovered store");
        let mut later_turn = context("agent");
        later_turn.turn_id = Some("turn-two".into());
        let read = store
            .read_artifact_text(&later_turn, &artifact.content_ref, 64 * 1024)
            .expect("same task later turn can read");
        assert!(read.content.contains(marker));
        assert!(!read.truncated);
        assert_eq!(read.sha256, artifact.sha256);

        let mut other_task = later_turn.clone();
        other_task.task_id = Some("other-task".into());
        let denied = store
            .read_artifact_text(&other_task, &artifact.content_ref, 64 * 1024)
            .expect_err("different task denied");
        assert_eq!(denied.code, "artifactAccessDenied");

        std::fs::write(root.join("orphan.blob.tmp-test"), b"partial").expect("orphan temp");
        drop(store);
        BlobStore::new(root.clone()).expect("restart cleans orphan");
        assert!(!root.join("orphan.blob.tmp-test").exists());
    }

    #[test]
    fn managed_artifact_file_import_is_streamed_registered_and_source_preserved() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().join("blobs")).expect("store");
        let ctx = context("agent");
        let source = directory.path().join("git-output.txt");
        let payload = "git-output-line\n".repeat(32 * 1024);
        std::fs::write(&source, payload.as_bytes()).expect("write source");

        let artifact = store
            .store_artifact_file(
                &ctx,
                &source,
                Some("text/plain; charset=utf-8".to_owned()),
                60,
            )
            .expect("import artifact file");
        assert!(source.exists(), "producer owns source cleanup");
        assert_eq!(artifact.size_bytes, payload.len() as u64);
        let first = store
            .read_artifact_text_range(&ctx, &artifact.content_ref, 0, 128 * 1024)
            .expect("read first imported artifact page");
        assert!(first.truncated);
        assert_eq!(first.offset, 0);
        let second_offset = first.next_offset.expect("continuation offset");
        let second = store
            .read_artifact_text_range(&ctx, &artifact.content_ref, second_offset, 128 * 1024)
            .expect("read second imported artifact page");
        assert_eq!(second.offset, second_offset);
        assert_eq!(first.sha256, artifact.sha256);
        assert_eq!(second.sha256, artifact.sha256);
        assert_eq!(
            format!("{}{}", first.content, second.content),
            payload[..256 * 1024]
        );
    }

    #[test]
    fn managed_artifact_cleanup_and_gc_are_safe_around_reads() {
        let directory = tempdir().expect("tempdir");
        let store = BlobStore::new(directory.path().to_path_buf()).expect("store");
        let ctx = context("agent");
        let artifact = store
            .store_artifact_json(&ctx, &serde_json::json!({"value":"x".repeat(4096)}), 60)
            .expect("store artifact");
        let reader_store = store.clone();
        let reader_ctx = ctx.clone();
        let content_ref = artifact.content_ref.clone();
        let reader = std::thread::spawn(move || {
            reader_store.read_artifact_text(&reader_ctx, &content_ref, 8192)
        });
        store.gc().expect("concurrent gc");
        let read = reader
            .join()
            .expect("reader thread")
            .expect("artifact read");
        assert!(!read.truncated);
        assert_eq!(store.cleanup_task("task").expect("task cleanup"), 1);
        let missing = store
            .read_artifact_text(&ctx, &artifact.content_ref, 8192)
            .expect_err("deleted task artifact unavailable");
        assert_eq!(missing.code, "artifact_not_found");
    }
}
