use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, FsBatchReadRequest, FsBatchStatRequest, OperationContext,
    PolicyContext, PolicyEngine, RuntimeResult, TextReadRange, TextReadRequestV2, WorkspaceService,
};
use std::{path::Path, sync::Arc};

struct Reject;
impl ApprovalDecision for Reject {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(false) })
    }
}

fn service(root: &Path) -> WorkspaceService {
    WorkspaceService::new(
        &[root.to_path_buf()],
        PolicyEngine::new(None, Arc::new(Reject)),
    )
    .expect("workspace")
}

#[tokio::test]
async fn rebuild_publishes_generation_and_mutation_marks_stale() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("one.txt"), "one").expect("write");
    let workspace = service(temp.path());
    let context = OperationContext::new("index", "agent", "workspace_index_rebuild");

    let first = workspace
        .rebuild_index(&context, temp.path())
        .await
        .expect("rebuild");
    assert_eq!(first.generation, 1);
    assert!(first.entry_count >= 2);

    workspace.mark_index_stale(&temp.path().join("one.txt"));
    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
    let second = workspace
        .rebuild_index(&context, temp.path())
        .await
        .expect("rebuild again");
    assert_eq!(second.generation, 2);
}

#[tokio::test]
async fn batches_preserve_order_errors_duplicates_and_output_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = temp.path().join("first.txt");
    let second = temp.path().join("second.txt");
    std::fs::write(&first, "first").expect("write first");
    std::fs::write(&second, "second").expect("write second");
    let missing = temp.path().join("missing.txt");
    let workspace = service(temp.path());
    let context = OperationContext::new("batch", "agent", "fs_batch_stat");

    let stats = workspace
        .batch_stat(
            &context,
            &FsBatchStatRequest {
                paths: vec![first.clone(), missing.clone(), first.clone()],
                version_strength: Default::default(),
                max_items: 3,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch stat");
    assert_eq!(
        stats
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>(),
        vec![first.clone(), missing, first.clone()]
    );
    assert_eq!((stats.usage.succeeded, stats.usage.failed), (2, 1));

    let read = |path| TextReadRequestV2 {
        path,
        range: TextReadRange::Byte {
            start: 0,
            limit: 32,
        },
        max_bytes: 32,
        include_line_endings: true,
        expected_version: None,
        budget: Default::default(),
    };
    let reads = workspace
        .batch_read(
            &context,
            &FsBatchReadRequest {
                requests: vec![read(first), read(second)],
                max_items: 2,
                max_total_output_bytes: 5,
                concurrency: 2,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch read");
    assert_eq!(reads.items.len(), 2);
    assert!(reads.items[0].ok);
    assert!(!reads.items[1].ok);
    assert!(reads.truncated);
}
