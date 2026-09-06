//! Deterministic, server-owned snapshots for command verification freshness.

use crate::CommandSourceState;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

const MAX_FILES: u64 = 50_000;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const BUFFER_BYTES: usize = 64 * 1024;
const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".next",
    ".cache",
    ".test-command-artifacts",
    "command-artifacts-v1",
    "dist",
    "build",
    "out",
];

pub(super) async fn capture_source_state(root: PathBuf) -> CommandSourceState {
    let fallback_root = root.clone();
    tokio::task::spawn_blocking(move || capture(&root))
        .await
        .unwrap_or_else(|_| incomplete_state(&fallback_root, "snapshot worker did not complete"))
}

fn capture(root: &Path) -> CommandSourceState {
    let mut paths = Vec::new();
    let filter_root = root.to_owned();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(move |entry| !is_excluded_directory(entry.path(), &filter_root))
        .build();
    let mut limitation = None;
    for entry in walker {
        match entry {
            Ok(entry) if entry.path() != root => paths.push(entry.path().to_owned()),
            Ok(_) => {}
            Err(_) => limitation = Some("one or more source paths could not be enumerated"),
        }
        if paths.len() > usize::try_from(MAX_FILES).unwrap_or(usize::MAX) {
            limitation = Some("source snapshot exceeded the file-count budget");
            break;
        }
    }
    paths.sort_by(|left, right| {
        normalized_relative(root, left).cmp(&normalized_relative(root, right))
    });

    let mut hasher = Sha256::new();
    hasher.update(b"chatcmd-source-state-v1\0");
    let mut files_scanned = 0_u64;
    let mut bytes_hashed = 0_u64;
    for path in paths {
        if limitation.is_some() {
            break;
        }
        let relative = normalized_relative(root, &path);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(_) => {
                limitation = Some("source metadata changed or became unreadable during snapshot");
                break;
            }
        };
        if metadata.is_dir() {
            continue;
        }
        files_scanned = files_scanned.saturating_add(1);
        if files_scanned > MAX_FILES {
            limitation = Some("source snapshot exceeded the file-count budget");
            break;
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            match std::fs::read_link(&path) {
                Ok(target) => {
                    hasher.update(target.to_string_lossy().as_bytes());
                    limitation = Some(
                        "source scope contains a symlink whose target content is outside the snapshot",
                    );
                }
                Err(_) => limitation = Some("a source symlink could not be read"),
            }
            continue;
        }
        if !metadata.is_file() {
            hasher.update(b"special\0");
            limitation = Some("source scope contains an unsupported special file");
            break;
        }
        if bytes_hashed.saturating_add(metadata.len()) > MAX_BYTES {
            limitation = Some("source snapshot exceeded the byte budget");
            break;
        }
        hasher.update(b"file\0");
        if let Err(reason) = hash_file(&path, &metadata, &mut hasher, &mut bytes_hashed) {
            limitation = Some(reason);
        }
    }
    CommandSourceState {
        schema_version: 1,
        algorithm: "sha256".to_owned(),
        digest: format!("sha256:{:x}", hasher.finalize()),
        scope: "cwdSourceInputsV1".to_owned(),
        files_scanned,
        bytes_hashed,
        complete: limitation.is_none(),
        limitation: limitation.map(str::to_owned),
        excluded_directories: EXCLUDED_DIRECTORIES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

fn hash_file(
    path: &Path,
    initial: &std::fs::Metadata,
    hasher: &mut Sha256,
    bytes_hashed: &mut u64,
) -> Result<(), &'static str> {
    let file = File::open(path).map_err(|_| "a source file could not be opened")?;
    let mut reader = BufReader::new(file);
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| "a source file could not be read")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        *bytes_hashed = bytes_hashed.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    let final_metadata = std::fs::metadata(path)
        .map_err(|_| "a source file changed or disappeared during snapshot")?;
    if initial.len() != final_metadata.len()
        || initial.modified().ok() != final_metadata.modified().ok()
    {
        return Err("a source file changed while its snapshot was being captured");
    }
    Ok(())
}

fn is_excluded_directory(path: &Path, root: &Path) -> bool {
    path != root
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| EXCLUDED_DIRECTORIES.contains(&value))
        && path.is_dir()
}

fn normalized_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn incomplete_state(root: &Path, limitation: &str) -> CommandSourceState {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    CommandSourceState {
        schema_version: 1,
        algorithm: "sha256".to_owned(),
        digest: format!("sha256:{:x}", hasher.finalize()),
        scope: "cwdSourceInputsV1".to_owned(),
        files_scanned: 0,
        bytes_hashed: 0,
        complete: false,
        limitation: Some(limitation.to_owned()),
        excluded_directories: EXCLUDED_DIRECTORIES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_tracks_untracked_content_and_rules_but_excludes_build_outputs() {
        let directory = TempDir::new().expect("temporary directory");
        std::fs::create_dir(directory.path().join(".git")).expect("git metadata directory");
        std::fs::write(directory.path().join(".git/HEAD"), "ref: refs/heads/main")
            .expect("stable head");
        std::fs::write(directory.path().join("tracked.rs"), "tracked before")
            .expect("tracked-like source");
        std::fs::write(directory.path().join("untracked.rs"), "one").expect("source");
        std::fs::create_dir(directory.path().join(".codex")).expect("rules directory");
        std::fs::write(directory.path().join(".codex/rules.md"), "rule one").expect("rules");
        std::fs::create_dir(directory.path().join("target")).expect("target directory");
        std::fs::write(directory.path().join("target/output"), "generated one").expect("output");
        let first = capture(directory.path());
        assert!(first.complete, "{:?}", first.limitation);

        std::fs::write(directory.path().join("tracked.rs"), "tracked dirty")
            .expect("dirty tracked-like source without moving HEAD");
        let dirty_changed = capture(directory.path());
        assert_ne!(first.digest, dirty_changed.digest);
        assert_eq!(
            std::fs::read_to_string(directory.path().join(".git/HEAD")).expect("head"),
            "ref: refs/heads/main"
        );
        std::fs::write(directory.path().join("untracked.rs"), "two").expect("edit source");
        let source_changed = capture(directory.path());
        assert_ne!(dirty_changed.digest, source_changed.digest);
        std::fs::write(directory.path().join(".codex/rules.md"), "rule two").expect("edit rules");
        let rules_changed = capture(directory.path());
        assert_ne!(source_changed.digest, rules_changed.digest);
        std::fs::write(directory.path().join("target/output"), "generated two").expect("output");
        assert_eq!(rules_changed.digest, capture(directory.path()).digest);
    }
}
