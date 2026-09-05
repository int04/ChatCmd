use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, FsConflictPolicy, FsDeleteMode, FsDeleteRequest,
    FsMutationBudget, FsQuarantineGcRequest, FsQuarantineRestoreRequest, FsTransferRequest,
    FsVerifyMode, MutationFaultInjector, MutationJournalSink, OperationContext, PolicyDecision,
    PolicyEngine, RuntimeError, RuntimeResult, WorkspaceService,
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Barrier, Mutex},
};

struct Approve;

#[derive(Debug)]
struct FailAfterBytes(u64);

#[derive(Debug)]
struct FailAtPhase(&'static str);

#[derive(Debug)]
struct GateAtPhase {
    point: &'static str,
    entered: Arc<Barrier>,
    resume: Arc<Barrier>,
}

impl MutationFaultInjector for GateAtPhase {
    fn checkpoint(&self, point: &str, _files: u64, _bytes: u64) -> RuntimeResult<()> {
        if point == self.point {
            self.entered.wait();
            self.resume.wait();
        }
        Ok(())
    }
}

impl MutationFaultInjector for FailAtPhase {
    fn checkpoint(&self, point: &str, _files: u64, _bytes: u64) -> RuntimeResult<()> {
        if point == self.0 {
            return Err(RuntimeError::new(
                "fault_injected",
                format!("fault at {point}"),
            ));
        }
        Ok(())
    }
}

impl MutationFaultInjector for FailAfterBytes {
    fn checkpoint(&self, point: &str, _files: u64, bytes: u64) -> RuntimeResult<()> {
        if point == "copyBytes" && bytes >= self.0 {
            return Err(RuntimeError::new(
                "fault_injected",
                "deterministic copy fault",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct BenchmarkState {
    journal_transitions: u64,
    max_journal_bytes: usize,
    max_open_files: usize,
}

#[derive(Debug, Default)]
struct BenchmarkProbe {
    state: Mutex<BenchmarkState>,
}

#[derive(Debug, Default)]
struct RecoveryJournalSink {
    rows: Mutex<Vec<String>>,
}

impl RecoveryJournalSink {
    fn with_row(row: String) -> Self {
        Self {
            rows: Mutex::new(vec![row]),
        }
    }
}

impl MutationJournalSink for RecoveryJournalSink {
    fn upsert_json(&self, journal_json: &str) -> RuntimeResult<()> {
        let operation_id = serde_json::from_str::<serde_json::Value>(journal_json)
            .ok()
            .and_then(|value| value["operationId"].as_str().map(str::to_owned))
            .ok_or_else(|| RuntimeError::new("journal_error", "missing operationId"))?;
        let mut rows = self.rows.lock().expect("recovery journal rows");
        rows.retain(|row| {
            serde_json::from_str::<serde_json::Value>(row)
                .ok()
                .and_then(|value| value["operationId"].as_str().map(str::to_owned))
                .as_deref()
                != Some(operation_id.as_str())
        });
        rows.push(journal_json.to_owned());
        Ok(())
    }

    fn remove(&self, operation_id: &str) -> RuntimeResult<()> {
        let mut rows = self.rows.lock().expect("recovery journal rows");
        rows.retain(|row| {
            serde_json::from_str::<serde_json::Value>(row)
                .ok()
                .and_then(|value| value["operationId"].as_str().map(str::to_owned))
                .as_deref()
                != Some(operation_id)
        });
        Ok(())
    }

    fn list_json(&self) -> RuntimeResult<Vec<String>> {
        Ok(self.rows.lock().expect("recovery journal rows").clone())
    }
}

impl MutationJournalSink for BenchmarkProbe {
    fn upsert_json(&self, journal_json: &str) -> RuntimeResult<()> {
        let mut state = self.state.lock().expect("benchmark state");
        state.journal_transitions = state.journal_transitions.saturating_add(1);
        state.max_journal_bytes = state.max_journal_bytes.max(journal_json.len());
        Ok(())
    }

    fn remove(&self, _operation_id: &str) -> RuntimeResult<()> {
        Ok(())
    }
}

impl MutationFaultInjector for BenchmarkProbe {
    fn checkpoint(&self, point: &str, files: u64, _bytes: u64) -> RuntimeResult<()> {
        if point == "copyFile" && (files == 0 || files % 1_000 != 0) {
            return Ok(());
        }
        #[cfg(unix)]
        let open_files = std::fs::read_dir("/dev/fd")
            .map(|entries| entries.count())
            .unwrap_or_default();
        #[cfg(not(unix))]
        let open_files = 0_usize;
        let mut state = self.state.lock().expect("benchmark state");
        state.max_open_files = state.max_open_files.max(open_files);
        Ok(())
    }
}

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn service(root: &Path) -> WorkspaceService {
    WorkspaceService::new(
        &[root.to_path_buf()],
        PolicyEngine::new(
            Some(ExecutionPolicy {
                default: PolicyDecision::Allow,
                per_agent_tool: BTreeMap::new(),
                per_root: BTreeMap::new(),
            }),
            Arc::new(Approve),
        ),
    )
    .expect("workspace")
}

fn transfer(source: &Path, destination: &Path) -> FsTransferRequest {
    FsTransferRequest {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        conflict_policy: FsConflictPolicy::Error,
        atomic_publish: true,
        verify: FsVerifyMode::Content,
        preserve_metadata: true,
        follow_symlinks: false,
        dry_run: false,
        expected_source_version: None,
        expected_destination_version: None,
        budget: FsMutationBudget::default(),
    }
}

fn delete_request(path: &Path, mode: FsDeleteMode) -> FsDeleteRequest {
    FsDeleteRequest {
        path: path.to_path_buf(),
        recursive: true,
        mode,
        expected_version: None,
        dry_run: false,
        budget: FsMutationBudget::default(),
    }
}

#[tokio::test]
async fn recursive_copy_is_verified_and_published_without_staging_residue() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir(root.join("source")).expect("source directory");
    std::fs::write(root.join("source/a.txt"), "alpha").expect("source file");
    std::fs::create_dir(root.join("source/nested")).expect("nested directory");
    std::fs::write(root.join("source/nested/b.txt"), "beta").expect("nested file");
    let workspace = service(root);

    let result = workspace
        .copy_safe(
            &OperationContext::new("copy", "agent", "fs_copy"),
            &transfer(&root.join("source"), &root.join("destination")),
        )
        .await
        .expect("safe copy");

    assert_eq!(result.state, "completed");
    assert!(result.destination_published);
    assert!(result.verified);
    assert_eq!(result.files_processed, 2);
    assert_eq!(
        std::fs::read_to_string(root.join("destination/nested/b.txt")).expect("copied file"),
        "beta"
    );
    assert!(
        !std::fs::read_dir(root).expect("root entries").any(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".chatcmd-stage-")
        })
    );
}

#[tokio::test]
async fn replace_keeps_old_destination_until_staged_copy_is_ready() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let workspace = service(root);
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;

    let result = workspace
        .copy_safe(
            &OperationContext::new("replace", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect("replace");

    assert_eq!(result.state, "completed");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination"),
        "new"
    );
    assert!(root.join("source.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_destination_replacement_is_detected_before_publish() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new-from-copy").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let workspace = service(root).with_mutation_fault_injector(Arc::new(GateAtPhase {
        point: "readyToPublish",
        entered: entered.clone(),
        resume: resume.clone(),
    }));
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;
    let operation = tokio::spawn(async move {
        workspace
            .copy_safe(
                &OperationContext::new("concurrent-destination", "agent", "fs_copy"),
                &request,
            )
            .await
            .expect("typed mutation result")
    });

    tokio::task::spawn_blocking(move || entered.wait())
        .await
        .expect("wait for readyToPublish");
    std::fs::write(root.join("destination.txt"), "concurrent-writer").expect("concurrent write");
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .expect("resume mutation");
    let result = operation.await.expect("copy task");

    assert_eq!(result.state, "failedRolledBack");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination"),
        "concurrent-writer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_at_publish_boundary_rolls_back_with_bounded_latency() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let workspace = service(root).with_mutation_fault_injector(Arc::new(GateAtPhase {
        point: "readyToPublish",
        entered: entered.clone(),
        resume: resume.clone(),
    }));
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;
    let context = OperationContext::new("cancel-boundary", "agent", "fs_copy");
    let cancellation = context.cancellation.clone();
    let operation = tokio::spawn(async move { workspace.copy_safe(&context, &request).await });

    tokio::task::spawn_blocking(move || entered.wait())
        .await
        .expect("wait for readyToPublish");
    let cancelled_at = std::time::Instant::now();
    cancellation.cancel();
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .expect("resume mutation");
    let result = operation
        .await
        .expect("copy task")
        .expect("typed cancellation result");
    let latency = cancelled_at.elapsed();

    eprintln!("PLAN12_CANCEL_LATENCY_MS={}", latency.as_millis());
    assert_eq!(result.state, "cancelledRolledBack");
    assert!(latency < std::time::Duration::from_secs(2));
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination"),
        "old"
    );
}

#[tokio::test]
async fn deterministic_fault_after_bytes_rolls_back_staging_and_keeps_old_destination() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.bin"), vec![7_u8; 2 * 1024 * 1024]).expect("source");
    std::fs::write(root.join("destination.bin"), "old").expect("destination");
    let workspace =
        service(root).with_mutation_fault_injector(Arc::new(FailAfterBytes(1024 * 1024)));
    let mut request = transfer(&root.join("source.bin"), &root.join("destination.bin"));
    request.conflict_policy = FsConflictPolicy::Replace;

    let result = workspace
        .copy_safe(
            &OperationContext::new("fault", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect("typed failed result");

    assert_eq!(result.state, "failedRolledBack");
    assert!(result.rollback_attempted);
    assert!(result.rollback_completed);
    assert_eq!(
        std::fs::read_to_string(root.join("destination.bin")).expect("old destination"),
        "old"
    );
    assert!(
        !std::fs::read_dir(root).expect("root entries").any(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".chatcmd-stage-")
        })
    );
}

#[tokio::test]
async fn phase_fault_persists_recoverable_journal_before_publish() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let workspace =
        service(root).with_mutation_fault_injector(Arc::new(FailAtPhase("readyToPublish")));
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;

    let error = workspace
        .copy_safe(
            &OperationContext::new("phase-fault", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect_err("injected phase fault");
    assert_eq!(error.code, "fault_injected");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("old destination"),
        "old"
    );
    assert!(std::fs::read_dir(root).expect("root entries").any(|entry| {
        entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".chatcmd-operation-")
    }));

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("startup recovery");
    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination after recovery"),
        "old"
    );
    assert!(
        !std::fs::read_dir(root).expect("root entries").any(|entry| {
            let name = entry.expect("entry").file_name();
            let name = name.to_string_lossy();
            name.starts_with(".chatcmd-stage-") || name.starts_with(".chatcmd-operation-")
        })
    );
}

#[tokio::test]
async fn crash_after_backup_rename_restores_old_destination() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let workspace = service(root)
        .with_mutation_fault_injector(Arc::new(FailAtPhase("afterBackupRenameBeforeJournal")));
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;

    let error = workspace
        .copy_safe(
            &OperationContext::new("backup-crash", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect_err("injected crash after backup rename");
    assert_eq!(error.code, "fault_injected");
    assert!(!root.join("destination.txt").exists());

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("startup recovery");
    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("restored destination"),
        "old"
    );
}

#[tokio::test]
async fn crash_after_publish_before_journal_keeps_published_destination() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let workspace = service(root)
        .with_mutation_fault_injector(Arc::new(FailAtPhase("afterPublishBeforeJournal")));
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.conflict_policy = FsConflictPolicy::Replace;

    let error = workspace
        .copy_safe(
            &OperationContext::new("publish-crash", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect_err("injected crash after publish");
    assert_eq!(error.code, "fault_injected");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("published destination"),
        "new"
    );

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("startup recovery");
    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination after recovery"),
        "new"
    );
    assert!(root.join("source.txt").exists());
}

#[tokio::test]
async fn move_crash_before_source_removal_keeps_both_copies_recoverable() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "payload").expect("source");
    let workspace =
        service(root).with_mutation_fault_injector(Arc::new(FailAtPhase("removingSource")));

    let error = workspace
        .move_safe(
            &OperationContext::new("move-source-cleanup-crash", "agent", "fs_move"),
            &transfer(&root.join("source.txt"), &root.join("destination.txt")),
        )
        .await
        .expect_err("fault before source removal");
    assert_eq!(error.code, "fault_injected");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("published destination"),
        "payload"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).expect("source remains"),
        "payload"
    );

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("startup recovery");
    assert_eq!(recovered, 1);
    assert!(root.join("source.txt").exists());
    assert!(root.join("destination.txt").exists());
}

#[tokio::test]
async fn move_publishes_before_removing_source() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "payload").expect("source");
    let workspace = service(root);

    let result = workspace
        .move_safe(
            &OperationContext::new("move", "agent", "fs_move"),
            &transfer(&root.join("source.txt"), &root.join("destination.txt")),
        )
        .await
        .expect("move");

    assert_eq!(result.state, "completed");
    assert!(result.source_removed);
    assert!(!root.join("source.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination"),
        "payload"
    );
}

#[tokio::test]
async fn dry_run_and_precancel_leave_the_tree_unchanged() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "payload").expect("source");
    let workspace = service(root);
    let mut request = transfer(&root.join("source.txt"), &root.join("destination.txt"));
    request.dry_run = true;
    let dry_run = workspace
        .copy_safe(&OperationContext::new("dry", "agent", "fs_copy"), &request)
        .await
        .expect("dry run");
    assert_eq!(dry_run.state, "planned");
    assert!(!root.join("destination.txt").exists());

    request.dry_run = false;
    let context = OperationContext::new("cancel", "agent", "fs_copy");
    context.cancellation.cancel();
    let cancelled = workspace
        .copy_safe(&context, &request)
        .await
        .expect("cancelled result");
    assert_eq!(cancelled.state, "cancelledNoChange");
    assert!(!root.join("destination.txt").exists());
}

#[tokio::test]
async fn ancestor_transfers_are_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir(root.join("source")).expect("source");
    let workspace = service(root);
    let error = workspace
        .copy_safe(
            &OperationContext::new("overlap", "agent", "fs_copy"),
            &transfer(&root.join("source"), &root.join("source/child")),
        )
        .await
        .expect_err("overlap must fail");
    assert_eq!(error.code, "overlapping_transfer");
}

#[tokio::test]
async fn quarantine_crash_recovery_preserves_quarantined_data() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir(root.join("quarantine-crash")).expect("directory");
    std::fs::write(root.join("quarantine-crash/file.txt"), "keep").expect("file");
    let workspace = service(root).with_mutation_fault_injector(Arc::new(FailAtPhase("completed")));

    let error = workspace
        .delete_safe(
            &OperationContext::new("quarantine-crash", "agent", "fs_delete"),
            &delete_request(&root.join("quarantine-crash"), FsDeleteMode::Quarantine),
        )
        .await
        .expect_err("fault after quarantine rename");
    assert_eq!(error.code, "fault_injected");
    assert!(!root.join("quarantine-crash").exists());
    let quarantine_path = std::fs::read_dir(root)
        .expect("root entries")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".chatcmd-quarantine-"))
        })
        .expect("quarantine path");
    assert_eq!(
        std::fs::read_to_string(quarantine_path.join("file.txt")).expect("quarantined file"),
        "keep"
    );

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("startup recovery");
    assert_eq!(recovered, 1);
    assert!(
        quarantine_path.exists(),
        "recovery must preserve quarantine"
    );
    assert_eq!(
        std::fs::read_to_string(quarantine_path.join("file.txt")).expect("quarantined file"),
        "keep"
    );
    assert!(
        !std::fs::read_dir(root).expect("root entries").any(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".chatcmd-operation-")
        })
    );
}

#[tokio::test]
async fn delete_supports_quarantine_and_explicit_permanent_mode() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir(root.join("quarantine-me")).expect("directory");
    std::fs::write(root.join("quarantine-me/file.txt"), "keep").expect("file");
    std::fs::write(root.join("permanent.txt"), "remove").expect("file");
    let workspace = service(root);

    let quarantined = workspace
        .delete_safe(
            &OperationContext::new("quarantine", "agent", "fs_delete"),
            &delete_request(&root.join("quarantine-me"), FsDeleteMode::Quarantine),
        )
        .await
        .expect("quarantine");
    assert_eq!(quarantined.state, "completed");
    assert!(!root.join("quarantine-me").exists());
    assert!(
        quarantined
            .warnings
            .iter()
            .any(|warning| warning.contains("quarantined data retained"))
    );

    let permanent = workspace
        .delete_safe(
            &OperationContext::new("permanent", "agent", "fs_delete"),
            &delete_request(&root.join("permanent.txt"), FsDeleteMode::Permanent),
        )
        .await
        .expect("permanent delete");
    assert_eq!(permanent.state, "completed");
    assert!(!root.join("permanent.txt").exists());
}

#[tokio::test]
async fn legacy_overwrite_adapter_remains_compatible() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::write(root.join("source.txt"), "new").expect("source");
    std::fs::write(root.join("destination.txt"), "old").expect("destination");
    let workspace = service(root);
    workspace
        .copy(
            &OperationContext::new("legacy", "agent", "fs_copy"),
            &root.join("source.txt"),
            &root.join("destination.txt"),
            true,
        )
        .await
        .expect("legacy copy");
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("destination"),
        "new"
    );
}

#[tokio::test]
async fn quarantine_can_be_restored_and_non_managed_paths_are_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir(root.join("restore-me")).expect("directory");
    std::fs::write(root.join("restore-me/file.txt"), "payload").expect("file");
    let workspace = service(root);

    let quarantined = workspace
        .delete_safe(
            &OperationContext::new("quarantine-restore", "agent", "fs_delete"),
            &delete_request(&root.join("restore-me"), FsDeleteMode::Quarantine),
        )
        .await
        .expect("quarantine");
    let quarantine_path = quarantined
        .warnings
        .iter()
        .find_map(|warning| warning.strip_prefix("quarantined data retained at "))
        .map(std::path::PathBuf::from)
        .expect("quarantine path");

    let restored = workspace
        .restore_quarantine(
            &OperationContext::new("restore", "agent", "fs_restore_quarantine"),
            &FsQuarantineRestoreRequest {
                quarantine_path: quarantine_path.clone(),
                destination: root.join("restored"),
                replace: false,
            },
        )
        .await
        .expect("restore");
    assert_eq!(restored.state, "completed");
    assert!(!quarantine_path.exists());
    assert_eq!(
        std::fs::read_to_string(root.join("restored/file.txt")).expect("restored file"),
        "payload"
    );

    std::fs::write(root.join("ordinary.txt"), "ordinary").expect("ordinary file");
    let error = workspace
        .restore_quarantine(
            &OperationContext::new("invalid-restore", "agent", "fs_restore_quarantine"),
            &FsQuarantineRestoreRequest {
                quarantine_path: root.join("ordinary.txt"),
                destination: root.join("ordinary-restored.txt"),
                replace: false,
            },
        )
        .await
        .expect_err("ordinary path must not restore as quarantine");
    assert_eq!(error.code, "invalid_quarantine_path");
}

#[tokio::test]
async fn quarantine_gc_supports_dry_run_retention_and_quota_cleanup() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    let first = root.join(".chatcmd-quarantine-first");
    let second = root.join(".chatcmd-quarantine-second");
    std::fs::create_dir(&first).expect("first quarantine");
    std::fs::create_dir(&second).expect("second quarantine");
    std::fs::write(first.join("a.bin"), vec![1_u8; 8]).expect("first file");
    std::fs::write(second.join("b.bin"), vec![2_u8; 12]).expect("second file");
    let workspace = service(root);

    let dry_run = workspace
        .quarantine_gc(
            &OperationContext::new("gc-dry", "agent", "fs_quarantine_gc"),
            &FsQuarantineGcRequest {
                path: root.to_path_buf(),
                retention_seconds: 0,
                max_total_bytes: u64::MAX,
                max_items: 10,
                dry_run: true,
            },
        )
        .await
        .expect("dry-run GC");
    assert_eq!(dry_run.scanned_items, 2);
    assert_eq!(dry_run.removed_items, 2);
    assert!(first.exists() && second.exists());

    let cleaned = workspace
        .quarantine_gc(
            &OperationContext::new("gc", "agent", "fs_quarantine_gc"),
            &FsQuarantineGcRequest {
                path: root.to_path_buf(),
                retention_seconds: u64::MAX,
                max_total_bytes: 10,
                max_items: 10,
                dry_run: false,
            },
        )
        .await
        .expect("quota GC");
    assert_eq!(cleaned.scanned_items, 2);
    assert!(cleaned.removed_items >= 1);
    assert!(cleaned.retained_bytes <= 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual Plan 12 benchmark: creates and copies 100,000 small files"]
async fn mutation_100k_small_files_reports_resource_metrics() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    let source = root.join("source");
    std::fs::create_dir(&source).expect("source directory");
    for group in 0..100_u32 {
        let group_dir = source.join(format!("g-{group:03}"));
        std::fs::create_dir(&group_dir).expect("group directory");
        for file in 0..1_000_u32 {
            std::fs::write(group_dir.join(format!("f-{file:04}.txt")), b"x")
                .expect("small benchmark file");
        }
    }
    let probe = Arc::new(BenchmarkProbe::default());
    let workspace = service(root)
        .with_mutation_journal_sink(probe.clone())
        .with_mutation_fault_injector(probe.clone());
    let mut request = transfer(&source, &root.join("destination"));
    request.verify = FsVerifyMode::Metadata;
    request.budget.timeout_ms = 900_000;
    let started = std::time::Instant::now();
    let result = workspace
        .copy_safe(
            &OperationContext::new("bench-100k", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect("100k copy");
    let elapsed = started.elapsed();
    let state = probe.state.lock().expect("benchmark state");
    let files_per_second = 100_000_f64 / elapsed.as_secs_f64().max(f64::EPSILON);
    eprintln!(
        "PLAN12_BENCH files=100000 elapsed_ms={} files_per_second={:.2} max_open_files={} journal_transitions={} max_journal_bytes={}",
        elapsed.as_millis(),
        files_per_second,
        state.max_open_files,
        state.journal_transitions,
        state.max_journal_bytes,
    );
    assert_eq!(result.state, "completed");
    assert_eq!(result.files_processed, 100_000);
    assert!(state.max_open_files < 128, "open files must remain bounded");
    assert!(
        state.max_journal_bytes < 64 * 1024,
        "journal must remain bounded"
    );
}

#[tokio::test]
#[ignore = "manual Plan 12 benchmark: logical 10 GiB sparse-file preflight only"]
async fn mutation_10gb_sparse_preflight_reports_resource_metrics() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    let source = root.join("logical-10gb.sparse");
    let file = std::fs::File::create(&source).expect("sparse fixture");
    file.set_len(10 * 1024 * 1024 * 1024)
        .expect("set sparse logical length");
    drop(file);

    let mut request = transfer(&source, &root.join("destination.sparse"));
    request.verify = FsVerifyMode::Metadata;
    request.dry_run = true;
    request.budget.timeout_ms = 60_000;
    request.budget.max_bytes_read = 16 * 1024 * 1024 * 1024;
    request.budget.max_bytes_written = 16 * 1024 * 1024 * 1024;
    let started = std::time::Instant::now();
    let result = service(root)
        .copy_safe(
            &OperationContext::new("bench-10gb-sparse", "agent", "fs_copy"),
            &request,
        )
        .await
        .expect("10 GiB sparse preflight");
    let elapsed = started.elapsed();
    eprintln!(
        "PLAN12_SPARSE_BENCH logical_bytes={} elapsed_ms={} state={}",
        std::fs::metadata(&source).expect("sparse metadata").len(),
        elapsed.as_millis(),
        result.state
    );
    assert_eq!(result.state, "planned");
    assert_eq!(result.files_processed, 1);
    assert!(!root.join("destination.sparse").exists());
}

#[tokio::test]
async fn startup_recovery_uses_durable_journal_when_sidecar_is_missing() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical temp root");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let stage = root.join(".chatcmd-stage-sqlite-only");
    let backup = root.join(".chatcmd-backup-sqlite-only");
    std::fs::write(&source, "new").expect("source");
    std::fs::write(&stage, "partial").expect("stage");
    std::fs::write(&backup, "old").expect("backup");
    let row = serde_json::json!({
        "operationId": "sqlite-only",
        "operationType": "copy",
        "ownerAgent": "agent",
        "ownerTask": null,
        "source": source,
        "destination": destination,
        "stagingPath": stage,
        "backupPath": backup,
        "requestedOptions": { "verify": "metadata" },
        "phase": "readyToPublish",
        "counts": { "files": 1, "directories": 0, "bytes": 7 },
        "backupCreated": true,
        "rollbackActions": ["remove staging path", "restore destination backup"],
        "warnings": [],
        "error": null,
        "updatedAtUnixMs": 1
    })
    .to_string();
    let sink = Arc::new(RecoveryJournalSink::with_row(row));
    let workspace = service(&root).with_mutation_journal_sink(sink.clone());

    let recovered = workspace
        .recover_interrupted_mutations()
        .await
        .expect("durable startup recovery");
    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("destination.txt")).expect("restored destination"),
        "old"
    );
    assert!(!root.join(".chatcmd-stage-sqlite-only").exists());
    assert!(sink.list_json().expect("remaining durable rows").is_empty());
}

#[tokio::test]
async fn startup_recovery_restores_backup_and_removes_staging_for_unpublished_operation() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let root = workspace.roots()[0].clone();
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let stage = root.join(".chatcmd-stage-recovery");
    let backup = root.join(".chatcmd-backup-recovery");
    let journal = root.join(".chatcmd-operation-recovery.json");
    std::fs::write(&source, "new").expect("source");
    std::fs::write(&stage, "partial").expect("stage");
    std::fs::write(&backup, "old").expect("backup");
    let payload = serde_json::json!({
        "operationId": "recovery",
        "operationType": "copy",
        "ownerAgent": "agent",
        "ownerTask": null,
        "source": source,
        "destination": destination,
        "stagingPath": stage,
        "backupPath": backup,
        "phase": "readyToPublish",
        "counts": { "files": 1, "directories": 0, "bytes": 7 },
        "backupCreated": true,
        "rollbackActions": ["remove staging path", "restore destination backup"],
        "warnings": [],
        "error": null,
        "updatedAtUnixMs": 0
    });
    std::fs::write(
        &journal,
        serde_json::to_vec_pretty(&payload).expect("journal json"),
    )
    .expect("journal");

    let recovered = workspace
        .recover_interrupted_mutations()
        .await
        .expect("recovery");

    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(&destination).expect("restored destination"),
        "old"
    );
    assert!(!stage.exists());
    assert!(!backup.exists());
    assert!(!journal.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn startup_recovery_skips_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temp directory");
    let outside = tempfile::tempdir().expect("outside directory");
    let root = directory.path();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, "safe").expect("outside fixture");
    symlink(&outside_file, root.join("ordinary-symlink")).expect("create symlink");

    let recovered = service(root)
        .recover_interrupted_mutations()
        .await
        .expect("recovery should ignore symlinks");

    assert_eq!(recovered, 0);
    assert_eq!(
        std::fs::read_to_string(&outside_file).expect("outside file"),
        "safe"
    );
}

#[tokio::test]
async fn startup_recovery_rejects_journal_paths_outside_workspace() {
    let directory = tempfile::tempdir().expect("temp directory");
    let outside = tempfile::tempdir().expect("outside directory");
    let root = directory.path();
    let outside_file = outside.path().join("do-not-touch.txt");
    std::fs::write(&outside_file, "safe").expect("outside fixture");
    let journal = root.join(".chatcmd-operation-escape.json");
    let payload = serde_json::json!({
        "operationId": "escape",
        "operationType": "copy",
        "ownerAgent": "agent",
        "ownerTask": null,
        "source": root.join("source.txt"),
        "destination": root.join("destination.txt"),
        "stagingPath": outside_file,
        "backupPath": root.join("backup.txt"),
        "phase": "staging",
        "counts": { "files": 0, "directories": 0, "bytes": 0 },
        "backupCreated": false,
        "rollbackActions": [],
        "warnings": [],
        "error": null,
        "updatedAtUnixMs": 0
    });
    std::fs::write(
        &journal,
        serde_json::to_vec_pretty(&payload).expect("journal json"),
    )
    .expect("journal");

    let error = service(root)
        .recover_interrupted_mutations()
        .await
        .expect_err("escaped journal must fail");

    assert_eq!(error.code, "journal_path_escape");
    assert_eq!(
        std::fs::read_to_string(&outside_file).expect("outside file"),
        "safe"
    );
}
