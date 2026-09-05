use super::*;
use std::{fs, time::Duration};
use tempfile::TempDir;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, content).expect("write fixture");
}

#[cfg(unix)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[test]
fn rule_io_warnings_distinguish_permission_from_other_io_failures() {
    let path = Path::new("AGENTS.md");
    let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let other = std::io::Error::other("broken");
    assert!(rule_io_warning(path, "read", &denied).starts_with("permission_denied:"));
    assert!(rule_io_warning(path, "read", &other).starts_with("io_error:"));
}

#[tokio::test]
async fn applies_root_and_nested_agents_without_sibling_leakage() {
    let fixture = TempDir::new().expect("fixture");
    let root = fixture.path();
    write(&root.join("AGENTS.md"), "root");
    write(&root.join("src/AGENTS.md"), "nested");
    write(&root.join("other/AGENTS.md"), "sibling");
    write(&root.join("src/deep/code.rs"), "fn main() {}");
    write(&root.join("other/code.rs"), "fn sibling() {}");

    let bundle = ProjectContextService::default()
        .load(root, &[PathBuf::from("src/deep/code.rs")])
        .await
        .expect("context");

    let contents = bundle
        .rules
        .iter()
        .map(|rule| rule.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(contents, vec!["root", "nested"]);
    assert!(bundle.rules[0].precedence < bundle.rules[1].precedence);
    assert!(bundle.rules[1].scope_root.ends_with("src"));
}

#[tokio::test]
async fn codex_rules_are_hidden_discovered_in_deterministic_order() {
    let fixture = TempDir::new().expect("fixture");
    write(&fixture.path().join("AGENTS.md"), "root");
    write(&fixture.path().join(".codex/rules/z.md"), "z");
    write(&fixture.path().join(".codex/rules/a.md"), "a");
    write(&fixture.path().join("Cargo.toml"), "[package]");

    let first = ProjectContextService::default()
        .load(fixture.path(), &[])
        .await
        .expect("first");
    let second = ProjectContextService::default()
        .load(fixture.path(), &[])
        .await
        .expect("second");
    let paths = first
        .rules
        .iter()
        .map(|rule| rule.path.as_str())
        .collect::<Vec<_>>();
    assert!(paths[0].ends_with("AGENTS.md"));
    assert!(paths[1].ends_with("a.md"));
    assert!(paths[2].ends_with("z.md"));
    assert_eq!(first.effective_hash, second.effective_hash);
    assert_eq!(first.manifests.len(), 1);
}

#[tokio::test]
async fn utf8_truncation_has_continuation_and_valid_boundary() {
    let fixture = TempDir::new().expect("fixture");
    write(&fixture.path().join("AGENTS.md"), "xin chào thế giới");
    let service = ProjectContextService::with_budgets(32, 8, 256, Duration::from_secs(5));

    let bundle = service.load(fixture.path(), &[]).await.expect("context");
    let rule = &bundle.rules[0];
    assert!(rule.truncated);
    assert!(
        rule.next_range
            .as_deref()
            .is_some_and(|range| range.starts_with("bytes="))
    );
    assert!(std::str::from_utf8(rule.content.as_bytes()).is_ok());
}

#[tokio::test]
async fn invalid_utf8_is_reported_instead_of_treated_as_no_rules() {
    let fixture = TempDir::new().expect("fixture");
    fs::write(fixture.path().join("AGENTS.md"), [0xff, 0xfe]).expect("write invalid rule");

    let bundle = ProjectContextService::default()
        .load(fixture.path(), &[])
        .await
        .expect("context");
    assert!(bundle.rules.is_empty());
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.starts_with("invalid_utf8:"))
    );
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("no applicable"))
    );
}

#[tokio::test]
async fn workspace_and_file_changes_cannot_reuse_another_bundle() {
    let first = TempDir::new().expect("first fixture");
    let second = TempDir::new().expect("second fixture");
    write(&first.path().join("AGENTS.md"), "first");
    write(&second.path().join("AGENTS.md"), "second");
    let service = ProjectContextService::default();
    let before = service.load(first.path(), &[]).await.expect("first before");
    let other = service.load(second.path(), &[]).await.expect("second");
    write(&first.path().join("AGENTS.md"), "first changed");
    let after = service.load(first.path(), &[]).await.expect("first after");

    assert_ne!(before.effective_hash, other.effective_hash);
    assert_ne!(before.effective_hash, after.effective_hash);
    assert_eq!(after.rules[0].content, "first changed");
}

#[tokio::test]
async fn file_count_and_total_budgets_report_partial_context() {
    let fixture = TempDir::new().expect("fixture");
    for index in 0..4 {
        write(
            &fixture.path().join(format!(".codex/rules/{index}.md")),
            "12345678",
        );
    }
    let service = ProjectContextService::with_budgets(2, 64, 10, Duration::from_secs(5));

    let bundle = service.load(fixture.path(), &[]).await.expect("context");
    assert_eq!(bundle.rules.len(), 2);
    assert!(bundle.truncated);
    assert!(bundle.rules[1].truncated);
    assert!(bundle.rules[1].next_range.is_some());
}

#[tokio::test]
async fn manifest_content_change_invalidates_context_hash() {
    let fixture = TempDir::new().expect("fixture");
    write(&fixture.path().join("Cargo.toml"), "version = '1'");
    let service = ProjectContextService::default();
    let before = service.load(fixture.path(), &[]).await.expect("before");
    write(&fixture.path().join("Cargo.toml"), "version = '2'");
    let after = service.load(fixture.path(), &[]).await.expect("after");

    assert_ne!(before.effective_hash, after.effective_hash);
}

#[tokio::test]
async fn target_outside_workspace_fails_closed() {
    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    let error = ProjectContextService::default()
        .load(root.path(), &[outside.path().to_path_buf()])
        .await
        .expect_err("outside target must fail");
    assert_eq!(error.code, "project_context_target_outside_workspace");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_rule_is_skipped_without_following_it() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    write(&outside.path().join("secret.md"), "outside");
    fs::create_dir_all(root.path().join(".codex/rules")).expect("rules directory");
    symlink(
        outside.path().join("secret.md"),
        root.path().join(".codex/rules/linked.md"),
    )
    .expect("symlink fixture");

    let bundle = ProjectContextService::default()
        .load(root.path(), &[])
        .await
        .expect("context");
    assert!(bundle.rules.is_empty());
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("cannot be symlinks"))
    );
}

#[tokio::test]
async fn continuation_reads_a_bounded_chunk_and_rejects_stale_versions() {
    let fixture = TempDir::new().expect("fixture");
    write(&fixture.path().join("AGENTS.md"), "0123456789abcdef");
    let service = ProjectContextService::with_budgets(32, 5, 256, Duration::from_secs(5));
    let first = service
        .load(fixture.path(), &[])
        .await
        .expect("first chunk");
    let first_rule = &first.rules[0];
    let range = ProjectContextRange {
        path: first_rule.path.clone(),
        offset: 5,
        version_token: first_rule.version_token.clone(),
    };
    let next = service
        .load_with_options(
            fixture.path(),
            &[],
            ProjectContextPolicy::default(),
            Some(range.clone()),
        )
        .await
        .expect("next chunk");
    assert_eq!(next.rules[0].content, "56789");
    assert!(
        next.rules[0]
            .next_range
            .as_deref()
            .is_some_and(|value| value == "bytes=10..16")
    );

    write(&fixture.path().join("AGENTS.md"), "changed");
    let error = service
        .load_with_options(
            fixture.path(),
            &[],
            ProjectContextPolicy::default(),
            Some(range),
        )
        .await
        .expect_err("stale continuation must fail");
    assert_eq!(error.code, "project_context_version_conflict");
}

#[tokio::test]
async fn claude_rules_require_explicit_policy_and_keep_separate_provenance() {
    let fixture = TempDir::new().expect("fixture");
    write(&fixture.path().join("AGENTS.md"), "agents");
    write(&fixture.path().join("CLAUDE.md"), "claude");
    let service = ProjectContextService::default();
    let default_bundle = service.load(fixture.path(), &[]).await.expect("default");
    assert_eq!(default_bundle.rules.len(), 1);
    assert_eq!(default_bundle.rules[0].kind, ProjectRuleKind::Agents);

    let opted_in = service
        .load_with_options(
            fixture.path(),
            &[],
            ProjectContextPolicy {
                load_claude_md: true,
            },
            None,
        )
        .await
        .expect("opted in");
    assert_eq!(opted_in.rules.len(), 2);
    assert!(
        opted_in
            .rules
            .iter()
            .any(|rule| rule.kind == ProjectRuleKind::Claude)
    );
    assert_ne!(opted_in.rules[0].path, opted_in.rules[1].path);
}

#[tokio::test]
async fn manifest_discovery_is_inert_and_rule_io_errors_remain_distinct() {
    let fixture = TempDir::new().expect("fixture");
    let marker = fixture.path().join("must-not-exist");
    write(
        &fixture.path().join("package.json"),
        &format!("touch {}", marker.display()),
    );
    fs::create_dir(fixture.path().join("AGENTS.md")).expect("non-file rule");
    let bundle = ProjectContextService::default()
        .load(fixture.path(), &[])
        .await
        .expect("manifest metadata");
    assert_eq!(bundle.manifests.len(), 1);
    assert!(!marker.exists(), "manifest content must never be executed");
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("not a regular file"))
    );

    let invalid = TempDir::new().expect("invalid rules fixture");
    write(&invalid.path().join(".codex/rules"), "not a directory");
    let error = ProjectContextService::default()
        .load(invalid.path(), &[])
        .await
        .expect_err("rules directory I/O failure must be structured");
    assert_eq!(error.code, "project_context_rules_unavailable");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn root_manifest_symlink_is_skipped_without_reading_or_hashing_external_content() {
    let fixture = TempDir::new().expect("fixture");
    let outside = TempDir::new().expect("outside fixture");
    let marker = outside.path().join("Cargo.toml");
    write(&marker, "external-secret-one");
    link_file(&marker, &fixture.path().join("Cargo.toml")).expect("link root manifest");
    let service = ProjectContextService::default();

    let first = service.load(fixture.path(), &[]).await.expect("first scan");
    assert!(first.manifests.is_empty());
    assert!(
        first
            .warnings
            .iter()
            .any(|warning| warning.contains("project manifests cannot be symlinks"))
    );

    write(&marker, "external-secret-two-with-different-length");
    let second = service
        .load(fixture.path(), &[])
        .await
        .expect("second scan");
    assert_eq!(first.effective_hash, second.effective_hash);
    assert!(second.manifests.is_empty());
}
