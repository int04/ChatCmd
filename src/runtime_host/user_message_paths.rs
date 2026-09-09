use std::{collections::BTreeSet, path::PathBuf};

pub(super) fn extract_explicit_absolute_paths(content: &str) -> Vec<PathBuf> {
    let mut candidates = quoted_candidates(content);
    candidates.extend(content.split_whitespace().map(str::to_owned));
    for line in content.lines() {
        candidates.extend(existing_absolute_paths_in_line(line));
    }

    let mut unique = BTreeSet::new();
    for candidate in candidates {
        let cleaned = clean_candidate(&candidate);
        if cleaned.is_empty() {
            continue;
        }
        let path = PathBuf::from(cleaned);
        if !path.is_absolute() || !path.exists() {
            continue;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if canonical.parent().is_none() {
            continue;
        }
        unique.insert(canonical);
        if unique.len() >= 64 {
            break;
        }
    }
    unique.into_iter().collect()
}

fn quoted_candidates(content: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut quoted = None::<(char, usize)>;
    for (index, ch) in content.char_indices() {
        if matches!(ch, '`' | '"' | '\'') {
            if let Some((delimiter, start)) = quoted {
                if delimiter == ch {
                    candidates.push(content[start..index].to_owned());
                    quoted = None;
                }
            } else {
                quoted = Some((ch, index + ch.len_utf8()));
            }
        }
    }
    candidates
}

fn existing_absolute_paths_in_line(line: &str) -> Vec<String> {
    let mut results = Vec::new();
    let indices = line
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for start in indices {
        let tail = &line[start..];
        if !looks_like_absolute_path_start(line, start, tail) {
            continue;
        }
        if let Some(path) = longest_existing_prefix(tail) {
            results.push(path);
        }
    }
    results
}

fn looks_like_absolute_path_start(line: &str, start: usize, tail: &str) -> bool {
    let boundary = start == 0
        || line[..start].chars().next_back().is_some_and(|ch| {
            ch.is_whitespace() || matches!(ch, ':' | '=' | '(' | '[' | '{' | '`' | '"' | '\'')
        });
    if !boundary {
        return false;
    }
    let bytes = tail.as_bytes();
    let windows = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    windows || tail.starts_with('/')
}

fn longest_existing_prefix(tail: &str) -> Option<String> {
    let mut ends = tail
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    ends.push(tail.len());
    ends.sort_unstable_by(|left, right| right.cmp(left));
    for end in ends {
        let cleaned = clean_candidate(&tail[..end]);
        if cleaned.is_empty() {
            continue;
        }
        let path = PathBuf::from(cleaned);
        if path.is_absolute() && path.exists() {
            return Some(cleaned.to_owned());
        }
    }
    None
}

fn clean_candidate(candidate: &str) -> &str {
    candidate.trim().trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extracts_unquoted_existing_path_with_spaces() {
        let root = TempDir::new().expect("temp root");
        let folder = root.path().join("folder with spaces");
        std::fs::create_dir_all(&folder).expect("create folder");
        let content = format!("Thư mục dự án: {} để thực hiện yêu cầu", folder.display());
        let paths = extract_explicit_absolute_paths(&content);
        assert!(paths.contains(&folder.canonicalize().expect("canonical folder")));
    }

    #[test]
    fn quoted_path_remains_supported() {
        let root = TempDir::new().expect("temp root");
        let folder = root.path().join("quoted folder");
        std::fs::create_dir_all(&folder).expect("create folder");
        let content = format!("project `{}`", folder.display());
        let paths = extract_explicit_absolute_paths(&content);
        assert!(paths.contains(&folder.canonicalize().expect("canonical folder")));
    }

    #[test]
    fn relative_paths_are_not_granted() {
        assert!(extract_explicit_absolute_paths("src/runtime_host").is_empty());
    }
}
