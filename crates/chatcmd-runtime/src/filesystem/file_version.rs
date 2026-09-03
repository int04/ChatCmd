use super::*;
use crate::{FsPermissions, FsStatBudget, FsStatRequest, FsStatResult, VersionStrength};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

type HmacSha256 = Hmac<Sha256>;
const TOKEN_VERSION: u8 = 1;
const HASH_CHUNK_BYTES: usize = 64 * 1024;
const SAMPLE_BYTES: usize = 64 * 1024;

/// Decoded, authenticated file version. Identity and path values are one-way fingerprints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    schema: u8,
    path_fingerprint: String,
    identity_fingerprint: String,
    entry_type: String,
    size_bytes: u64,
    modified_at_ns: Option<u64>,
    changed_at_ns: Option<u64>,
    strength: VersionStrength,
    content_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    identity_fingerprint: String,
    entry_type: String,
    size_bytes: u64,
    modified_at_ns: Option<u64>,
    changed_at_ns: Option<u64>,
}

impl WorkspaceService {
    /// Captures metadata and, when requested, a bounded streaming hash.
    pub async fn stat_v2(
        &self,
        context: Option<&OperationContext>,
        request: &FsStatRequest,
    ) -> RuntimeResult<FsStatResult> {
        validate_hash_algorithm(request)?;
        let resolved = self.existing(&request.path)?;
        resolved.revalidate()?;
        let path = resolved.canonical_path.clone();
        let root = resolved.root.clone();
        let key = self.version_key.clone();
        let request = request.clone();
        let cancellation =
            context.map_or_else(CancellationToken::new, |value| value.cancellation.clone());
        tokio::task::spawn_blocking(move || capture(&path, &root, &key, &request, &cancellation))
            .await
            .map_err(join_error)?
    }

    /// Authenticates and re-captures a token for mutation precondition checks.
    pub async fn verify_expected_version(
        &self,
        path: &Path,
        token: &str,
        context: Option<&OperationContext>,
    ) -> RuntimeResult<FileVersion> {
        let expected = decode_token(&self.version_key, token)?;
        let resolved = self.existing(path).map_err(|error| {
            if error.code == "not_found" {
                RuntimeError::new("targetMissing", "versioned target no longer exists")
            } else {
                error
            }
        })?;
        let request = FsStatRequest {
            path: resolved.canonical_path.clone(),
            version_strength: expected.strength,
            hash_algorithm: expected.content_hash.as_ref().map(|_| "sha256".to_owned()),
            budget: FsStatBudget::default(),
        };
        let current_result = self.stat_v2(context, &request).await?;
        let current = decode_token(&self.version_key, &current_result.version_token)?;
        if current.path_fingerprint != expected.path_fingerprint {
            return Err(RuntimeError::new(
                "versionMismatch",
                "version token belongs to a different path or scope",
            ));
        }
        if current.identity_fingerprint != expected.identity_fingerprint {
            return Err(RuntimeError::new(
                "targetReplaced",
                "path now refers to a different filesystem entry",
            ));
        }
        if current != expected {
            return Err(RuntimeError::new(
                "versionMismatch",
                "filesystem entry changed since the version token was captured",
            ));
        }
        Ok(current)
    }
}

fn capture(
    path: &Path,
    root: &Path,
    key: &[u8; 32],
    request: &FsStatRequest,
    cancellation: &CancellationToken,
) -> RuntimeResult<FsStatResult> {
    let started = Instant::now();
    let before_metadata = fs::symlink_metadata(path).map_err(io_error)?;
    reject_reparse_metadata(&before_metadata)?;
    let before = snapshot(path, &before_metadata)?;
    #[cfg(test)]
    wait_on_hash_test_hook(path);
    let content_hash = match request.version_strength {
        VersionStrength::Metadata => None,
        VersionStrength::Sampled => {
            let bytes = before
                .size_bytes
                .min(u64::try_from(SAMPLE_BYTES).unwrap_or(u64::MAX))
                .saturating_mul(3);
            ensure_hash_size_within_budget(bytes, &request.budget)?;
            Some(hash_sampled(
                path,
                before.size_bytes,
                &request.budget,
                cancellation,
                started,
            )?)
        }
        VersionStrength::Content => {
            ensure_hash_size_within_budget(before.size_bytes, &request.budget)?;
            Some(hash_content(path, &request.budget, cancellation, started)?)
        }
    };
    let after_metadata = fs::symlink_metadata(path).map_err(io_error)?;
    let after = snapshot(path, &after_metadata)?;
    ensure_unchanged(&before, &after)?;
    let path_fingerprint = fingerprint_path(path, root);
    let version = FileVersion {
        schema: TOKEN_VERSION,
        path_fingerprint,
        identity_fingerprint: before.identity_fingerprint,
        entry_type: before.entry_type.clone(),
        size_bytes: before.size_bytes,
        modified_at_ns: before.modified_at_ns,
        changed_at_ns: before.changed_at_ns,
        strength: request.version_strength,
        content_hash: content_hash.clone(),
    };
    let token = encode_token(key, &version)?;
    Ok(FsStatResult {
        path: path.to_path_buf(),
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        entry_type: before.entry_type,
        size: before.size_bytes,
        size_bytes: before.size_bytes,
        readonly: after_metadata.permissions().readonly(),
        modified_at_ns: before.modified_at_ns,
        created_at_ns: system_time_ns(after_metadata.created().ok()),
        permissions: permissions(&after_metadata),
        version_token: token,
        version_strength: request.version_strength,
        content_hash,
        hash_algorithm: match request.version_strength {
            VersionStrength::Metadata => None,
            VersionStrength::Sampled | VersionStrength::Content => Some("sha256".to_owned()),
        },
        symlink: false,
    })
}

fn validate_hash_algorithm(request: &FsStatRequest) -> RuntimeResult<()> {
    if let Some(algorithm) = request.hash_algorithm.as_deref()
        && !algorithm.eq_ignore_ascii_case("sha256")
    {
        return Err(RuntimeError::new(
            "unsupportedHashAlgorithm",
            "fs_stat supports only sha256",
        ));
    }
    Ok(())
}

fn snapshot(path: &Path, metadata: &fs::Metadata) -> RuntimeResult<Snapshot> {
    let entry_type = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "other"
    }
    .to_owned();
    Ok(Snapshot {
        identity_fingerprint: fingerprint_identity(path, metadata)?,
        entry_type,
        size_bytes: metadata.len(),
        modified_at_ns: system_time_ns(metadata.modified().ok()),
        changed_at_ns: changed_time_ns(metadata),
    })
}

fn fingerprint_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(relative.to_string_lossy().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn fingerprint_identity(_path: &Path, _metadata: &fs::Metadata) -> RuntimeResult<String> {
    let mut hasher = Sha256::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        hasher.update(_metadata.dev().to_le_bytes());
        hasher.update(_metadata.ino().to_le_bytes());
    }
    #[cfg(windows)]
    {
        let (volume, index) = windows_file_identity(_path)?;
        hasher.update(volume.to_le_bytes());
        hasher.update(index.to_le_bytes());
    }
    #[cfg(not(any(unix, windows)))]
    {
        hasher.update(_metadata.len().to_le_bytes());
        if let Some(ns) = system_time_ns(_metadata.created().ok()) {
            hasher.update(ns.to_le_bytes());
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn changed_time_ns(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;
    let seconds = i128::from(metadata.ctime());
    let nanos = i128::from(metadata.ctime_nsec());
    u64::try_from(seconds.checked_mul(1_000_000_000)?.checked_add(nanos)?).ok()
}

#[cfg(not(unix))]
fn changed_time_ns(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

#[cfg(windows)]
fn windows_file_identity(path: &Path) -> RuntimeResult<(u32, u64)> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle as _};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(path).map_err(io_error)?;
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `information` points to writable storage for the duration of the call,
    // and `file` keeps the borrowed OS handle valid.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    // SAFETY: a successful Win32 call initialized the complete output structure.
    let information = unsafe { information.assume_init() };
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((information.dwVolumeSerialNumber, index))
}

fn permissions(_metadata: &fs::Metadata) -> FsPermissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        FsPermissions {
            mode: Some(format!("{:04o}", _metadata.permissions().mode() & 0o7777)),
        }
    }
    #[cfg(not(unix))]
    FsPermissions { mode: None }
}

fn system_time_ns(value: Option<SystemTime>) -> Option<u64> {
    value?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
}

fn hash_content(
    path: &Path,
    budget: &FsStatBudget,
    cancellation: &CancellationToken,
    started: Instant,
) -> RuntimeResult<String> {
    let file = File::open(path).map_err(io_error)?;
    let mut reader = BufReader::with_capacity(HASH_CHUNK_BYTES, file);
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; HASH_CHUNK_BYTES];
    let mut bytes_read = 0_u64;
    loop {
        check_budget(budget, cancellation, started, bytes_read)?;
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        check_budget(budget, cancellation, started, bytes_read)?;
        hash.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn hash_sampled(
    path: &Path,
    size: u64,
    budget: &FsStatBudget,
    cancellation: &CancellationToken,
    started: Instant,
) -> RuntimeResult<String> {
    let mut file = File::open(path).map_err(io_error)?;
    let sample = u64::try_from(SAMPLE_BYTES).unwrap_or(u64::MAX).min(size);
    let positions = [
        0,
        size.saturating_sub(sample) / 2,
        size.saturating_sub(sample),
    ];
    let mut hash = Sha256::new();
    hash.update(size.to_le_bytes());
    let mut bytes_read = 0_u64;
    let mut buffer = vec![0_u8; usize::try_from(sample).unwrap_or(SAMPLE_BYTES)];
    for position in positions {
        check_budget(budget, cancellation, started, bytes_read)?;
        file.seek(SeekFrom::Start(position)).map_err(io_error)?;
        let read = file.read(&mut buffer).map_err(io_error)?;
        bytes_read = bytes_read.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        check_budget(budget, cancellation, started, bytes_read)?;
        hash.update(position.to_le_bytes());
        hash.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn check_budget(
    budget: &FsStatBudget,
    cancellation: &CancellationToken,
    started: Instant,
    bytes_read: u64,
) -> RuntimeResult<()> {
    if cancellation.is_cancelled() {
        return Err(RuntimeError::new(
            "operationCancelled",
            "fs_stat hashing was cancelled",
        ));
    }
    if started.elapsed() > Duration::from_millis(budget.timeout_ms) {
        return Err(RuntimeError::new(
            "hashBudgetExceeded",
            "fs_stat hashing timed out",
        ));
    }
    if bytes_read > budget.max_bytes_read {
        return Err(RuntimeError::new(
            "hashBudgetExceeded",
            "fs_stat hashing exceeded maxBytesRead",
        ));
    }
    Ok(())
}

fn ensure_unchanged(before: &Snapshot, after: &Snapshot) -> RuntimeResult<()> {
    if before == after {
        Ok(())
    } else {
        Err(RuntimeError::new(
            "fileChangedDuringHash",
            "filesystem entry changed while its version was being captured",
        ))
    }
}

fn ensure_hash_size_within_budget(bytes: u64, budget: &FsStatBudget) -> RuntimeResult<()> {
    if bytes > budget.max_bytes_read {
        Err(RuntimeError::new(
            "hashBudgetExceeded",
            "fs_stat hashing would exceed maxBytesRead",
        ))
    } else {
        Ok(())
    }
}

fn encode_token(key: &[u8; 32], version: &FileVersion) -> RuntimeResult<String> {
    let payload = serde_json::to_vec(version)
        .map_err(|error| RuntimeError::new("versionUnsupported", error.to_string()))?;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| RuntimeError::new("versionUnsupported", "invalid version signing key"))?;
    mac.update(encoded.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("v{TOKEN_VERSION}:{encoded}:{signature}"))
}

fn decode_token(key: &[u8; 32], token: &str) -> RuntimeResult<FileVersion> {
    let mut parts = token.split(':');
    let version = parts.next();
    let payload = parts.next();
    let signature = parts.next();
    if version != Some("v1") || parts.next().is_some() {
        return Err(RuntimeError::new(
            "versionUnsupported",
            "unsupported version token",
        ));
    }
    let (Some(payload), Some(signature)) = (payload, signature) else {
        return Err(RuntimeError::new(
            "versionUnsupported",
            "malformed version token",
        ));
    };
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| RuntimeError::new("versionUnsupported", "malformed version signature"))?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| RuntimeError::new("versionUnsupported", "invalid version signing key"))?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).map_err(|_| {
        RuntimeError::new("versionUnsupported", "version token authentication failed")
    })?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| RuntimeError::new("versionUnsupported", "malformed version payload"))?;
    let decoded: FileVersion = serde_json::from_slice(&bytes)
        .map_err(|_| RuntimeError::new("versionUnsupported", "invalid version payload"))?;
    if decoded.schema != TOKEN_VERSION {
        return Err(RuntimeError::new(
            "versionUnsupported",
            "unsupported version schema",
        ));
    }
    Ok(decoded)
}

#[cfg(test)]
fn digest_hex(value: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovalDecision, BoxFuture, PolicyEngine};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Barrier},
    };
    use tempfile::TempDir;

    struct Approve;

    impl ApprovalDecision for Approve {
        fn request<'a>(
            &'a self,
            _context: &'a PolicyContext,
        ) -> BoxFuture<'a, RuntimeResult<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    fn service(root: &Path) -> WorkspaceService {
        WorkspaceService::new(
            &[root.to_path_buf()],
            PolicyEngine::new(
                Some(crate::ExecutionPolicy {
                    default: crate::PolicyDecision::Allow,
                    per_agent_tool: BTreeMap::new(),
                    per_root: BTreeMap::new(),
                }),
                Arc::new(Approve),
            ),
        )
        .expect("workspace service")
    }

    fn request(path: &Path, strength: VersionStrength) -> FsStatRequest {
        FsStatRequest {
            path: path.to_path_buf(),
            version_strength: strength,
            hash_algorithm: None,
            budget: FsStatBudget::default(),
        }
    }

    #[tokio::test]
    async fn metadata_token_is_stable_and_legacy_fields_remain() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("stable.txt");
        fs::write(&path, "stable").expect("write file");
        let workspace = service(directory.path());

        let first = workspace
            .stat_v2(None, &request(&path, VersionStrength::Metadata))
            .await
            .expect("first stat");
        let second = workspace
            .stat_v2(None, &request(&path, VersionStrength::Metadata))
            .await
            .expect("second stat");

        assert_eq!(first.version_token, second.version_token);
        assert_eq!(first.size, first.size_bytes);
        assert_eq!(first.content_hash, None);
        assert_eq!(first.hash_algorithm, None);
    }

    #[tokio::test]
    async fn metadata_detects_same_size_change_and_atomic_replacement() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("changed.txt");
        fs::write(&path, "aaaa").expect("write file");
        let workspace = service(directory.path());
        let initial = workspace
            .stat_v2(None, &request(&path, VersionStrength::Metadata))
            .await
            .expect("initial stat");

        std::thread::sleep(Duration::from_millis(20));
        fs::write(&path, "bbbb").expect("same-size update");
        let changed = workspace
            .stat_v2(None, &request(&path, VersionStrength::Metadata))
            .await
            .expect("changed stat");
        assert_ne!(initial.version_token, changed.version_token);
        assert_eq!(
            workspace
                .verify_expected_version(&path, &initial.version_token, None)
                .await
                .expect_err("old metadata token must fail")
                .code,
            "versionMismatch"
        );

        let replacement = directory.path().join("replacement.tmp");
        fs::write(&replacement, "bbbb").expect("replacement");
        fs::remove_file(&path).expect("remove old file");
        fs::rename(&replacement, &path).expect("atomic rename");
        assert_eq!(
            workspace
                .verify_expected_version(&path, &changed.version_token, None)
                .await
                .expect_err("replacement identity must fail")
                .code,
            "targetReplaced"
        );
    }

    #[tokio::test]
    async fn content_hash_matches_sha256_vector_and_streams_large_file() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("vector.bin");
        fs::write(&path, b"abc").expect("write vector");
        let workspace = service(directory.path());
        let vector = workspace
            .stat_v2(None, &request(&path, VersionStrength::Content))
            .await
            .expect("content stat");
        assert_eq!(
            vector.content_hash.as_deref(),
            Some("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );

        let large = vec![0x5a_u8; HASH_CHUNK_BYTES * 5 + 17];
        fs::write(&path, &large).expect("write large file");
        let result = workspace
            .stat_v2(None, &request(&path, VersionStrength::Content))
            .await
            .expect("stream large file");
        assert_eq!(result.content_hash, Some(digest_hex(&large)));
    }

    #[tokio::test]
    async fn tokens_are_tamper_evident_and_path_bound() {
        let directory = TempDir::new().expect("temp directory");
        let first_path = directory.path().join("a.txt");
        let second_path = directory.path().join("b.txt");
        fs::write(&first_path, "same").expect("first");
        fs::write(&second_path, "same").expect("second");
        let workspace = service(directory.path());
        let result = workspace
            .stat_v2(None, &request(&first_path, VersionStrength::Content))
            .await
            .expect("stat");

        let mut tampered = result.version_token.clone().into_bytes();
        let last = tampered.last_mut().expect("nonempty token");
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).expect("ASCII token");
        assert_eq!(
            workspace
                .verify_expected_version(&first_path, &tampered, None)
                .await
                .expect_err("tampered token")
                .code,
            "versionUnsupported"
        );
        assert_eq!(
            workspace
                .verify_expected_version(&second_path, &result.version_token, None)
                .await
                .expect_err("cross-path token")
                .code,
            "versionMismatch"
        );
        assert_eq!(
            workspace
                .verify_expected_version(&first_path, "v0:obsolete:token", None)
                .await
                .expect_err("old schema token")
                .code,
            "versionUnsupported"
        );
        fs::remove_file(&first_path).expect("remove versioned target");
        assert_eq!(
            workspace
                .verify_expected_version(&first_path, &result.version_token, None)
                .await
                .expect_err("missing target")
                .code,
            "targetMissing"
        );
    }

    #[tokio::test]
    async fn hashing_honors_cancellation_and_byte_budget() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("bounded.bin");
        fs::write(&path, vec![1_u8; HASH_CHUNK_BYTES + 1]).expect("write file");
        let workspace = service(directory.path());
        let context = OperationContext::new("stat", "agent", "fs_stat");
        context.cancellation.cancel();
        assert_eq!(
            workspace
                .stat_v2(Some(&context), &request(&path, VersionStrength::Content))
                .await
                .expect_err("cancelled hash")
                .code,
            "operationCancelled"
        );

        let mut bounded = request(&path, VersionStrength::Content);
        bounded.budget.max_bytes_read = 1;
        assert_eq!(
            workspace
                .stat_v2(None, &bounded)
                .await
                .expect_err("byte budget")
                .code,
            "hashBudgetExceeded"
        );

        let mut timed_out = request(&path, VersionStrength::Content);
        timed_out.budget.timeout_ms = 0;
        assert_eq!(
            workspace
                .stat_v2(None, &timed_out)
                .await
                .expect_err("time budget")
                .code,
            "hashBudgetExceeded"
        );
    }

    #[tokio::test]
    async fn file_changed_during_hash_returns_conflict() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("racing.bin");
        fs::write(&path, vec![1_u8; HASH_CHUNK_BYTES]).expect("write file");
        let workspace = service(directory.path());
        let gate = Arc::new((Barrier::new(2), Barrier::new(2)));
        *hash_test_hook().lock().expect("hook lock") =
            Some((path.canonicalize().expect("canonical path"), gate.clone()));
        let mutation_path = path.clone();
        let replacement_path = directory.path().join("racing-replacement.bin");
        fs::write(&replacement_path, vec![2_u8; HASH_CHUNK_BYTES + 1]).expect("write replacement");
        let mutation_gate = gate.clone();
        let mutation = std::thread::spawn(move || {
            mutation_gate.0.wait();
            fs::remove_file(&mutation_path).expect("remove during capture");
            fs::rename(&replacement_path, &mutation_path).expect("replace during capture");
            mutation_gate.1.wait();
        });

        let error = workspace
            .stat_v2(None, &request(&path, VersionStrength::Content))
            .await
            .expect_err("concurrent mutation");
        mutation.join().expect("mutation thread");
        assert_eq!(error.code, "fileChangedDuringHash");
    }

    #[test]
    fn changed_snapshot_maps_to_hash_conflict() {
        let before = Snapshot {
            identity_fingerprint: "identity".into(),
            entry_type: "file".into(),
            size_bytes: 1,
            modified_at_ns: Some(1),
            changed_at_ns: Some(1),
        };
        let mut after = before.clone();
        after.size_bytes = 2;
        assert_eq!(
            ensure_unchanged(&before, &after)
                .expect_err("changed during hash")
                .code,
            "fileChangedDuringHash"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_identity_uses_device_and_inode() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("identity");
        fs::write(&path, "one").expect("write");
        let first =
            fingerprint_identity(&path, &fs::metadata(&path).expect("metadata")).expect("identity");
        fs::remove_file(&path).expect("remove");
        fs::write(&path, "two").expect("replace");
        let second =
            fingerprint_identity(&path, &fs::metadata(&path).expect("metadata")).expect("identity");
        assert_ne!(first, second);
    }

    #[cfg(windows)]
    #[test]
    fn windows_identity_uses_volume_and_file_index() {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("identity");
        fs::write(&path, "one").expect("write");
        let first = windows_file_identity(&path).expect("identity");
        fs::remove_file(&path).expect("remove");
        fs::write(&path, "two").expect("replace");
        let second = windows_file_identity(&path).expect("identity");
        assert_ne!(first, second);
    }
}

#[cfg(test)]
type HashTestHook = (PathBuf, Arc<(std::sync::Barrier, std::sync::Barrier)>);

#[cfg(test)]
fn hash_test_hook() -> &'static std::sync::Mutex<Option<HashTestHook>> {
    static HOOK: std::sync::OnceLock<std::sync::Mutex<Option<HashTestHook>>> =
        std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn wait_on_hash_test_hook(path: &Path) {
    let gate = hash_test_hook()
        .lock()
        .expect("hash hook lock")
        .as_ref()
        .filter(|(hook_path, _)| hook_path == path)
        .map(|(_, gate)| gate.clone());
    let Some(gate) = gate else {
        return;
    };
    gate.0.wait();
    gate.1.wait();
    *hash_test_hook().lock().expect("hash hook lock") = None;
}
