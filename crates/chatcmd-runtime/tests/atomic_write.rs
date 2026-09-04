#![allow(clippy::result_large_err)]

#[path = "support/process_helper.rs"]
mod process_helper;

use chatcmd_runtime::{
    ApprovalDecision, AtomicWriteOptions, BoxFuture, ExecutionPolicy, FsStatBudget, FsStatRequest,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, VersionStrength,
    WorkspaceService,
};
#[cfg(unix)]
use chatcmd_runtime::{DurabilityMode, MetadataPolicy};
use process_helper::{kill_at_marker, spawn_test_helper};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_race_has_exactly_one_no_clobber_winner() {
    use std::sync::{Arc, Barrier};

    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("create-race.txt");
    let workspace = Arc::new(service(directory.path()));
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();

    for contender in ["writer-a", "writer-b"] {
        let workspace = workspace.clone();
        let path = path.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            tokio::runtime::Handle::current().block_on(workspace.write_text_atomic(
                &OperationContext::new(contender, contender, "fs_write_text"),
                &path,
                contender,
                AtomicWriteOptions::default(),
            ))
        }));
    }

    barrier.wait();
    let first = tasks.remove(0).await.expect("first writer task");
    let second = tasks.remove(0).await.expect("second writer task");
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let loser = if first.is_err() { first } else { second };
    assert!(matches!(
        loser.expect_err("one loser").code.as_str(),
        "writeConflict" | "already_exists"
    ));
    let content = std::fs::read_to_string(path).expect("read winner");
    assert!(matches!(content.as_str(), "writer-a" | "writer-b"));
}

#[tokio::test]
async fn text_write_keeps_bom_and_line_endings_byte_exact() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace = service(directory.path());
    let path = directory.path().join("text-policy.txt");
    let context = OperationContext::new("text-policy", "agent", "fs_write_text");
    let content = "\u{feff}first\r\nsecond\r\n";

    workspace
        .write_text_atomic(&context, &path, content, AtomicWriteOptions::default())
        .await
        .expect("write exact text");

    assert_eq!(
        std::fs::read(&path).expect("read bytes"),
        content.as_bytes()
    );
}

#[test]
fn subprocess_atomic_write_helper() {
    let Some(root) = std::env::var_os("CHATCMD_CRASH_HELPER_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let path = root.join("target.txt");
    let workspace = service(&root);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime
        .block_on(workspace.write_text_atomic(
            &OperationContext::new("crash-helper", "agent", "fs_write_text"),
            &path,
            "new-complete",
            AtomicWriteOptions {
                overwrite: true,
                durability: chatcmd_runtime::DurabilityMode::Full,
                ..AtomicWriteOptions::default()
            },
        ))
        .expect("atomic write helper");
}

#[test]
fn process_kill_before_commit_keeps_old_target_complete() {
    let directory = tempfile::tempdir().expect("temp directory");
    let target = directory.path().join("target.txt");
    std::fs::write(&target, b"old-complete").expect("seed crash target");
    let child = spawn_test_helper("subprocess_atomic_write_helper", directory.path());
    let status = kill_at_marker(child, "CHATCMD_ATOMIC_WRITE_READY_BEFORE_COMMIT");

    assert!(!status.success());
    assert_eq!(
        std::fs::read(&target).expect("read target"),
        b"old-complete"
    );
    let orphan_temps: Vec<_> = std::fs::read_dir(directory.path())
        .expect("list directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".chatcmd-atomic-write-")
        })
        .collect();
    assert_eq!(
        orphan_temps.len(),
        1,
        "crash should leave one identifiable orphan temp"
    );
    std::fs::remove_file(orphan_temps[0].path()).expect("cleanup orphan fixture");
}
