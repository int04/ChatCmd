use chatcmd_runtime::{
    ApprovalDecision, AtomicWriteOptions, BoxFuture, ExecutionPolicy, FsStatBudget, FsStatRequest,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, VersionStrength,
    WorkspaceService,
};
#[cfg(unix)]
use chatcmd_runtime::{DurabilityMode, MetadataPolicy};
use std::{collections::BTreeMap, sync::Arc};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn service(root: &std::path::Path) -> WorkspaceService {
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
    .expect("workspace service")
}

async fn version(workspace: &WorkspaceService, path: &std::path::Path) -> String {
    workspace
        .stat_v2(
            None,
            &FsStatRequest {
                path: path.to_path_buf(),
                version_strength: VersionStrength::Metadata,
                hash_algorithm: None,
                budget: FsStatBudget::default(),
            },
        )
        .await
        .expect("stat")
        .version_token
}

#[tokio::test]
async fn create_is_no_clobber_and_reports_commit_details() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let path = directory.path().join("created.txt");
    let context = OperationContext::new("create", "agent", "fs_write_text");

    let result = workspace
        .write_text_atomic(&context, &path, "complete", AtomicWriteOptions::default())
        .await
        .expect("create file");

    assert!(result.committed);
    assert!(result.created);
    assert!(result.atomic);
    assert_eq!(result.bytes_written, 8);
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "complete");
    let error = workspace
        .write_text_atomic(&context, &path, "clobber", AtomicWriteOptions::default())
        .await
        .expect_err("no-clobber create");
    assert_eq!(error.code, "already_exists");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "complete");
}

#[tokio::test]
async fn overwrite_rejects_stale_version_without_changing_target() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let path = directory.path().join("versioned.txt");
    std::fs::write(&path, "old").expect("seed file");
    let stale = version(&workspace, &path).await;
    std::fs::write(&path, "concurrent").expect("concurrent writer");
    let context = OperationContext::new("overwrite", "agent", "fs_write_text");
    let options = AtomicWriteOptions {
        overwrite: true,
        expected_version: Some(stale),
        ..AtomicWriteOptions::default()
    };

    let error = workspace
        .write_text_atomic(&context, &path, "new", options)
        .await
        .expect_err("stale version");

    assert!(matches!(
        error.code.as_str(),
        "versionMismatch" | "targetReplaced"
    ));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "concurrent");
}

#[tokio::test]
async fn cancellation_keeps_complete_old_target_and_cleans_temporary() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let path = directory.path().join("cancelled.txt");
    std::fs::write(&path, "old-complete").expect("seed file");
    let context = OperationContext::new("cancel", "agent", "fs_write_text");
    context.cancellation.cancel();

    let error = workspace
        .write_text_atomic(
            &context,
            &path,
            "new",
            AtomicWriteOptions {
                overwrite: true,
                ..AtomicWriteOptions::default()
            },
        )
        .await
        .expect_err("cancelled write");

    assert_eq!(error.code, "operationCancelled");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "old-complete"
    );
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list").count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn overwrite_preserves_posix_mode_and_full_durability() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let path = directory.path().join("script.sh");
    std::fs::write(&path, "old").expect("seed file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).expect("chmod");
    let context = OperationContext::new("mode", "agent", "fs_write_text");

    let result = workspace
        .write_text_atomic(
            &context,
            &path,
            "new",
            AtomicWriteOptions {
                overwrite: true,
                metadata_policy: MetadataPolicy::Preserve,
                durability: DurabilityMode::Full,
                ..AtomicWriteOptions::default()
            },
        )
        .await
        .expect("overwrite");

    assert!(result.metadata_preserved);
    assert_eq!(result.durability_achieved, DurabilityMode::Full);
    assert_eq!(
        std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
}
