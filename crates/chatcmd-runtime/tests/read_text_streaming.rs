use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, OperationContext, PolicyDecision, PolicyEngine,
    RuntimeResult, TextReadBudget, TextReadRange, TextReadRequestV2, WorkspaceService,
};
use std::{collections::BTreeMap, fs::OpenOptions, io::Write, path::PathBuf, sync::Arc};

struct Approve;
impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn workspace(root: PathBuf) -> WorkspaceService {
    WorkspaceService::new(
        &[root],
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

fn request(path: PathBuf, range: TextReadRange, max_bytes: usize) -> TextReadRequestV2 {
    TextReadRequestV2 {
        path,
        range,
        max_bytes,
        include_line_endings: true,
        expected_version: None,
        budget: TextReadBudget {
            timeout_ms: 30_000,
            max_bytes_read: 256 * 1024 * 1024,
        },
    }
}

#[tokio::test]
async fn newline_bom_and_long_line_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let mixed = dir.path().join("mixed.txt");
    std::fs::write(&mixed, b"\xEF\xBB\xBFone\r\ntwo\rthree\nfour").unwrap();
    let ws = workspace(dir.path().to_path_buf());
    let result = ws
        .read_text_v2(
            None,
            &request(mixed, TextReadRange::Line { start: 1, limit: 4 }, 1024),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "one\r\ntwo\rthree\nfour");
    assert!(result.bom);
    assert_eq!(result.line_ending, "mixed");
    assert_eq!(result.total_lines, Some(4));

    let long = dir.path().join("long.txt");
    let mut file = std::fs::File::create(&long).unwrap();
    for _ in 0..1024 {
        file.write_all(&[b'a'; 1024]).unwrap();
    }
    file.write_all(b"\nnext\n").unwrap();
    let result = ws
        .read_text_v2(
            None,
            &request(long, TextReadRange::Line { start: 1, limit: 1 }, 4096),
        )
        .await
        .unwrap();
    assert!(result.truncated);
    assert_eq!(result.truncation_reason.as_deref(), Some("outputLimit"));
    assert!(result.content.len() <= 4096);
    assert!(result.bytes_read <= 4097);
}

#[tokio::test]
async fn utf8_boundaries_invalid_bytes_and_budget_are_safe() {
    let dir = tempfile::tempdir().unwrap();
    let utf8 = dir.path().join("utf8.txt");
    std::fs::write(&utf8, "a😀béz").unwrap();
    let ws = workspace(dir.path().to_path_buf());
    let result = ws
        .read_text_v2(
            None,
            &request(utf8, TextReadRange::Byte { start: 2, limit: 5 }, 5),
        )
        .await
        .unwrap();
    assert!(std::str::from_utf8(result.content.as_bytes()).is_ok());
    assert!(result.range.start_byte.unwrap() >= 2);

    let invalid = dir.path().join("invalid.txt");
    std::fs::write(&invalid, [0xff, b'\n', b'o', b'k', b'\n', 0xff]).unwrap();
    let result = ws
        .read_text_v2(
            None,
            &request(
                invalid.clone(),
                TextReadRange::Line { start: 2, limit: 1 },
                32,
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "ok\n");
    assert_eq!(
        ws.read_text_v2(
            None,
            &request(invalid, TextReadRange::Line { start: 1, limit: 1 }, 32)
        )
        .await
        .unwrap_err()
        .code,
        "invalid_utf8"
    );

    let deep = dir.path().join("deep.txt");
    std::fs::write(&deep, vec![b'a'; 1024 * 1024]).unwrap();
    let context = OperationContext::new("r", "agent", "fs_read_text_v2");
    context.cancellation.cancel();
    assert_eq!(
        ws.read_text_v2(
            Some(&context),
            &request(deep.clone(), TextReadRange::Line { start: 2, limit: 1 }, 32)
        )
        .await
        .unwrap_err()
        .code,
        "cancelled"
    );
    let mut bounded = request(deep, TextReadRange::Line { start: 2, limit: 1 }, 32);
    bounded.budget.max_bytes_read = 4096;
    let result = ws.read_text_v2(None, &bounded).await.unwrap();
    assert_eq!(result.truncation_reason.as_deref(), Some("readBudget"));
    assert!(result.bytes_read <= 4096);
}

#[tokio::test]
async fn stale_version_rejects_mutations() {
    for mutation in ["append", "truncate", "replace"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version.txt");
        std::fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let ws = workspace(dir.path().to_path_buf());
        let first = ws
            .read_text_v2(
                None,
                &request(path.clone(), TextReadRange::Byte { start: 0, limit: 4 }, 4),
            )
            .await
            .unwrap();
        match mutation {
            "append" => OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"four\n")
                .unwrap(),
            "truncate" => std::fs::write(&path, b"one\n").unwrap(),
            "replace" => {
                let replacement = dir.path().join("replacement.txt");
                std::fs::write(&replacement, b"ONE\nTWO\nTHREE\n").unwrap();
                std::fs::remove_file(&path).unwrap();
                std::fs::rename(replacement, &path).unwrap();
            }
            _ => unreachable!(),
        }
        let mut next = request(path, TextReadRange::Byte { start: 4, limit: 4 }, 4);
        next.expected_version = Some(first.version_token);
        assert_eq!(
            ws.read_text_v2(None, &next).await.unwrap_err().code,
            "version_mismatch",
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn sparse_one_gibibyte_first_middle_end_reads_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse.bin");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.write_all(b"head\n").unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    drop(file);
    let ws = workspace(dir.path().to_path_buf());
    let first = ws
        .read_text_v2(
            None,
            &request(path.clone(), TextReadRange::Line { start: 1, limit: 1 }, 64),
        )
        .await
        .unwrap();
    assert_eq!(first.content, "head\n");
    assert!(first.bytes_read < 128);
    assert_eq!(first.size_bytes, 1024 * 1024 * 1024);
    for start in [512 * 1024 * 1024, 1024 * 1024 * 1024 - 32] {
        let result = ws
            .read_text_v2(
                None,
                &request(path.clone(), TextReadRange::Byte { start, limit: 16 }, 16),
            )
            .await
            .unwrap();
        assert!(result.bytes_read <= 27);
        assert!(result.content.len() <= 16);
    }
}

#[tokio::test]
async fn legacy_adapter_preserves_full_read_and_range_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.txt");
    std::fs::write(&path, b"one\r\ntwo\r\nthree\r\n").unwrap();
    let ws = workspace(dir.path().to_path_buf());

    let full = ws.read_text(&path, 1024).await.unwrap();
    assert_eq!(full.content, "one\r\ntwo\r\nthree\r\n");
    assert_eq!(full.total_lines, 3);
    assert!(!full.truncated);

    let range = ws.read_text_range(&path, 1024, 2, Some(1)).await.unwrap();
    assert_eq!(range.content, "two");
    assert_eq!(range.start_line, 2);
    assert_eq!(range.end_line, 2);
    assert_eq!(range.total_lines, 3);

    let truncated = ws.read_text(&path, 5).await.unwrap();
    assert!(truncated.truncated);
    assert_eq!(truncated.total_lines, 3);
}

#[tokio::test]
async fn eof_newline_metadata_and_utf8_boundaries_are_exact() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path().to_path_buf());
    for (name, bytes, expected_lines, ending) in [
        ("empty", b"".as_slice(), 0, "none"),
        ("none", b"one".as_slice(), 1, "none"),
        ("lf", b"one\n".as_slice(), 1, "lf"),
        ("crlf", b"one\r\n".as_slice(), 1, "crlf"),
        ("cr", b"one\r".as_slice(), 1, "cr"),
        ("mixed", b"a\nb\r\nc\r".as_slice(), 3, "mixed"),
    ] {
        let path = dir.path().join(format!("{name}.txt"));
        std::fs::write(&path, bytes).unwrap();
        let result = ws
            .read_text_v2(
                None,
                &request(
                    path,
                    TextReadRange::Line {
                        start: 1,
                        limit: expected_lines.max(1),
                    },
                    1024,
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.line_ending, ending, "{name}");
        assert_eq!(result.total_lines, Some(expected_lines), "{name}");
        assert!(result.total_lines_known, "{name}");
        assert_eq!(result.line_ending_detection, "complete", "{name}");
        assert!(!result.truncated, "{name}");
    }

    let utf8 = dir.path().join("emoji.txt");
    std::fs::write(&utf8, "a😀b").unwrap();
    for (start, actual) in [(1_u64, 1_u64), (2, 5), (3, 5), (4, 5), (5, 5)] {
        let result = ws
            .read_text_v2(
                None,
                &request(utf8.clone(), TextReadRange::Byte { start, limit: 8 }, 8),
            )
            .await
            .unwrap();
        assert_eq!(result.range.start_byte, Some(actual), "start={start}");
        assert!(std::str::from_utf8(result.content.as_bytes()).is_ok());
    }

    let invalid_continuation = dir.path().join("bad-continuation.bin");
    std::fs::write(&invalid_continuation, [b'a', 0x80, b'b']).unwrap();
    assert_eq!(
        ws.read_text_v2(
            None,
            &request(
                invalid_continuation,
                TextReadRange::Byte { start: 1, limit: 2 },
                2
            ),
        )
        .await
        .unwrap_err()
        .code,
        "invalid_utf8"
    );
}

#[tokio::test]
async fn invalid_after_range_and_deep_timeout_do_not_over_scan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("after.txt");
    std::fs::write(&path, [b'o', b'k', b'\n', 0xff]).unwrap();
    let ws = workspace(dir.path().to_path_buf());
    let result = ws
        .read_text_v2(
            None,
            &request(path, TextReadRange::Line { start: 1, limit: 1 }, 16),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "ok\n");

    let deep = dir.path().join("timeout.txt");
    let file = std::fs::File::create(&deep).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    let mut timed = request(deep, TextReadRange::Line { start: 2, limit: 1 }, 16);
    timed.budget.timeout_ms = 1;
    assert_eq!(
        ws.read_text_v2(None, &timed).await.unwrap_err().code,
        "timeout"
    );
}
