use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, FsConflictPolicy, FsDeleteMode, FsDeleteRequest,
    FsMutationBudget, FsTransferRequest, FsVerifyMode, OperationContext, PolicyDecision,
    PolicyEngine, RuntimeResult, WorkspaceService,
};
use std::{collections::BTreeMap, path::Path, sync::Arc};

struct Approve;

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
