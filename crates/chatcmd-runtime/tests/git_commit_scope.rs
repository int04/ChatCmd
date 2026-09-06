#![allow(clippy::result_large_err)]

use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, GitRunOptions, GitService, GitStructuredOutput,
    PolicyDecision, PolicyEngine, RuntimeResult, WorkspaceService,
};
use std::{collections::BTreeMap, path::Path, process::Command, sync::Arc};
use tokio_util::sync::CancellationToken;

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn workspace(root: &Path) -> WorkspaceService {
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
    .expect("create test workspace")
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp repository");
    git(directory.path(), &["init", "--quiet"]);
    git(
        directory.path(),
        &["config", "user.email", "chatcmd-test@example.invalid"],
    );
    git(directory.path(), &["config", "user.name", "ChatCMD Test"]);
    directory
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write fixture");
}

fn commit_all(cwd: &Path, message: &str) {
    git(cwd, &["add", "--all"]);
    git(cwd, &["commit", "--quiet", "--message", message]);
}

fn service(cwd: &Path) -> GitService {
    GitService::new(workspace(cwd), 64 * 1024)
}

async fn scoped_commit(
    service: &GitService,
    cwd: &Path,
    message: &str,
    paths: &[String],
) -> Result<chatcmd_runtime::CommandOutput, chatcmd_runtime::RuntimeError> {
    service
        .commit_with_options(
            cwd,
            message,
            false,
            paths,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn commit_without_explicit_scope_is_rejected_without_side_effects() {
    let directory = repository();
    write(&directory.path().join("change.txt"), "change\n");
    let before = git(directory.path(), &["status", "--porcelain=v1"]);

    let error = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "no scope",
        &[],
    )
    .await
    .expect_err("missing scope must fail closed");

    assert_eq!(error.code, "commit_scope_required");
    assert_eq!(git(directory.path(), &["status", "--porcelain=v1"]), before);
}

#[tokio::test]
async fn selected_path_with_unrelated_staged_path_preserves_index() {
    let directory = repository();
    write(&directory.path().join("selected.txt"), "base\n");
    write(&directory.path().join("unrelated.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("selected.txt"), "selected\n");
    write(&directory.path().join("unrelated.txt"), "staged\n");
    git(directory.path(), &["add", "--", "unrelated.txt"]);
    let index_before = git(directory.path(), &["diff", "--cached", "--binary"]);
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);

    let error = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "selected only",
        &["selected.txt".to_owned()],
    )
    .await
    .expect_err("unrelated staged content must conflict");

    assert_eq!(error.code, "git_scope_conflict");
    assert_eq!(
        git(directory.path(), &["diff", "--cached", "--binary"]),
        index_before
    );
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
}

#[tokio::test]
async fn scoped_commit_leaves_unrelated_unstaged_and_untracked_files_untouched() {
    let directory = repository();
    write(&directory.path().join("selected.txt"), "base\n");
    write(&directory.path().join("unrelated.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("selected.txt"), "selected\n");
    write(&directory.path().join("unrelated.txt"), "unstaged\n");
    write(&directory.path().join("untracked.txt"), "untracked\n");

    let output = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "selected only",
        &["selected.txt".to_owned()],
    )
    .await
    .expect("scoped commit");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    assert_eq!(
        git(directory.path(), &["show", "HEAD:selected.txt"]),
        "selected\n"
    );
    assert_eq!(
        git(directory.path(), &["show", "HEAD:unrelated.txt"]),
        "base\n"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("unrelated.txt")).expect("unrelated file"),
        "unstaged\n"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("untracked.txt")).expect("untracked file"),
        "untracked\n"
    );
    assert!(matches!(
        output.structured,
        Some(GitStructuredOutput::Commit(_))
    ));
}

#[tokio::test]
async fn selected_path_with_staged_and_unstaged_hunks_fails_closed() {
    let directory = repository();
    write(&directory.path().join("mixed.txt"), "one\ntwo\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("mixed.txt"), "ONE\ntwo\n");
    git(directory.path(), &["add", "--", "mixed.txt"]);
    write(&directory.path().join("mixed.txt"), "ONE\nTWO\n");
    let index_before = git(directory.path(), &["diff", "--cached", "--binary"]);

    let error = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "mixed",
        &["mixed.txt".to_owned()],
    )
    .await
    .expect_err("mixed ownership must conflict");

    assert_eq!(error.code, "git_scope_conflict");
    assert_eq!(
        git(directory.path(), &["diff", "--cached", "--binary"]),
        index_before
    );
}

#[tokio::test]
async fn literal_unicode_metacharacter_path_and_delete_are_committed() {
    let directory = repository();
    let unusual = "dữ liệu[1].txt";
    let deleted = "deleted.txt";
    write(&directory.path().join(unusual), "base\n");
    write(&directory.path().join(deleted), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join(unusual), "updated\n");
    std::fs::remove_file(directory.path().join(deleted)).expect("delete tracked fixture");

    let output = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "literal paths",
        &[unusual.to_owned(), deleted.to_owned()],
    )
    .await
    .expect("literal scoped commit");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    assert_eq!(
        git(directory.path(), &["show", &format!("HEAD:{unusual}")]),
        "updated\n"
    );
    let names = git(
        directory.path(),
        &[
            "-c",
            "core.quotePath=false",
            "diff-tree",
            "--root",
            "--name-only",
            "-r",
            "HEAD",
        ],
    );
    assert!(names.lines().any(|name| name == unusual), "names={names:?}");
    assert!(names.lines().any(|name| name == deleted), "names={names:?}");
}

#[tokio::test]
async fn preview_detects_head_or_index_change_before_execution() {
    let directory = repository();
    write(&directory.path().join("selected.txt"), "base\n");
    write(&directory.path().join("other.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("selected.txt"), "selected\n");
    let service = service(directory.path());
    let paths = vec!["selected.txt".to_owned()];
    let preview = service
        .preview_commit_with_options(
            directory.path(),
            false,
            &paths,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("commit preview");
    write(&directory.path().join("other.txt"), "indexed externally\n");
    git(directory.path(), &["add", "--", "other.txt"]);

    let error = service
        .commit_previewed_with_options(
            directory.path(),
            "stale preview",
            false,
            &paths,
            &preview,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("stale preview must fail");

    assert_eq!(error.code, "git_scope_changed");
}

#[tokio::test]
async fn detached_head_supports_an_explicit_scoped_commit() {
    let directory = repository();
    write(&directory.path().join("selected.txt"), "base\n");
    commit_all(directory.path(), "base");
    git(directory.path(), &["checkout", "--quiet", "--detach"]);
    write(&directory.path().join("selected.txt"), "detached\n");

    let output = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "detached scoped commit",
        &["selected.txt".to_owned()],
    )
    .await
    .expect("detached commit");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    assert_eq!(
        git(directory.path(), &["show", "HEAD:selected.txt"]),
        "detached\n"
    );
}

#[tokio::test]
async fn staged_rename_commits_only_when_both_paths_are_explicit() {
    let directory = repository();
    write(&directory.path().join("old.txt"), "content\n");
    commit_all(directory.path(), "base");
    git(directory.path(), &["mv", "--", "old.txt", "renamed[1].txt"]);

    let output = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "rename",
        &["old.txt".to_owned(), "renamed[1].txt".to_owned()],
    )
    .await
    .expect("scoped rename");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    assert_eq!(
        git(directory.path(), &["show", "HEAD:renamed[1].txt"]),
        "content\n"
    );
    assert!(git(directory.path(), &["status", "--porcelain=v1"]).is_empty());
}

#[tokio::test]
async fn empty_scoped_commit_reports_terminal_failure_without_changing_head() {
    let directory = repository();
    write(&directory.path().join("tracked.txt"), "content\n");
    commit_all(directory.path(), "base");
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);

    let output = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "empty",
        &["tracked.txt".to_owned()],
    )
    .await
    .expect("git reports empty commit as process outcome");

    assert_ne!(output.exit_code, Some(0));
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
    match output.structured {
        Some(GitStructuredOutput::Commit(data)) => {
            assert_eq!(data.phase, "commitHooksIncluded");
            assert!(data.hooks_included);
            assert!(data.commit_hash.is_none());
        }
        other => panic!("expected commit outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn unresolved_merge_is_rejected_during_preview() {
    let directory = repository();
    write(&directory.path().join("conflict.txt"), "base\n");
    commit_all(directory.path(), "base");
    git(directory.path(), &["checkout", "--quiet", "-b", "side"]);
    write(&directory.path().join("conflict.txt"), "side\n");
    commit_all(directory.path(), "side");
    git(directory.path(), &["checkout", "--quiet", "master"]);
    write(&directory.path().join("conflict.txt"), "main\n");
    commit_all(directory.path(), "main");
    let merge = Command::new("git")
        .args(["merge", "--no-edit", "side"])
        .current_dir(directory.path())
        .output()
        .expect("run conflicting merge");
    assert!(!merge.status.success(), "merge fixture must conflict");
    let index_before = git(directory.path(), &["ls-files", "--stage"]);

    let error = scoped_commit(
        &service(directory.path()),
        directory.path(),
        "must not commit conflict",
        &["conflict.txt".to_owned()],
    )
    .await
    .expect_err("unresolved merge must fail during preview");

    assert_eq!(error.code, "git_scope_conflict");
    assert_eq!(
        git(directory.path(), &["ls-files", "--stage"]),
        index_before
    );
}

#[tokio::test]
async fn preview_binds_selected_worktree_bytes_not_only_the_path() {
    let directory = repository();
    write(&directory.path().join("selected.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("selected.txt"), "first\n");
    let service = service(directory.path());
    let paths = vec!["selected.txt".to_owned()];
    let preview = service
        .preview_commit_with_options(
            directory.path(),
            false,
            &paths,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("preview selected bytes");
    write(&directory.path().join("selected.txt"), "second\n");

    let error = service
        .commit_previewed_with_options(
            directory.path(),
            "stale bytes",
            false,
            &paths,
            &preview,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("same path with different bytes must invalidate preview");

    assert_eq!(error.code, "git_scope_changed");
    assert_eq!(
        git(directory.path(), &["show", "HEAD:selected.txt"]),
        "base\n"
    );
}

#[tokio::test]
async fn all_rejects_unstaged_or_untracked_changes_without_mutating_index() {
    let directory = repository();
    write(&directory.path().join("tracked.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("tracked.txt"), "changed\n");
    write(&directory.path().join("untracked.txt"), "new\n");
    let index_before = git(directory.path(), &["diff", "--cached", "--binary"]);
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);

    let error = service(directory.path())
        .preview_commit_with_options(
            directory.path(),
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("all must not implicitly stage worktree changes");

    assert_eq!(error.code, "git_scope_conflict");
    assert_eq!(
        git(directory.path(), &["diff", "--cached", "--binary"]),
        index_before
    );
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
}

#[tokio::test]
async fn ambiguous_path_spellings_are_rejected_before_commit() {
    let directory = repository();
    write(&directory.path().join("tracked.txt"), "base\n");
    commit_all(directory.path(), "base");
    write(&directory.path().join("tracked.txt"), "changed\n");
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);

    for path in [".", "./tracked.txt", "dir//file.txt", "dir/./file.txt"] {
        let error = scoped_commit(
            &service(directory.path()),
            directory.path(),
            "ambiguous",
            &[path.to_owned()],
        )
        .await
        .expect_err("ambiguous path must fail before commit");
        assert_eq!(error.code, "invalid_commit_path", "path={path:?}");
    }
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
}
