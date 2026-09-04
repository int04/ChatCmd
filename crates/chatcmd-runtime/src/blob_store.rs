//! Bounded, owner-scoped storage for large tool payloads.

use crate::{OperationContext, RuntimeError, RuntimeResult};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobBeginResult {
    pub upload_id: String,
    pub content_ref: String,
    pub chunk_size_bytes: usize,
    pub expires_at_ms: u64,
    pub max_size_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobChunkRequest {
    pub upload_id: String,
    pub offset: u64,
    pub data_base64: String,
    #[serde(default)]
    pub chunk_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobSealRequest {
    pub upload_id: String,
    pub final_size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobIdRequest {
    pub upload_id: String,
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
}

#[derive(Clone)]
pub struct BlobStore {
    root: Arc<PathBuf>,
    entries: Arc<Mutex<HashMap<String, BlobEntry>>>,
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

    pub fn new(root: PathBuf) -> RuntimeResult<Self> {
        fs::create_dir_all(&root).map_err(io_error)?;
        let entries = recover_entries(&root)?;
        Ok(Self {
            root: Arc::new(root),
            entries: Arc::new(Mutex::new(entries)),
        })
    }

    pub fn temporary() -> RuntimeResult<Self> {
        Self::new(std::env::temp_dir().join("chatcmd-blobs-v1"))
    }

    pub fn begin(
        &self,
        context: &OperationContext,
        request: BlobBeginRequest,
    ) -> RuntimeResult<BlobBeginResult> {
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
        Ok(BlobBeginResult {
            upload_id,
            content_ref,
            chunk_size_bytes: chunk_size,
            expires_at_ms,
            max_size_bytes: MAX_BLOB_BYTES,
        })
    }

    pub fn write_chunk(
        &self,
        context: &OperationContext,
        request: BlobChunkRequest,
    ) -> RuntimeResult<BlobStatus> {
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
                    Ok(status(entry))
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
        Ok(status(entry))
    }

    pub fn status(&self, context: &OperationContext, upload_id: &str) -> RuntimeResult<BlobStatus> {
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, upload_id)?;
        Ok(status(entry))
    }

    pub fn seal(
        &self,
        context: &OperationContext,
        request: BlobSealRequest,
    ) -> RuntimeResult<BlobStatus> {
        let expected_hash = normalize_hash(Some(&request.sha256))?.ok_or_else(|| {
            RuntimeError::new("blobIntegrityRequired", "final SHA-256 is required")
        })?;
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, &request.upload_id)?;
        if entry.state == BlobState::Sealed && entry.sha256.as_deref() == Some(&expected_hash) {
            return Ok(status(entry));
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
        let actual_hash = sha256_file(&entry.path)?;
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
        Ok(status(entry))
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
        let mut entries = lock(&self.entries)?;
        let entry = checked_entry_mut(&mut entries, context, upload_id)?;
        if entry.state != BlobState::Consumed {
            entry.state = BlobState::Aborted;
            let _ = fs::remove_file(&entry.path);
            let _ = fs::remove_file(metadata_path(&self.root, &entry.upload_id));
        }
        Ok(status(entry))
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
        if name.contains(".meta.tmp-") {
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

fn status(entry: &BlobEntry) -> BlobStatus {
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
                },
            )
            .expect("begin");
        let request = BlobChunkRequest {
            upload_id: begin.upload_id.clone(),
            offset: 0,
            data_base64: STANDARD.encode(bytes),
            chunk_sha256: Some(sha256_hex(bytes)),
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
}
