use crate::{RuntimeError, RuntimeResult};
mod manifest;
mod range;
use manifest::{bundle_hash, discover_manifests};
use range::{display, read_rule_range, rule_io_warning};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const DEFAULT_MAX_FILES: usize = 32;
const DEFAULT_MAX_FILE_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: usize = 256 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuleRecord {
    pub path: String,
    pub scope_root: String,
    pub kind: ProjectRuleKind,
    pub version_token: String,
    pub content_hash: String,
    pub precedence: usize,
    pub content: String,
    pub truncated: bool,
    pub next_range: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ProjectRuleKind {
    Agents,
    Claude,
    CodexRule,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectContextPolicy {
    /// Opt in to separate, scoped CLAUDE.md provenance records; never silently merged.
    #[serde(default)]
    pub load_claude_md: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectContextRange {
    pub path: String,
    pub offset: usize,
    pub version_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextBundle {
    pub context_ref: String,
    pub workspace: String,
    pub effective_hash: String,
    pub rules: Vec<ProjectRuleRecord>,
    pub manifests: Vec<String>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectContextService {
    max_files: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
    timeout: Duration,
}

impl Default for ProjectContextService {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl ProjectContextService {
    #[must_use]
    pub fn with_budgets(
        max_files: usize,
        max_file_bytes: usize,
        max_total_bytes: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            max_files: max_files.clamp(1, DEFAULT_MAX_FILES),
            max_file_bytes: max_file_bytes.clamp(1, DEFAULT_MAX_FILE_BYTES),
            max_total_bytes: max_total_bytes.clamp(1, DEFAULT_MAX_TOTAL_BYTES),
            timeout: timeout.min(DEFAULT_TIMEOUT),
        }
    }

    pub async fn load(
        &self,
        workspace: impl AsRef<Path>,
        targets: &[PathBuf],
    ) -> RuntimeResult<ProjectContextBundle> {
        self.load_with_options(workspace, targets, ProjectContextPolicy::default(), None)
            .await
    }

    pub async fn load_with_options(
        &self,
        workspace: impl AsRef<Path>,
        targets: &[PathBuf],
        policy: ProjectContextPolicy,
        range: Option<ProjectContextRange>,
    ) -> RuntimeResult<ProjectContextBundle> {
        let workspace = workspace.as_ref().to_path_buf();
        let targets = targets.to_vec();
        let limits = self.clone();
        tokio::time::timeout(
            self.timeout,
            tokio::task::spawn_blocking(move || {
                limits.load_sync(&workspace, &targets, &policy, range.as_ref())
            }),
        )
        .await
        .map_err(|_| {
            RuntimeError::new("project_context_timeout", "project context scan timed out")
        })?
        .map_err(|_| {
            RuntimeError::new(
                "project_context_worker_failed",
                "project context scan worker failed",
            )
        })?
    }

    fn load_sync(
        &self,
        workspace: &Path,
        targets: &[PathBuf],
        policy: &ProjectContextPolicy,
        range: Option<&ProjectContextRange>,
    ) -> RuntimeResult<ProjectContextBundle> {
        let root = workspace.canonicalize().map_err(|error| {
            RuntimeError::new(
                "project_context_workspace_invalid",
                format!("project workspace is unavailable: {error}"),
            )
        })?;
        if !root.is_dir() {
            return Err(RuntimeError::new(
                "project_context_workspace_invalid",
                "project workspace must be a directory",
            ));
        }
        let target_dirs = canonical_target_dirs(&root, targets)?;
        let mut agent_candidates = BTreeSet::new();
        let mut warnings = Vec::new();
        for directory in target_dirs {
            for ancestor in scoped_ancestors(&root, &directory) {
                let path = ancestor.join("AGENTS.md");
                match fs::symlink_metadata(&path) {
                    Ok(_) => {
                        agent_candidates.insert((path, ancestor.clone(), ProjectRuleKind::Agents));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        warnings.push(format!("{} metadata unavailable: {error}", display(&path)))
                    }
                }
                if policy.load_claude_md {
                    let path = ancestor.join("CLAUDE.md");
                    match fs::symlink_metadata(&path) {
                        Ok(_) => {
                            agent_candidates.insert((path, ancestor, ProjectRuleKind::Claude));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => warnings
                            .push(format!("{} metadata unavailable: {error}", display(&path))),
                    }
                }
            }
        }
        let mut codex_candidates = BTreeSet::new();
        discover_codex_rules(&root, &mut codex_candidates, &mut warnings)?;
        let candidates = agent_candidates
            .into_iter()
            .chain(codex_candidates)
            .collect::<Vec<_>>();

        if let Some(range) = range {
            return self.load_range(&root, candidates, warnings, range);
        }

        let mut remaining = self.max_total_bytes;
        let mut rules = Vec::new();
        let mut truncated = candidates.len() > self.max_files;
        for (precedence, (path, scope, kind)) in
            candidates.into_iter().take(self.max_files).enumerate()
        {
            match read_rule(
                &root,
                &path,
                &scope,
                kind,
                precedence,
                self.max_file_bytes,
                remaining,
            ) {
                Ok(Some(record)) => {
                    remaining = remaining.saturating_sub(record.content.len());
                    truncated |= record.truncated;
                    rules.push(record);
                }
                Ok(None) => {}
                Err(warning) => warnings.push(warning),
            }
        }
        if rules.is_empty() {
            warnings.push("no applicable AGENTS.md or .codex/rules files were found".to_owned());
        }
        if remaining == 0 {
            warnings.push("project rule total byte budget was exhausted".to_owned());
            truncated = true;
        }
        let manifests = discover_manifests(&root, &mut warnings);
        let effective_hash = bundle_hash(&root, &rules, &manifests);
        Ok(ProjectContextBundle {
            context_ref: format!("project-context:sha256:{effective_hash}"),
            workspace: display(&root),
            effective_hash,
            rules,
            manifests,
            warnings,
            truncated,
        })
    }

    fn load_range(
        &self,
        root: &Path,
        candidates: Vec<(PathBuf, PathBuf, ProjectRuleKind)>,
        mut warnings: Vec<String>,
        range: &ProjectContextRange,
    ) -> RuntimeResult<ProjectContextBundle> {
        let Some((precedence, (path, scope, kind))) = candidates
            .into_iter()
            .take(self.max_files)
            .enumerate()
            .find(|(_, (path, _, _))| display(path) == range.path)
        else {
            return Err(RuntimeError::new(
                "project_context_range_invalid",
                "continuation path is not an applicable project rule",
            ));
        };
        let record = read_rule_range(
            root,
            &path,
            &scope,
            kind,
            precedence,
            self.max_file_bytes.min(self.max_total_bytes),
            range,
        )?;
        let manifests = discover_manifests(root, &mut warnings);
        let effective_hash = bundle_hash(root, std::slice::from_ref(&record), &manifests);
        Ok(ProjectContextBundle {
            context_ref: format!("project-context:sha256:{effective_hash}"),
            workspace: display(root),
            effective_hash,
            truncated: record.truncated,
            rules: vec![record],
            manifests,
            warnings,
        })
    }
}

fn canonical_target_dirs(root: &Path, targets: &[PathBuf]) -> RuntimeResult<Vec<PathBuf>> {
    let requested = if targets.is_empty() {
        vec![root.to_path_buf()]
    } else {
        targets.to_vec()
    };
    requested
        .into_iter()
        .map(|target| {
            let candidate = if target.is_absolute() {
                target
            } else {
                root.join(target)
            };
            let canonical = candidate.canonicalize().map_err(|error| {
                RuntimeError::new(
                    "project_context_target_invalid",
                    format!("target is unavailable: {error}"),
                )
            })?;
            if !canonical.starts_with(root) {
                return Err(RuntimeError::new(
                    "project_context_target_outside_workspace",
                    "project context target resolves outside the task workspace",
                ));
            }
            Ok(if canonical.is_dir() {
                canonical
            } else {
                canonical.parent().unwrap_or(root).to_path_buf()
            })
        })
        .collect()
}

fn scoped_ancestors(root: &Path, target: &Path) -> Vec<PathBuf> {
    let mut values = vec![root.to_path_buf()];
    let Ok(relative) = target.strip_prefix(root) else {
        return values;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        values.push(current.clone());
    }
    values
}

fn discover_codex_rules(
    root: &Path,
    candidates: &mut BTreeSet<(PathBuf, PathBuf, ProjectRuleKind)>,
    warnings: &mut Vec<String>,
) -> RuntimeResult<()> {
    let directory = root.join(".codex/rules");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(RuntimeError::new(
                "project_context_rules_unavailable",
                format!(".codex/rules cannot be read: {error}"),
            ));
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!(".codex/rules entry unavailable: {error}"));
                continue;
            }
        };
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("{} metadata unavailable: {error}", display(&path)));
                continue;
            }
        };
        if metadata.file_type().is_file() && path.extension().is_some_and(|value| value == "md") {
            candidates.insert((path, root.to_path_buf(), ProjectRuleKind::CodexRule));
        } else if metadata.file_type().is_symlink() {
            warnings.push(format!(
                "{} was skipped because project rules cannot be symlinks",
                display(&path)
            ));
        }
    }
    Ok(())
}

fn read_rule(
    root: &Path,
    path: &Path,
    scope: &Path,
    kind: ProjectRuleKind,
    precedence: usize,
    max_file_bytes: usize,
    remaining: usize,
) -> Result<Option<ProjectRuleRecord>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(rule_io_warning(path, "metadata", &error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "invalid_type: {} was skipped because it is not a regular file",
            display(path)
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("{} cannot be resolved: {error}", display(path)))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "{} resolves outside the workspace and was skipped",
            display(path)
        ));
    }
    let bytes = fs::read(&canonical).map_err(|error| rule_io_warning(path, "read", &error))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("invalid_utf8: {} is not valid UTF-8", display(path)))?;
    let take = max_file_bytes.min(remaining).min(bytes.len());
    let boundary = floor_char_boundary(text, take);
    let content = text[..boundary].to_owned();
    let truncated = boundary < bytes.len();
    let content_hash = sha256_hex(&bytes);
    Ok(Some(ProjectRuleRecord {
        path: display(&canonical),
        scope_root: display(scope),
        kind,
        version_token: format!("sha256:{content_hash}"),
        content_hash,
        precedence,
        content,
        truncated,
        next_range: truncated.then(|| format!("bytes={boundary}..{}", bytes.len())),
        warnings: truncated
            .then(|| "rule content was truncated by the configured byte budget".to_owned())
            .into_iter()
            .collect(),
    }))
}

fn floor_char_boundary(value: &str, requested: usize) -> usize {
    let mut boundary = requested.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn sha256_hex(value: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_ref());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
#[path = "project_context_tests.rs"]
mod tests;
