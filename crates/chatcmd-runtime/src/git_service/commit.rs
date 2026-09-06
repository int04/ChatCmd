use super::GitService;
use crate::{
    CommandOutput, GitCommitData, GitRunOptions, GitStructuredOutput, RuntimeError, RuntimeResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeSet, path::Path};
use tokio_util::sync::CancellationToken;

mod inspection;
use inspection::{inspect, inspect_output, inspection_options, optional_head};

const PREVIEW_VERSION: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitPreview {
    pub version: u8,
    pub head: Option<String>,
    pub index_digest: String,
    #[serde(default)]
    pub worktree_digest: String,
    pub staged_paths: Vec<String>,
    pub unstaged_paths: Vec<String>,
    pub untracked_paths: Vec<String>,
    pub unmerged_paths: Vec<String>,
    pub scope_paths: Vec<String>,
    pub all: bool,
}

pub(super) async fn preview(
    service: &GitService,
    cwd: &Path,
    all: bool,
    paths: &[String],
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<GitCommitPreview> {
    let scope_paths = validate_scope(all, paths)?;
    let first = inspect(
        service,
        cwd,
        all,
        scope_paths.clone(),
        options,
        cancellation.clone(),
    )
    .await?;
    let second = inspect(service, cwd, all, scope_paths, options, cancellation).await?;
    if first != second {
        return Err(scope_changed(
            "repository changed while commit preview was created",
        ));
    }
    validate_preview(&first)?;
    Ok(first)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    service: &GitService,
    cwd: &Path,
    message: &str,
    all: bool,
    paths: &[String],
    preview: &GitCommitPreview,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<CommandOutput> {
    if message.trim().is_empty() {
        return Err(RuntimeError::new(
            "invalid_commit_message",
            "commit message cannot be empty",
        ));
    }
    let scope_paths = validate_scope(all, paths)?;
    if preview.version != PREVIEW_VERSION
        || preview.all != all
        || preview.scope_paths != scope_paths
    {
        return Err(scope_changed("commit scope does not match its preview"));
    }
    let current = inspect(
        service,
        cwd,
        all,
        scope_paths,
        options,
        cancellation.clone(),
    )
    .await?;
    if &current != preview {
        return Err(scope_changed(
            "HEAD, index, or worktree changed after commit preview",
        ));
    }
    validate_preview(&current)?;

    let mut args = vec![
        "commit".to_owned(),
        "--message".to_owned(),
        message.to_owned(),
    ];
    if !all {
        args.push("--only".to_owned());
        args.push("--".to_owned());
        args.extend(current.scope_paths.iter().cloned());
    }
    let mut committed = service
        .run_owned(cwd, &args, options, cancellation.clone())
        .await?;
    if !succeeded(&committed) {
        let observed_head = commit_hash(service, cwd, options, CancellationToken::new()).await?;
        let changed = observed_head != current.head;
        set_commit_phase(
            &mut committed,
            if changed {
                "commitCompletedHookInterrupted"
            } else {
                "commitHooksIncluded"
            },
            true,
            changed.then_some(observed_head).flatten(),
        );
        return Ok(committed);
    }

    let commit_hash = commit_hash(service, cwd, options, cancellation.clone()).await?;
    let Some(commit_hash) = commit_hash else {
        return Err(post_commit_error(
            "commit completed but HEAD could not be verified",
        ));
    };
    verify_parent(
        service,
        cwd,
        current.head.as_deref(),
        options,
        cancellation.clone(),
    )
    .await?;
    verify_committed_paths(service, cwd, &current, options, cancellation).await?;
    set_commit_phase(
        &mut committed,
        "commitHooksIncluded",
        true,
        Some(commit_hash),
    );
    Ok(committed)
}

fn validate_preview(preview: &GitCommitPreview) -> RuntimeResult<()> {
    if !preview.unmerged_paths.is_empty() {
        return Err(scope_conflict(format!(
            "repository has unresolved merge paths: {:?}",
            preview.unmerged_paths
        )));
    }
    if preview.all {
        if !preview.unstaged_paths.is_empty() || !preview.untracked_paths.is_empty() {
            return Err(scope_conflict(
                "all=true requires every intended change to be staged before preview; automatic staging is disabled to preserve the index on commit failure",
            ));
        }
        return Ok(());
    }
    let selected = |path: &str| {
        preview
            .scope_paths
            .iter()
            .any(|scope| in_scope(path, scope))
    };
    let staged_outside = preview.staged_paths.iter().find(|path| !selected(path));
    if let Some(path) = staged_outside {
        return Err(scope_conflict(format!(
            "staged path outside commit scope: {path:?}"
        )));
    }
    if let Some(path) = preview
        .staged_paths
        .iter()
        .find(|path| selected(path) && preview.unstaged_paths.contains(path))
    {
        return Err(scope_conflict(format!(
            "selected path has both staged and unstaged changes: {path:?}"
        )));
    }
    if let Some(path) = preview.untracked_paths.iter().find(|path| selected(path)) {
        return Err(scope_conflict(format!(
            "selected untracked path is not supported safely: {path:?}"
        )));
    }
    Ok(())
}

fn validate_scope(all: bool, paths: &[String]) -> RuntimeResult<Vec<String>> {
    if all && !paths.is_empty() {
        return Err(RuntimeError::new(
            "invalid_commit_scope",
            "all and paths are mutually exclusive",
        ));
    }
    if !all && paths.is_empty() {
        return Err(RuntimeError::new(
            "commit_scope_required",
            "commit requires explicit paths or all=true",
        ));
    }
    let mut scope = BTreeSet::new();
    for path in paths {
        let normalized = path.replace('\\', "/");
        let components = normalized.split('/').collect::<Vec<_>>();
        if normalized.is_empty()
            || normalized.starts_with('/')
            || normalized.contains('\0')
            || components
                .iter()
                .any(|part| part.is_empty() || matches!(*part, "." | ".."))
            || normalized.get(1..2) == Some(":")
        {
            return Err(RuntimeError::new(
                "invalid_commit_path",
                "commit paths must be normalized relative paths without dot components, repeated separators, drive prefixes, or parent traversal",
            ));
        }
        scope.insert(normalized);
    }
    Ok(scope.into_iter().collect())
}

async fn commit_hash(
    service: &GitService,
    cwd: &Path,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<Option<String>> {
    optional_head(service, cwd, options, cancellation).await
}

async fn verify_parent(
    service: &GitService,
    cwd: &Path,
    expected: Option<&str>,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<()> {
    let output = service
        .run(
            cwd,
            &["rev-parse", "--verify", "HEAD^"],
            &inspection_options(options),
            cancellation,
        )
        .await?;
    let actual = if output.exit_code == Some(0) && !output.cancelled && !output.timed_out {
        parse_hash(output.stdout.trim())
    } else {
        None
    };
    if actual.as_deref() != expected {
        return Err(post_commit_error(
            "commit parent differs from the previewed HEAD; repository changed concurrently",
        ));
    }
    Ok(())
}

async fn verify_committed_paths(
    service: &GitService,
    cwd: &Path,
    preview: &GitCommitPreview,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<()> {
    let output = inspect_output(
        service,
        cwd,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "HEAD",
        ],
        options,
        cancellation,
    )
    .await?;
    if !preview.all
        && nul_paths(&output.stdout).iter().any(|path| {
            !preview
                .scope_paths
                .iter()
                .any(|scope| in_scope(path, scope))
        })
    {
        return Err(post_commit_error(
            "commit contains a path outside its previewed scope",
        ));
    }
    Ok(())
}

fn in_scope(path: &str, scope: &str) -> bool {
    path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn nul_paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn unmerged_paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter_map(|entry| {
            let (metadata, path) = entry.split_once('\t')?;
            let stage = metadata.rsplit_once(' ')?.1;
            (stage != "0").then(|| path.to_owned())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn parse_hash(value: &str) -> Option<String> {
    (matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_owned())
}

fn succeeded(output: &CommandOutput) -> bool {
    output.exit_code == Some(0) && !output.timed_out && !output.cancelled
}

fn set_commit_phase(
    output: &mut CommandOutput,
    phase: &str,
    hooks_included: bool,
    commit_hash: Option<String>,
) {
    output.structured = Some(GitStructuredOutput::Commit(GitCommitData {
        phase: phase.to_owned(),
        commit_hash,
        hooks_included,
    }));
}

fn scope_conflict(message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new("git_scope_conflict", message);
    error.phase = Some("preview".to_owned());
    error
}

fn scope_changed(message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new("git_scope_changed", message);
    error.retryable = true;
    error.phase = Some("preCommitRecheck".to_owned());
    error
}

fn post_commit_error(message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new("git_commit_verification_failed", message);
    error.phase = Some("postCommitVerification".to_owned());
    error
}
