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
                if !definition.choices.iter().any(|choice| *choice == text) {
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

    pub async fn install(&self, repository_url: &str) -> RuntimeResult<ManagedSkill> {
        let source = parse_github_url(repository_url)?;
        let install_root = self.install_root.clone().ok_or_else(|| {
            RuntimeError::new("skill_install_unavailable", "User home is unavailable.")
        })?;
        let temp_root =
            std::env::temp_dir().join(format!("chatcmd-skill-{}", uuid::Uuid::new_v4().simple()));
        let clone_root = temp_root.join("repo");
        fs::create_dir_all(&temp_root).map_err(io_error)?;
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
            source.clone_url.clone(),
            clone_root.to_string_lossy().into_owned(),
        ]);
        let output = Command::new("git")
            .args(&args)
            .current_dir(&temp_root)
            .output()
            .await
            .map_err(io_error)?;
        if !output.status.success() {
            let _ = fs::remove_dir_all(&temp_root);
            return Err(RuntimeError::new(
                "skill_clone_failed",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(2000)
                    .collect::<String>(),
            ));
        }
        let candidate = source
            .subdirectory
            .as_deref()
            .map_or(clone_root.clone(), |path| clone_root.join(path));
        let mut skill_files = Vec::new();
        collect_skill_files(&candidate, &mut skill_files)?;
        if skill_files.len() != 1 {
            let _ = fs::remove_dir_all(&temp_root);
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "GitHub URL must identify a repository or subdirectory containing exactly one SKILL.md.",
            ));
        }
        let source_dir = skill_files[0].parent().unwrap_or(&candidate).to_path_buf();
        validate_install_tree(&source_dir)?;
        let metadata =
            parse_frontmatter(&fs::read_to_string(source_dir.join("SKILL.md")).map_err(io_error)?);
        let name = metadata.get("name").cloned().unwrap_or_default();
        let description = metadata.get("description").cloned().unwrap_or_default();
        if !valid_skill_name(&name) || description.trim().is_empty() {
            let _ = fs::remove_dir_all(&temp_root);
            return Err(RuntimeError::new(
                "invalid_skill_repository",
                "SKILL.md frontmatter must declare a valid lowercase name and description.",
            ));
        }
        fs::create_dir_all(&install_root).map_err(io_error)?;
        let destination = install_root.join(&name);
        if destination.exists() {
            let _ = fs::remove_dir_all(&temp_root);
            return Err(RuntimeError::new(
                "skill_conflict",
                format!("Skill '{name}' is already installed."),
            ));
        }
        copy_tree(&source_dir, &destination)?;
        fs::write(
            destination.join(".cmdgpt-source.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"repositoryUrl": repository_url}))
                .unwrap_or_default(),
        )
        .map_err(io_error)?;
        let _ = fs::remove_dir_all(&temp_root);
        self.global_by_name(&name)?.map(to_managed).ok_or_else(|| {
            RuntimeError::new(
                "skill_install_failed",
                "Installed skill could not be discovered.",
            )
        })
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
