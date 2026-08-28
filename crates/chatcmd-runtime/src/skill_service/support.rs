use super::*;

pub(super) const MAX_INSTALL_FILES: usize = 2_000;
pub(super) const MAX_INSTALL_BYTES: u64 = 50_000_000;
pub(super) fn yes() -> bool {
    true
}

#[derive(Clone)]
pub(super) struct OptionDefinition {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub option_type: &'static str,
    pub default_value: &'static str,
    pub choices: &'static [&'static str],
}

pub(super) fn option_definitions(name: &str) -> Vec<OptionDefinition> {
    if name.eq_ignore_ascii_case("caveman") {
        vec![OptionDefinition {
            key: "intensity",
            label: "Intensity",
            description: "Default communication compression level.",
            option_type: "select",
            default_value: "full",
            choices: &[
                "lite",
                "full",
                "ultra",
                "wenyan-lite",
                "wenyan-full",
                "wenyan-ultra",
            ],
        }]
    } else {
        Vec::new()
    }
}

pub(super) fn create_options(
    name: &str,
    values: Option<&HashMap<String, Value>>,
) -> Vec<SkillOption> {
    option_definitions(name)
        .into_iter()
        .map(|definition| {
            let value = values
                .and_then(|v| v.get(definition.key))
                .cloned()
                .unwrap_or_else(|| Value::String(definition.default_value.into()));
            SkillOption {
                key: definition.key.into(),
                label: definition.label.into(),
                description: Some(definition.description.into()),
                option_type: definition.option_type.into(),
                value,
                choices: Some(
                    definition
                        .choices
                        .iter()
                        .map(|choice| SkillOptionChoice {
                            value: (*choice).into(),
                            label: (*choice).into(),
                        })
                        .collect(),
                ),
            }
        })
        .collect()
}

pub(super) fn skill_key(skill: &DiscoveredSkill) -> String {
    format!("{}:{}", skill.source, skill.directory.to_string_lossy())
}

pub(super) fn to_managed(skill: DiscoveredSkill) -> ManagedSkill {
    ManagedSkill {
        id: skill.id,
        title: skill.title,
        description: (!skill.description.is_empty()).then_some(skill.description),
        icon_path: skill.icon_path,
        source: skill.source,
        source_url: skill.source_url,
        enabled: skill.enabled,
        can_delete: skill.can_delete,
        options: skill.options,
    }
}

pub(super) struct GitHubSource {
    pub clone_url: String,
    pub reference: Option<String>,
    pub subdirectory: Option<String>,
}

pub(super) fn parse_github_url(value: &str) -> RuntimeResult<GitHubSource> {
    let path = value.strip_prefix("https://github.com/").ok_or_else(|| {
        RuntimeError::new(
            "invalid_repository_url",
            "repositoryUrl must be an HTTPS github.com repository or tree URL.",
        )
    })?;
    if path.contains(['?', '#', '@']) {
        return Err(RuntimeError::new(
            "invalid_repository_url",
            "Invalid GitHub repository URL.",
        ));
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if parts.len() < 2 {
        return Err(RuntimeError::new(
            "invalid_repository_url",
            "Invalid GitHub repository URL.",
        ));
    }
    let repo = parts[1].trim_end_matches(".git");
    let clone_url = format!("https://github.com/{}/{}.git", parts[0], repo);
    if parts.len() == 2 {
        return Ok(GitHubSource {
            clone_url,
            reference: None,
            subdirectory: None,
        });
    }
    if parts.len() < 5 || parts[2] != "tree" {
        return Err(RuntimeError::new(
            "invalid_repository_url",
            "Use a GitHub repository URL or /tree/{ref}/{skill-path} URL.",
        ));
    }
    Ok(GitHubSource {
        clone_url,
        reference: Some(parts[3].into()),
        subdirectory: Some(parts[4..].join("/")),
    })
}

pub(super) fn collect_skill_files(root: &Path, output: &mut Vec<PathBuf>) -> RuntimeResult<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_skill_files(&path, output)?;
            if output.len() > 1 {
                return Ok(());
            }
        } else if entry.file_name() == "SKILL.md" {
            output.push(path);
        }
    }
    Ok(())
}

pub(super) fn validate_install_tree(root: &Path) -> RuntimeResult<()> {
    fn walk(root: &Path, files: &mut usize, bytes: &mut u64) -> RuntimeResult<()> {
        for entry in fs::read_dir(root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let kind = entry.file_type().map_err(io_error)?;
            if kind.is_symlink() {
                return Err(RuntimeError::new(
                    "invalid_skill_repository",
                    "Skills containing symbolic links cannot be installed.",
                ));
            }
            if kind.is_dir() {
                walk(&entry.path(), files, bytes)?;
            } else {
                *files += 1;
                *bytes += entry.metadata().map_err(io_error)?.len();
                if *files > MAX_INSTALL_FILES || *bytes > MAX_INSTALL_BYTES {
                    return Err(RuntimeError::new(
                        "skill_too_large",
                        "Skill exceeds the safe installation size limit.",
                    ));
                }
            }
        }
        Ok(())
    }
    let mut files = 0usize;
    let mut bytes = 0u64;
    walk(root, &mut files, &mut bytes)
}

pub(super) fn copy_tree(source: &Path, destination: &Path) -> RuntimeResult<()> {
    fs::create_dir_all(destination).map_err(io_error)?;
    for entry in fs::read_dir(source).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target).map_err(io_error)?;
        }
    }
    Ok(())
}

pub(super) fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

pub(super) fn parse_frontmatter(content: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return values;
    }
    let mut multiline: Option<String> = None;
    let mut buffer = String::new();
    for line in lines {
        if line.trim() == "---" {
            if let Some(key) = multiline.take() {
                values.insert(key, buffer.trim().into());
            }
            break;
        }
        if let Some(key) = multiline.clone() {
            if line.starts_with(' ') || line.starts_with('\t') {
                if !buffer.is_empty() {
                    buffer.push(' ');
                }
                buffer.push_str(line.trim());
                continue;
            }
            values.insert(key, buffer.trim().into());
            multiline = None;
            buffer.clear();
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_owned();
            let value = value.trim();
            if value == ">" || value == "|" {
                multiline = Some(key);
            } else {
                values.insert(key, value.trim_matches(['"', '\'']).into());
            }
        }
    }
    values
}

pub(super) fn openai_value(directory: &Path, key: &str) -> Option<String> {
    let content = fs::read_to_string(directory.join("agents/openai.yaml")).ok()?;
    content.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&format!("{key}:"))
            .map(|value| value.trim().trim_matches(['"', '\'']).to_owned())
    })
}

pub(super) fn resolve_icon(directory: &Path, value: &str) -> Option<String> {
    if value.contains("://") {
        return None;
    }
    let path = directory.join(value.replace('/', &std::path::MAIN_SEPARATOR.to_string()));
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    (path.is_file()
        && matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
        && fs::metadata(&path).ok()?.len() <= 2_000_000)
        .then(|| path.to_string_lossy().into_owned())
}

pub(super) fn source_url(directory: &Path) -> Option<String> {
    let value: Value =
        serde_json::from_slice(&fs::read(directory.join(".cmdgpt-source.json")).ok()?).ok()?;
    value.get("repositoryUrl")?.as_str().map(str::to_owned)
}

pub(super) fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("io_error", error.to_string())
}
