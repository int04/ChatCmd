use crate::{RuntimeError, RuntimeResult, SkillReadResult, SkillSummary};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use tokio::process::Command;

mod support;
use support::*;

const MAX_SKILL_BYTES: u64 = 2_000_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOptionChoice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOption {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub option_type: String,
    pub value: Value,
    pub choices: Option<Vec<SkillOptionChoice>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSkill {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub icon_path: Option<String>,
    pub source: String,
    pub source_url: Option<String>,
    pub enabled: bool,
    pub can_delete: bool,
    pub options: Vec<SkillOption>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallCandidate {
    pub name: String,
    pub title: String,
    pub description: String,
    pub path: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallPreview {
    pub repository_url: String,
    pub skills: Vec<SkillInstallCandidate>,
    pub skipped_invalid: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SkillSettings {
    #[serde(default)]
    skills: HashMap<String, SkillSetting>,
}
#[derive(Debug, Serialize, Deserialize)]
struct SkillSetting {
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    options: HashMap<String, Value>,
}
impl Default for SkillSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            options: HashMap::new(),
        }
    }
}
#[derive(Clone)]
pub struct SkillService {
    roots: Vec<(String, PathBuf)>,
    global_roots: Vec<PathBuf>,
    install_root: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    max_characters: usize,
}

#[derive(Clone)]
struct DiscoveredSkill {
    id: String,
    name: String,
    title: String,
    description: String,
    directory: PathBuf,
    source: String,
    source_url: Option<String>,
    enabled: bool,
    can_delete: bool,
    icon_path: Option<String>,
    options: Vec<SkillOption>,
    precedence: usize,
}

struct InstallCandidateSource {
    candidate: SkillInstallCandidate,
    directory: PathBuf,
}

struct InstallCandidateDiscovery {
    skills: Vec<InstallCandidateSource>,
    skipped_invalid: usize,
}

impl SkillService {
    #[must_use]
    pub fn new(
        user_home: Option<&Path>,
        repository_root: Option<&Path>,
        max_characters: usize,
    ) -> Self {
        let mut roots = Vec::new();
        if let Some(repository) = repository_root {
            roots.push(("workspace".into(), repository.join(".agents/skills")));
            roots.push(("workspace".into(), repository.join(".codex/skills")));
        }
        let mut global_roots = Vec::new();
        if let Some(home) = user_home {
            global_roots.push(home.join(".agents/skills"));
            global_roots.push(home.join(".codex/skills"));
            roots.push(("global".into(), home.join(".agents/skills")));
            roots.push(("global".into(), home.join(".codex/skills")));
        }
        let install_root = global_roots.first().cloned();
        let settings_path = user_home.map(|home| home.join(".chatcmd/skills.json"));
        Self {
            roots,
            global_roots,
            install_root,
            settings_path,
            max_characters: max_characters.clamp(1, 1_000_000),
        }
    }

    pub async fn list(&self) -> RuntimeResult<Vec<SkillSummary>> {
        self.list_from_roots(&self.roots)
    }

    pub async fn list_for_workspace(
        &self,
        repository_root: Option<&Path>,
    ) -> RuntimeResult<Vec<SkillSummary>> {
        let roots = self.roots_for_workspace(repository_root);
        self.list_from_roots(&roots)
    }

    pub async fn read(&self, skill_id: &str) -> RuntimeResult<SkillReadResult> {
        let selected = self
            .list()
            .await?
            .into_iter()
            .find(|skill| skill.id == skill_id)
            .ok_or_else(|| {
                RuntimeError::new("skill_not_found", "skill is unavailable or shadowed")
            })?;
        self.read_selected(selected).await
    }

    pub async fn read_for_workspace(
        &self,
        skill_id: &str,
        repository_root: Option<&Path>,
    ) -> RuntimeResult<SkillReadResult> {
        let selected = self
            .list_for_workspace(repository_root)
            .await?
            .into_iter()
            .find(|skill| skill.id == skill_id)
            .ok_or_else(|| {
                RuntimeError::new("skill_not_found", "skill is unavailable or shadowed")
            })?;
        self.read_selected(selected).await
    }

    async fn read_selected(&self, selected: SkillSummary) -> RuntimeResult<SkillReadResult> {
        let content = tokio::fs::read_to_string(PathBuf::from(&selected.source).join("SKILL.md"))
            .await
            .map_err(io_error)?;
        let truncated = content.chars().count() > self.max_characters;
        Ok(SkillReadResult {
            id: selected.id,
            name: selected.name,
            source: selected.source,
            instructions: content.chars().take(self.max_characters).collect(),
            truncated,
        })
    }

    pub async fn list_global(&self) -> RuntimeResult<Vec<ManagedSkill>> {
        let mut values: Vec<_> = self
            .discover_all()?
            .into_iter()
            .filter(|skill| skill.source == "global")
            .collect();
        values.sort_by_key(|skill| (skill.precedence, skill.name.to_lowercase()));
        let mut seen = HashSet::new();
        Ok(values
            .into_iter()
            .filter(|skill| seen.insert(skill.name.to_lowercase()))
            .map(to_managed)
            .collect())
    }

    pub async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> RuntimeResult<Option<ManagedSkill>> {
        let Some(skill) = self.global_by_id(id)? else {
            return Ok(None);
        };
        let mut settings = self.load_settings()?;
        settings
            .skills
            .entry(skill_key(&skill))
            .or_default()
            .enabled = enabled;
        self.save_settings(&settings)?;
        let mut skill = skill;
        skill.enabled = enabled;
        Ok(Some(to_managed(skill)))
    }

    pub async fn set_options(
        &self,
        id: &str,
        values: HashMap<String, Value>,
    ) -> RuntimeResult<Option<ManagedSkill>> {
        let Some(mut skill) = self.global_by_id(id)? else {
            return Ok(None);
        };
        let definitions = option_definitions(&skill.name);
        if definitions.is_empty() && !values.is_empty() {
            return Err(RuntimeError::new(
                "invalid_skill_options",
                "This skill does not expose configurable options.",
            ));
        }
        for (key, value) in &values {
            let Some(definition) = definitions.iter().find(|item| item.key == key) else {
                return Err(RuntimeError::new(
                    "invalid_skill_options",
                    format!("Unknown skill option '{key}'."),
                ));
            };
            if definition.option_type == "select" {
                let Some(text) = value.as_str() else {
                    return Err(RuntimeError::new(
                        "invalid_skill_options",
                        format!("Invalid value for option '{key}'."),
                    ));
                };
                if !definition.choices.contains(&text) {
                    return Err(RuntimeError::new(
                        "invalid_skill_options",
                        format!("Invalid value for option '{key}'."),
                    ));
                }
            }
        }
        let mut settings = self.load_settings()?;
        settings
            .skills
            .entry(skill_key(&skill))
            .or_default()
            .options = values.clone();
        self.save_settings(&settings)?;
        skill.options = create_options(&skill.name, Some(&values));
        Ok(Some(to_managed(skill)))
    }

    pub async fn delete(&self, id: &str) -> RuntimeResult<bool> {
        let Some(skill) = self.global_by_id(id)? else {
            return Ok(false);
        };
        if !skill.can_delete {
            return Err(RuntimeError::new(
                "skill_delete_denied",
                "This skill cannot be deleted by ChatCMD.",
            ));
        }
        fs::remove_dir_all(&skill.directory).map_err(io_error)?;
        let mut settings = self.load_settings()?;
        settings.skills.remove(&skill_key(&skill));
        self.save_settings(&settings)?;
        Ok(true)
    }

    pub async fn icon(&self, id: &str) -> RuntimeResult<Option<(PathBuf, &'static str)>> {
        let Some(skill) = self.global_by_id(id)? else {
            return Ok(None);
        };
        let Some(path) = skill.icon_path.map(PathBuf::from) else {
            return Ok(None);
        };
        let content_type = match path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "application/octet-stream",
        };
        Ok(Some((path, content_type)))
    }

    pub async fn preview_install(
        &self,
        repository_url: &str,
    ) -> RuntimeResult<SkillInstallPreview> {
        let source = parse_github_url(repository_url)?;
        let checkout = clone_repository(&source).await?;
        let clone_root = checkout.path().join("repo");
        let candidate_root = source
            .subdirectory
            .as_deref()
            .map_or_else(|| clone_root.clone(), |path| clone_root.join(path));
        if !candidate_root.is_dir() {
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "The GitHub subdirectory does not exist in the selected repository revision.",
            ));
        }
        let installed_names = self
            .discover_all()?
            .into_iter()
            .filter(|skill| skill.source == "global")
            .map(|skill| skill.name.to_lowercase())
            .collect();
        let discovery =
            discover_install_candidates(&candidate_root, &clone_root, &installed_names)?;
        if discovery.skills.is_empty() {
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "No valid skills were found. Each skill directory must contain a SKILL.md with a lowercase name and description.",
            ));
        }
        Ok(SkillInstallPreview {
            repository_url: source.repository_url,
            skills: discovery
                .skills
                .into_iter()
                .map(|skill| skill.candidate)
                .collect(),
            skipped_invalid: discovery.skipped_invalid,
        })
    }

    pub async fn install(
        &self,
        repository_url: &str,
        skill_paths: &[String],
    ) -> RuntimeResult<Vec<ManagedSkill>> {
        let source = parse_github_url(repository_url)?;
        let install_root = self.install_root.as_ref().ok_or_else(|| {
            RuntimeError::new("skill_install_unavailable", "User home is unavailable.")
        })?;
        let checkout = clone_repository(&source).await?;
        let clone_root = checkout.path().join("repo");
        let candidate_root = source
            .subdirectory
            .as_deref()
            .map_or_else(|| clone_root.clone(), |path| clone_root.join(path));
        if !candidate_root.is_dir() {
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "The GitHub subdirectory does not exist in the selected repository revision.",
            ));
        }
        let installed_names = self
            .discover_all()?
            .into_iter()
            .filter(|skill| skill.source == "global")
            .map(|skill| skill.name.to_lowercase())
            .collect();
        let discovery =
            discover_install_candidates(&candidate_root, &clone_root, &installed_names)?;
        if discovery.skills.is_empty() {
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "No valid skills were found. Each skill directory must contain a SKILL.md with a lowercase name and description.",
            ));
        }
        let requested_paths: Vec<&str> = if skill_paths.is_empty() {
            if discovery.skills.len() != 1 {
                return Err(RuntimeError::new(
                    "skill_selection_required",
                    "Repository contains multiple skills. Preview it and choose which skills to install.",
                ));
            }
            vec![discovery.skills[0].candidate.path.as_str()]
        } else {
            skill_paths.iter().map(String::as_str).collect()
        };
        let unique_paths: HashSet<_> = requested_paths.iter().copied().collect();
        if unique_paths.len() != requested_paths.len()
            || requested_paths.len() > MAX_DISCOVERED_SKILLS
        {
            return Err(RuntimeError::new(
                "invalid_skill_selection",
                "Select between 1 and 200 unique skill paths.",
            ));
        }
        let selected: Vec<_> = discovery
            .skills
            .iter()
            .filter(|skill| unique_paths.contains(skill.candidate.path.as_str()))
            .collect();
        if selected.len() != requested_paths.len() {
            return Err(RuntimeError::new(
                "invalid_skill_selection",
                "One or more selected skill paths are not available in this repository.",
            ));
        }
        let mut selected_names = HashSet::new();
        for skill in &selected {
            if skill.candidate.installed || install_root.join(&skill.candidate.name).exists() {
                return Err(RuntimeError::new(
                    "skill_conflict",
                    format!("Skill '{}' is already installed.", skill.candidate.name),
                ));
            }
            if !selected_names.insert(skill.candidate.name.to_lowercase()) {
                return Err(RuntimeError::new(
                    "invalid_skill_selection",
                    format!(
                        "More than one selected directory declares the skill name '{}'.",
                        skill.candidate.name
                    ),
                ));
            }
        }

        let installed_destinations =
            install_candidate_directories(install_root, &selected, &source.repository_url)?;

        let mut installed = Vec::with_capacity(selected.len());
        for skill in selected {
            let discovered = match self.global_by_name(&skill.candidate.name) {
                Ok(Some(discovered)) => discovered,
                Ok(None) => {
                    rollback_install(&installed_destinations);
                    return Err(RuntimeError::new(
                        "skill_install_failed",
                        "Installed skills could not be discovered.",
                    ));
                }
                Err(error) => {
                    rollback_install(&installed_destinations);
                    return Err(error);
                }
            };
            installed.push(to_managed(discovered));
        }
        Ok(installed)
    }

    fn global_by_id(&self, id: &str) -> RuntimeResult<Option<DiscoveredSkill>> {
        Ok(self
            .discover_all()?
            .into_iter()
            .find(|skill| skill.source == "global" && skill.id == id))
    }
    fn global_by_name(&self, name: &str) -> RuntimeResult<Option<DiscoveredSkill>> {
        Ok(self
            .discover_all()?
            .into_iter()
            .find(|skill| skill.source == "global" && skill.name.eq_ignore_ascii_case(name)))
    }

    fn roots_for_workspace(&self, repository_root: Option<&Path>) -> Vec<(String, PathBuf)> {
        let mut roots = Vec::new();
        if let Some(repository) = repository_root {
            roots.push(("workspace".into(), repository.join(".agents/skills")));
            roots.push(("workspace".into(), repository.join(".codex/skills")));
        }
        roots.extend(
            self.global_roots
                .iter()
                .cloned()
                .map(|root| ("global".into(), root)),
        );
        roots
    }

    fn list_from_roots(&self, roots: &[(String, PathBuf)]) -> RuntimeResult<Vec<SkillSummary>> {
        let all = self.discover_from_roots(roots)?;
        let mut seen = HashSet::new();
        Ok(all
            .into_iter()
            .filter(|skill| skill.enabled && seen.insert(skill.name.to_lowercase()))
            .map(|skill| SkillSummary {
                id: skill.name.clone(),
                name: skill.name,
                title: skill.title,
                description: skill.description,
                source: skill.directory.to_string_lossy().into_owned(),
            })
            .collect())
    }

    fn discover_all(&self) -> RuntimeResult<Vec<DiscoveredSkill>> {
        self.discover_from_roots(&self.roots)
    }

    fn discover_from_roots(
        &self,
        roots: &[(String, PathBuf)],
    ) -> RuntimeResult<Vec<DiscoveredSkill>> {
        let settings = self.load_settings()?;
        let mut values = Vec::new();
        for (index, (source, root)) in roots.iter().enumerate() {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            let mut directories: Vec<_> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.path())
                .collect();
            directories.sort_by_key(|path| {
                path.file_name()
                    .map(|v| v.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            });
            for directory in directories {
                if directory
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                if !directory.join("SKILL.md").is_file() {
                    continue;
                }
                if let Ok(skill) = self.parse_skill(directory, source, index, &settings) {
                    values.push(skill);
                }
            }
        }
        Ok(values)
    }

    fn parse_skill(
        &self,
        directory: PathBuf,
        source: &str,
        precedence: usize,
        settings: &SkillSettings,
    ) -> RuntimeResult<DiscoveredSkill> {
        let file = directory.join("SKILL.md");
        if fs::metadata(&file).map_err(io_error)?.len() > MAX_SKILL_BYTES {
            return Err(RuntimeError::new(
                "skill_too_large",
                "SKILL.md exceeds 2 MB.",
            ));
        }
        let metadata = parse_frontmatter(&fs::read_to_string(&file).map_err(io_error)?);
        let fallback = directory
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| "skill".into());
        let name = metadata
            .get("name")
            .cloned()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| fallback.clone());
        let title = openai_value(&directory, "display_name").unwrap_or_else(|| name.clone());
        let key = format!("{source}:{}", directory.to_string_lossy());
        let stored = settings.skills.get(&key);
        let icon_path = openai_value(&directory, "icon_small")
            .and_then(|value| resolve_icon(&directory, &value));
        let can_delete = source == "global"
            && self
                .global_roots
                .iter()
                .any(|root| directory.starts_with(root))
            && !directory
                .components()
                .any(|part| part.as_os_str() == ".system");
        Ok(DiscoveredSkill {
            id: name.clone(),
            name: name.clone(),
            title,
            description: metadata.get("description").cloned().unwrap_or_default(),
            directory: directory.clone(),
            source: source.into(),
            source_url: source_url(&directory),
            enabled: stored.map(|v| v.enabled).unwrap_or(true),
            can_delete,
            icon_path,
            options: create_options(&name, stored.map(|v| &v.options)),
            precedence,
        })
    }

    fn load_settings(&self) -> RuntimeResult<SkillSettings> {
        let Some(path) = &self.settings_path else {
            return Ok(SkillSettings::default());
        };
        if !path.exists() {
            return Ok(SkillSettings::default());
        }
        serde_json::from_slice(&fs::read(path).map_err(io_error)?)
            .map_err(|error| RuntimeError::new("skill_settings_invalid", error.to_string()))
    }
    fn save_settings(&self, settings: &SkillSettings) -> RuntimeResult<()> {
        let Some(path) = &self.settings_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(settings)
                .map_err(|error| RuntimeError::new("skill_settings_invalid", error.to_string()))?,
        )
        .map_err(io_error)
    }
}

async fn clone_repository(source: &GitHubSource) -> RuntimeResult<tempfile::TempDir> {
    let checkout = tempfile::Builder::new()
        .prefix("chatcmd-skill-")
        .tempdir()
        .map_err(io_error)?;
    let clone_root = checkout.path().join("repo");
    let mut args = vec![
        "clone".to_owned(),
        "--depth".into(),
        "1".into(),
        "--single-branch".into(),
        "--filter=blob:none".into(),
    ];
    if let Some(reference) = &source.reference {
        args.extend(["--branch".into(), reference.clone()]);
    }
    args.extend([
        "--".into(),
        source.clone_url.clone(),
        clone_root.to_string_lossy().into_owned(),
    ]);
    let output = Command::new("git")
        .args(&args)
        .current_dir(checkout.path())
        .output()
        .await
        .map_err(io_error)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(2_000)
            .collect::<String>();
        return Err(RuntimeError::new(
            "skill_clone_failed",
            if detail.is_empty() {
                "Git could not clone the selected repository.".into()
            } else {
                detail
            },
        ));
    }
    Ok(checkout)
}

fn discover_install_candidates(
    candidate_root: &Path,
    clone_root: &Path,
    installed_names: &HashSet<String>,
) -> RuntimeResult<InstallCandidateDiscovery> {
    let mut skill_files = Vec::new();
    collect_skill_files(candidate_root, &mut skill_files)?;
    skill_files.sort();
    let mut skills = Vec::with_capacity(skill_files.len());
    let mut skipped_invalid = 0usize;
    for skill_file in skill_files {
        if fs::metadata(&skill_file).map_err(io_error)?.len() > MAX_SKILL_BYTES {
            skipped_invalid += 1;
            continue;
        }
        let Some(directory) = skill_file.parent() else {
            skipped_invalid += 1;
            continue;
        };
        let metadata = parse_frontmatter(&fs::read_to_string(&skill_file).map_err(io_error)?);
        let name = metadata.get("name").cloned().unwrap_or_default();
        let description = metadata.get("description").cloned().unwrap_or_default();
        if !valid_skill_name(&name) || description.trim().is_empty() {
            skipped_invalid += 1;
            continue;
        }
        if let Err(error) = validate_install_tree(directory) {
            if matches!(
                error.code.as_str(),
                "invalid_skill_repository" | "skill_too_large"
            ) {
                skipped_invalid += 1;
                continue;
            }
            return Err(error);
        }
        let relative = directory.strip_prefix(clone_root).map_err(|_| {
            RuntimeError::new(
                "invalid_skill_repository",
                "A discovered skill is outside the cloned repository.",
            )
        })?;
        let path = if relative.as_os_str().is_empty() {
            ".".into()
        } else {
            relative.to_string_lossy().replace('\\', "/")
        };
        let title = openai_value(directory, "display_name").unwrap_or_else(|| name.clone());
        skills.push(InstallCandidateSource {
            candidate: SkillInstallCandidate {
                installed: installed_names.contains(&name.to_lowercase()),
                name,
                title,
                description: description.trim().to_owned(),
                path,
            },
            directory: directory.to_path_buf(),
        });
    }
    Ok(InstallCandidateDiscovery {
        skills,
        skipped_invalid,
    })
}

fn install_candidate_directories(
    install_root: &Path,
    selected: &[&InstallCandidateSource],
    repository_url: &str,
) -> RuntimeResult<Vec<PathBuf>> {
    fs::create_dir_all(install_root).map_err(io_error)?;
    let staging = tempfile::Builder::new()
        .prefix(".chatcmd-skill-install-")
        .tempdir_in(install_root)
        .map_err(io_error)?;
    for skill in selected {
        let staged_destination = staging.path().join(&skill.candidate.name);
        copy_tree(&skill.directory, &staged_destination)?;
        let source_metadata = serde_json::to_vec_pretty(&serde_json::json!({
            "repositoryUrl": repository_url,
            "skillPath": skill.candidate.path.as_str(),
        }))
        .map_err(|error| RuntimeError::new("skill_install_failed", error.to_string()))?;
        fs::write(
            staged_destination.join(".cmdgpt-source.json"),
            source_metadata,
        )
        .map_err(io_error)?;
    }

    let mut installed_destinations = Vec::with_capacity(selected.len());
    for skill in selected {
        let destination = install_root.join(&skill.candidate.name);
        if let Err(error) = fs::rename(staging.path().join(&skill.candidate.name), &destination) {
            rollback_install(&installed_destinations);
            return Err(io_error(error));
        }
        installed_destinations.push(destination);
    }
    Ok(installed_destinations)
}

fn rollback_install(destinations: &[PathBuf]) {
    for destination in destinations.iter().rev() {
        let _ = fs::remove_dir_all(destination);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, marker: &str) {
        let directory = root.join(".codex/skills").join(name);
        fs::create_dir_all(&directory).expect("create skill directory");
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n\n{marker}\n"),
        )
        .expect("write skill");
    }

    fn write_install_candidate(root: &Path, path: &str, name: &str, description: &str) {
        let directory = root.join(path);
        fs::create_dir_all(&directory).expect("create install candidate");
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n"),
        )
        .expect("write install candidate");
    }

    #[test]
    fn repository_discovery_returns_all_valid_skills_in_path_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repository = temp.path().join("repository");
        write_install_candidate(
            &repository,
            "skills/extension-test",
            "extension-test",
            "Test browser extensions",
        );
        write_install_candidate(
            &repository,
            "skills/extension-create",
            "extension-create",
            "Create browser extensions",
        );
        write_install_candidate(
            &repository,
            "skills/invalid",
            "Invalid Name",
            "Invalid skill name",
        );
        let installed_names = HashSet::from(["extension-test".to_owned()]);

        let discovery = discover_install_candidates(&repository, &repository, &installed_names)
            .expect("discover install candidates");

        assert_eq!(discovery.skipped_invalid, 1);
        assert_eq!(discovery.skills.len(), 2);
        assert_eq!(
            discovery
                .skills
                .iter()
                .map(|skill| skill.candidate.path.as_str())
                .collect::<Vec<_>>(),
            vec!["skills/extension-create", "skills/extension-test"]
        );
        assert!(!discovery.skills[0].candidate.installed);
        assert!(discovery.skills[1].candidate.installed);
    }

    #[test]
    fn batch_install_copies_selected_skills_and_source_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repository = temp.path().join("repository");
        let install_root = temp.path().join("home/.agents/skills");
        write_install_candidate(
            &repository,
            "skills/extension-create",
            "extension-create",
            "Create browser extensions",
        );
        write_install_candidate(
            &repository,
            "skills/extension-test",
            "extension-test",
            "Test browser extensions",
        );
        let discovery = discover_install_candidates(&repository, &repository, &HashSet::new())
            .expect("discover install candidates");
        let selected: Vec<_> = discovery.skills.iter().collect();

        let installed = install_candidate_directories(
            &install_root,
            &selected,
            "https://github.com/quangpl/browser-extension-skills",
        )
        .expect("install selected candidates");

        assert_eq!(installed.len(), 2);
        assert!(install_root.join("extension-create/SKILL.md").is_file());
        assert!(install_root.join("extension-test/SKILL.md").is_file());
        let source: Value = serde_json::from_slice(
            &fs::read(
                install_root
                    .join("extension-create")
                    .join(".cmdgpt-source.json"),
            )
            .expect("read source metadata"),
        )
        .expect("parse source metadata");
        assert_eq!(
            source.get("repositoryUrl").and_then(Value::as_str),
            Some("https://github.com/quangpl/browser-extension-skills")
        );
        assert_eq!(
            source.get("skillPath").and_then(Value::as_str),
            Some("skills/extension-create")
        );
    }

    #[test]
    fn root_skill_install_excludes_git_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repository = temp.path().join("repository");
        let install_root = temp.path().join("home/.agents/skills");
        write_install_candidate(
            &repository,
            "",
            "root-skill",
            "Skill stored at repository root",
        );
        let git_metadata = repository.join(".git/objects");
        fs::create_dir_all(&git_metadata).expect("create git metadata");
        fs::write(git_metadata.join("noise"), "not skill content").expect("write git metadata");
        let discovery = discover_install_candidates(&repository, &repository, &HashSet::new())
            .expect("discover root skill");
        let selected: Vec<_> = discovery.skills.iter().collect();

        install_candidate_directories(
            &install_root,
            &selected,
            "https://github.com/example/root-skill",
        )
        .expect("install root skill");

        assert!(install_root.join("root-skill/SKILL.md").is_file());
        assert!(!install_root.join("root-skill/.git").exists());
    }

    #[tokio::test]
    async fn workspace_can_switch_without_recreating_skill_service() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let startup = temp.path().join("startup");
        let project_a = temp.path().join("project-a");
        let project_b = temp.path().join("project-b");
        write_skill(&home, "shared-skill", "global");
        write_skill(&startup, "startup-skill", "startup");
        write_skill(&project_a, "shared-skill", "project-a");
        write_skill(&project_b, "shared-skill", "project-b");

        let service = SkillService::new(Some(&home), Some(&startup), 10_000);
        let from_a = service
            .read_for_workspace("shared-skill", Some(&project_a))
            .await
            .expect("read project a skill");
        let from_b = service
            .read_for_workspace("shared-skill", Some(&project_b))
            .await
            .expect("read project b skill");

        assert!(from_a.instructions.contains("project-a"));
        assert!(from_b.instructions.contains("project-b"));
        assert!(
            from_a
                .source
                .starts_with(project_a.to_string_lossy().as_ref())
        );
        assert!(
            from_b
                .source
                .starts_with(project_b.to_string_lossy().as_ref())
        );
    }
}
