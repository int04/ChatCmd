use super::{
    GitCommitPreview, hex_digest, in_scope, nul_paths, parse_hash, scope_changed, succeeded,
    unmerged_paths,
};
use crate::{GitOutputMode, GitRunOptions, RuntimeError, RuntimeResult};
use sha2::Digest as _;
use std::{
    collections::BTreeSet,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

const MAX_WORKTREE_HASH_BYTES: u64 = 1024 * 1024 * 1024;

pub(super) async fn inspect(
    service: &super::GitService,
    cwd: &Path,
    all: bool,
    scope_paths: Vec<String>,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<GitCommitPreview> {
    let head = optional_head(service, cwd, options, cancellation.clone()).await?;
    let index = inspect_output(
        service,
        cwd,
        &["ls-files", "--stage", "-z"],
        options,
        cancellation.clone(),
    )
    .await?;
    let staged = inspect_output(
        service,
        cwd,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACDMRTUXB",
            "-z",
        ],
        options,
        cancellation.clone(),
    )
    .await?;
    let unstaged = inspect_output(
        service,
        cwd,
        &["diff", "--name-only", "--diff-filter=ACDMRTUXB", "-z"],
        options,
        cancellation.clone(),
    )
    .await?;
    let untracked = inspect_output(
        service,
        cwd,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        options,
        cancellation.clone(),
    )
    .await?;
    let staged_paths = nul_paths(&staged.stdout);
    let unstaged_paths = nul_paths(&unstaged.stdout);
    let untracked_paths = nul_paths(&untracked.stdout);
    let worktree_digest = worktree_digest(
        cwd,
        all,
        &scope_paths,
        &unstaged_paths,
        &untracked_paths,
        cancellation,
    )
    .await?;
    Ok(GitCommitPreview {
        version: super::PREVIEW_VERSION,
        head,
        index_digest: hex_digest(index.stdout.as_bytes()),
        worktree_digest,
        staged_paths,
        unstaged_paths,
        untracked_paths,
        unmerged_paths: unmerged_paths(&index.stdout),
        scope_paths,
        all,
    })
}

pub(super) async fn inspect_output(
    service: &super::GitService,
    cwd: &Path,
    args: &[&str],
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<crate::CommandOutput> {
    let output = service
        .run(cwd, args, &inspection_options(options), cancellation)
        .await?;
    if !succeeded(&output) || output.truncated {
        return Err(inspection_error(if output.stderr.trim().is_empty() {
            "git scope inspection did not complete"
        } else {
            output.stderr.trim()
        }));
    }
    Ok(output)
}

pub(super) async fn optional_head(
    service: &super::GitService,
    cwd: &Path,
    options: &GitRunOptions,
    cancellation: CancellationToken,
) -> RuntimeResult<Option<String>> {
    let output = service
        .run(
            cwd,
            &["rev-parse", "--verify", "HEAD"],
            &inspection_options(options),
            cancellation,
        )
        .await?;
    if output.cancelled || output.timed_out {
        return Err(scope_changed("HEAD inspection was interrupted"));
    }
    if output.exit_code != Some(0) {
        return Ok(None);
    }
    parse_hash(output.stdout.trim())
        .map(Some)
        .ok_or_else(|| scope_changed("HEAD returned an invalid object id"))
}

pub(super) fn inspection_options(options: &GitRunOptions) -> GitRunOptions {
    let mut bounded = options.clone();
    bounded.output_mode = GitOutputMode::Inline;
    bounded.max_output_bytes = bounded.max_output_bytes.max(1024 * 1024);
    bounded.max_stderr_bytes = bounded.max_stderr_bytes.max(64 * 1024);
    bounded.cursor = None;
    bounded
}

async fn worktree_digest(
    cwd: &Path,
    all: bool,
    scope_paths: &[String],
    unstaged_paths: &[String],
    untracked_paths: &[String],
    cancellation: CancellationToken,
) -> RuntimeResult<String> {
    let selected = unstaged_paths
        .iter()
        .chain(untracked_paths)
        .filter(|path| all || scope_paths.iter().any(|scope| in_scope(path, scope)))
        .collect::<BTreeSet<_>>();
    let mut hasher = sha2::Sha256::new();
    let mut total = 0_u64;
    for path in selected {
        if cancellation.is_cancelled() {
            return Err(scope_changed("worktree inspection was cancelled"));
        }
        hasher.update(path.as_bytes());
        hasher.update([0]);
        let Some(validated) = validated_worktree_path(cwd, path).await? else {
            hasher.update(b"deleted");
            hasher.update([0]);
            continue;
        };
        hash_path(&validated, &mut hasher, &mut total, cancellation.clone()).await?;
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn validated_worktree_path(cwd: &Path, relative: &str) -> RuntimeResult<Option<PathBuf>> {
    let root = tokio::fs::canonicalize(cwd)
        .await
        .map_err(|error| inspection_error(&error.to_string()))?;
    let mut current = root.clone();
    let components = Path::new(relative).components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(inspection_error(
            "git returned a non-relative worktree path",
        ));
    }
    let Some((leaf, parents)) = components.split_last() else {
        return Err(inspection_error("git returned an empty worktree path"));
    };
    for component in parents {
        current.push(component.as_os_str());
        let metadata = match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(inspection_error(&error.to_string())),
        };
        if is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Err(inspection_error(
                "worktree path has a symbolic-link or non-directory parent",
            ));
        }
        if !current.starts_with(&root) {
            return Err(inspection_error("worktree path escapes the repository"));
        }
    }
    current.push(leaf.as_os_str());
    Ok(Some(current))
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

async fn hash_path(
    path: &Path,
    hasher: &mut sha2::Sha256,
    total: &mut u64,
    cancellation: CancellationToken,
) -> RuntimeResult<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            hasher.update(b"deleted");
            return Ok(());
        }
        Err(error) => return Err(inspection_error(&error.to_string())),
    };
    if is_link_or_reparse(&metadata) {
        let target = tokio::fs::read_link(path)
            .await
            .map_err(|error| inspection_error(&error.to_string()))?;
        hasher.update(b"symlink\0");
        hasher.update(target.to_string_lossy().as_bytes());
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(inspection_error(
            "worktree path is not a regular file or symbolic link",
        ));
    }
    hasher.update(b"file\0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        hasher.update(metadata.permissions().mode().to_le_bytes());
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| inspection_error(&error.to_string()))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            read = file.read(&mut buffer) => read.map_err(|error| inspection_error(&error.to_string()))?,
            () = cancellation.cancelled() => return Err(scope_changed("worktree inspection was cancelled")),
        };
        if read == 0 {
            break;
        }
        *total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if *total > MAX_WORKTREE_HASH_BYTES {
            return Err(inspection_error(
                "worktree content exceeds the commit preview hash budget",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}

fn inspection_error(message: &str) -> RuntimeError {
    let mut error = RuntimeError::new("git_scope_inspection_failed", message);
    error.phase = Some("preview".to_owned());
    error
}
