use crate::{
    CommandOutput, CursorCodec, GitBranchData, GitBranchEntry, GitBranchMetadata, GitLogData,
    GitLogEntry, GitPathValue, GitRunOptions, GitStatusData, GitStatusEntry, RuntimeError,
    RuntimeResult,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Cursor},
    path::{Path, PathBuf},
};

const MAX_STRUCTURED_ITEMS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitCursorState {
    offset: usize,
}

#[derive(Clone)]
pub(crate) enum StructuredSource {
    File(PathBuf),
    Inline(Vec<u8>),
}

impl StructuredSource {
    fn reader(&self) -> RuntimeResult<Box<dyn BufRead>> {
        match self {
            Self::File(path) => Ok(Box::new(BufReader::with_capacity(
                64 * 1024,
                File::open(path).map_err(io_error)?,
            ))),
            Self::Inline(bytes) => Ok(Box::new(BufReader::new(Cursor::new(bytes.clone())))),
        }
    }
}

pub(crate) fn structured_source(output: &CommandOutput) -> StructuredSource {
    output
        .artifact_ref
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .map_or_else(
            || StructuredSource::Inline(output.stdout.as_bytes().to_vec()),
            StructuredSource::File,
        )
}

pub(crate) fn cursor_scope(cwd: &Path, kind: &str) -> String {
    format!("{}\0{kind}", cwd.display())
}

fn page_start(
    options: &GitRunOptions,
    codec: &CursorCodec,
    kind: &str,
    scope: &str,
) -> RuntimeResult<usize> {
    options.cursor.as_deref().map_or(Ok(0), |cursor| {
        codec
            .decode::<GitCursorState>(cursor, kind, scope)
            .map(|state| state.offset)
    })
}

fn next_cursor(
    codec: &CursorCodec,
    kind: &str,
    scope: &str,
    offset: usize,
    has_more: bool,
) -> RuntimeResult<Option<String>> {
    if has_more {
        codec
            .encode(kind, scope, &GitCursorState { offset }, None)
            .map(Some)
    } else {
        Ok(None)
    }
}

fn structured_limit(options: &GitRunOptions) -> usize {
    options.limit.clamp(1, MAX_STRUCTURED_ITEMS)
}

pub(crate) fn parse_status(
    source: StructuredSource,
    options: &GitRunOptions,
    codec: &CursorCodec,
    scope: &str,
) -> RuntimeResult<GitStatusData> {
    let start = page_start(options, codec, "git_status", scope)?;
    let limit = structured_limit(options);
    let mut branch = GitBranchMetadata::default();
    let mut entries = Vec::with_capacity(limit.min(256));
    let mut entry_index = 0usize;
    let mut has_more = false;
    let mut reader = source.reader()?;
    let mut line = Vec::with_capacity(512);
    while reader.read_until(b'\n', &mut line).map_err(io_error)? != 0 {
        trim_line_endings(&mut line);
        if line.starts_with(b"# ") {
            parse_branch_header(&line, &mut branch);
        } else if let Some(entry) = parse_status_entry(&line)? {
            if entry_index >= start {
                if entries.len() == limit {
                    has_more = true;
                    break;
                }
                entries.push(entry);
            }
            entry_index = entry_index.saturating_add(1);
        }
        line.clear();
    }
    let next_cursor = next_cursor(
        codec,
        "git_status",
        scope,
        start.saturating_add(entries.len()),
        has_more,
    )?;
    Ok(GitStatusData {
        branch,
        entries,
        next_cursor,
        has_more,
    })
}

fn parse_branch_header(line: &[u8], branch: &mut GitBranchMetadata) {
    let text = String::from_utf8_lossy(line);
    if let Some(value) = text.strip_prefix("# branch.oid ") {
        branch.oid = (value != "(initial)").then(|| value.to_owned());
    } else if let Some(value) = text.strip_prefix("# branch.head ") {
        branch.head = (value != "(detached)").then(|| value.to_owned());
    } else if let Some(value) = text.strip_prefix("# branch.upstream ") {
        branch.upstream = Some(value.to_owned());
    } else if let Some(value) = text.strip_prefix("# branch.ab ") {
        for part in value.split_whitespace() {
            if let Some(value) = part.strip_prefix('+') {
                branch.ahead = value.parse().unwrap_or(0);
            } else if let Some(value) = part.strip_prefix('-') {
                branch.behind = value.parse().unwrap_or(0);
            }
        }
    }
}

fn parse_status_entry(line: &[u8]) -> RuntimeResult<Option<GitStatusEntry>> {
    if line.starts_with(b"? ") || line.starts_with(b"! ") {
        let kind = if line[0] == b'?' {
            "untracked"
        } else {
            "ignored"
        };
        return Ok(Some(GitStatusEntry {
            kind: kind.to_owned(),
            path: parse_git_path(&line[2..])?,
            original_path: None,
            index_status: if line[0] == b'?' { "?" } else { "!" }.to_owned(),
            worktree_status: if line[0] == b'?' { "?" } else { "!" }.to_owned(),
            score: None,
        }));
    }
    if line.starts_with(b"1 ") {
        let fields = split_ascii_fields(line, 9);
        if fields.len() < 9 {
            return Err(parse_error("malformed porcelain v2 ordinary entry"));
        }
        let xy = fields[1];
        return Ok(Some(GitStatusEntry {
            kind: "ordinary".to_owned(),
            path: parse_git_path(fields[8])?,
            original_path: None,
            index_status: status_byte(xy, 0),
            worktree_status: status_byte(xy, 1),
            score: None,
        }));
    }
    if line.starts_with(b"2 ") {
        let fields = split_ascii_fields(line, 10);
        if fields.len() < 10 {
            return Err(parse_error("malformed porcelain v2 rename/copy entry"));
        }
        let xy = fields[1];
        let (path, original_path) = split_rename_paths(fields[9])?;
        return Ok(Some(GitStatusEntry {
            kind: if fields[8].first() == Some(&b'R') {
                "rename"
            } else {
                "copy"
            }
            .to_owned(),
            path,
            original_path: Some(original_path),
            index_status: status_byte(xy, 0),
            worktree_status: status_byte(xy, 1),
            score: Some(String::from_utf8_lossy(fields[8]).into_owned()),
        }));
    }
    if line.starts_with(b"u ") {
        let fields = split_ascii_fields(line, 11);
        if fields.len() < 11 {
            return Err(parse_error("malformed porcelain v2 unmerged entry"));
        }
        let xy = fields[1];
        return Ok(Some(GitStatusEntry {
            kind: "unmerged".to_owned(),
            path: parse_git_path(fields[10])?,
            original_path: None,
            index_status: status_byte(xy, 0),
            worktree_status: status_byte(xy, 1),
            score: None,
        }));
    }
    Ok(None)
}

fn split_ascii_fields(line: &[u8], max_fields: usize) -> Vec<&[u8]> {
    let mut fields = Vec::with_capacity(max_fields);
    let mut start = 0usize;
    while fields.len() + 1 < max_fields {
        let Some(relative) = line[start..].iter().position(|byte| *byte == b' ') else {
            break;
        };
        let end = start + relative;
        fields.push(&line[start..end]);
        start = end.saturating_add(1);
    }
    fields.push(&line[start..]);
    fields
}

fn split_rename_paths(value: &[u8]) -> RuntimeResult<(GitPathValue, GitPathValue)> {
    let Some(tab) = value.iter().position(|byte| *byte == b'\t') else {
        return Err(parse_error("rename/copy entry is missing original path"));
    };
    Ok((
        parse_git_path(&value[..tab])?,
        parse_git_path(&value[tab + 1..])?,
    ))
}

fn status_byte(value: &[u8], index: usize) -> String {
    value
        .get(index)
        .copied()
        .map(char::from)
        .unwrap_or('.')
        .to_string()
}

fn parse_git_path(value: &[u8]) -> RuntimeResult<GitPathValue> {
    let bytes = if value.first() == Some(&b'"') && value.last() == Some(&b'"') && value.len() >= 2 {
        decode_git_quoted_path(&value[1..value.len() - 1])?
    } else {
        value.to_vec()
    };
    match String::from_utf8(bytes.clone()) {
        Ok(display) => Ok(GitPathValue {
            display,
            path_bytes_base64: None,
        }),
        Err(_) => Ok(GitPathValue {
            display: String::from_utf8_lossy(&bytes).into_owned(),
            path_bytes_base64: Some(STANDARD.encode(bytes)),
        }),
    }
}

fn decode_git_quoted_path(value: &[u8]) -> RuntimeResult<Vec<u8>> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0usize;
    while index < value.len() {
        if value[index] != b'\\' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        index += 1;
        let Some(&escaped) = value.get(index) else {
            return Err(parse_error("unterminated git path escape"));
        };
        match escaped {
            b'"' | b'\\' => decoded.push(escaped),
            b't' => decoded.push(b'\t'),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b'b' => decoded.push(8),
            b'f' => decoded.push(12),
            b'v' => decoded.push(11),
            b'a' => decoded.push(7),
            b'0'..=b'7' => {
                let mut octal = u16::from(escaped - b'0');
                let mut consumed = 1usize;
                while consumed < 3 {
                    let Some(next @ b'0'..=b'7') = value.get(index + consumed).copied() else {
                        break;
                    };
                    octal = octal
                        .saturating_mul(8)
                        .saturating_add(u16::from(next - b'0'));
                    consumed += 1;
                }
                decoded.push(
                    u8::try_from(octal).map_err(|_| parse_error("invalid octal path escape"))?,
                );
                index += consumed - 1;
            }
            other => decoded.push(other),
        }
        index += 1;
    }
    Ok(decoded)
}

pub(crate) fn parse_log(
    source: StructuredSource,
    options: &GitRunOptions,
    codec: &CursorCodec,
    scope: &str,
) -> RuntimeResult<GitLogData> {
    let start = page_start(options, codec, "git_log", scope)?;
    let limit = structured_limit(options);
    let mut reader = source.reader()?;
    let mut record = Vec::with_capacity(512);
    let mut index = 0usize;
    let mut entries = Vec::with_capacity(limit.min(256));
    let mut has_more = false;
    while reader.read_until(0x1e, &mut record).map_err(io_error)? != 0 {
        if record.last() == Some(&0x1e) {
            record.pop();
        }
        while matches!(record.last(), Some(b'\n' | b'\r')) {
            record.pop();
        }
        if !record.is_empty() {
            if index >= start {
                if entries.len() == limit {
                    has_more = true;
                    break;
                }
                let fields = record.split(|byte| *byte == 0x1f).collect::<Vec<_>>();
                if fields.len() != 5 {
                    return Err(parse_error("malformed git log record"));
                }
                entries.push(GitLogEntry {
                    commit: String::from_utf8_lossy(fields[0]).into_owned(),
                    short_commit: String::from_utf8_lossy(fields[1]).into_owned(),
                    author: String::from_utf8_lossy(fields[2]).into_owned(),
                    authored_at: String::from_utf8_lossy(fields[3]).into_owned(),
                    subject: String::from_utf8_lossy(fields[4]).into_owned(),
                });
            }
            index = index.saturating_add(1);
        }
        record.clear();
    }
    let next_cursor = next_cursor(
        codec,
        "git_log",
        scope,
        start.saturating_add(entries.len()),
        has_more,
    )?;
    Ok(GitLogData {
        entries,
        next_cursor,
        has_more,
    })
}

pub(crate) fn parse_branches(
    source: StructuredSource,
    options: &GitRunOptions,
    codec: &CursorCodec,
    scope: &str,
) -> RuntimeResult<GitBranchData> {
    let start = page_start(options, codec, "git_branch", scope)?;
    let limit = structured_limit(options);
    let mut reader = source.reader()?;
    let mut line = String::new();
    let mut index = 0usize;
    let mut entries = Vec::with_capacity(limit.min(256));
    let mut has_more = false;
    while reader.read_line(&mut line).map_err(io_error)? != 0 {
        let text = line.trim_end_matches(['\r', '\n']);
        if !text.is_empty() {
            if index >= start {
                if entries.len() == limit {
                    has_more = true;
                    break;
                }
                let mut fields = text.splitn(4, '\t');
                let name = fields.next().unwrap_or_default().to_owned();
                let object_id = fields.next().unwrap_or_default().to_owned();
                let current = fields.next().unwrap_or_default() == "*";
                let upstream = fields
                    .next()
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                if name.is_empty() || object_id.is_empty() {
                    return Err(parse_error("malformed git branch record"));
                }
                entries.push(GitBranchEntry {
                    name,
                    object_id,
                    current,
                    upstream,
                });
            }
            index = index.saturating_add(1);
        }
        line.clear();
    }
    let next_cursor = next_cursor(
        codec,
        "git_branch",
        scope,
        start.saturating_add(entries.len()),
        has_more,
    )?;
    Ok(GitBranchData {
        entries,
        next_cursor,
        has_more,
    })
}

fn trim_line_endings(value: &mut Vec<u8>) {
    while matches!(value.last(), Some(b'\r' | b'\n')) {
        value.pop();
    }
}

fn parse_error(message: &str) -> RuntimeError {
    RuntimeError::new("git_parse_failed", message)
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("process_start_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parser_handles_branch_rename_and_non_utf8_path() {
        let source = StructuredSource::Inline(
            b"# branch.oid 0123456789012345678901234567890123456789\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n2 R. N... 100644 100644 100644 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb R100 new.txt\told.txt\n? \"bad-\\377.txt\"\n".to_vec(),
        );
        let codec = CursorCodec::from_key([7; 32]);
        let data = parse_status(source, &GitRunOptions::default(), &codec, "repo").expect("status");
        assert_eq!(data.branch.head.as_deref(), Some("main"));
        assert_eq!(data.branch.ahead, 2);
        assert_eq!(data.branch.behind, 1);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.entries[0].kind, "rename");
        assert_eq!(
            data.entries[0]
                .original_path
                .as_ref()
                .map(|p| p.display.as_str()),
            Some("old.txt")
        );
        assert!(data.entries[1].path.path_bytes_base64.is_some());
    }

    #[test]
    fn structured_cursor_is_signed_and_resumes_without_duplicates() {
        let source = StructuredSource::Inline(b"? one\n? two\n? three\n".to_vec());
        let codec = CursorCodec::from_key([9; 32]);
        let mut options = GitRunOptions {
            limit: 2,
            ..GitRunOptions::default()
        };
        let first = parse_status(source.clone(), &options, &codec, "repo").expect("first page");
        assert!(first.has_more);
        assert_eq!(first.entries.len(), 2);
        options.cursor = first.next_cursor.clone();
        let second = parse_status(source, &options, &codec, "repo").expect("second page");
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].path.display, "three");
        assert!(!second.has_more);
    }

    #[test]
    fn log_and_branch_parsers_are_machine_readable() {
        let codec = CursorCodec::from_key([3; 32]);
        let log = parse_log(
            StructuredSource::Inline(
                b"abc\x1fabc\x1fAda\x1f2026-09-04T10:00:00Z\x1fsubject\x1e\n".to_vec(),
            ),
            &GitRunOptions::default(),
            &codec,
            "repo-log",
        )
        .expect("log");
        assert_eq!(log.entries[0].author, "Ada");
        let branches = parse_branches(
            StructuredSource::Inline(b"refs/heads/main\tabc\t*\torigin/main\n".to_vec()),
            &GitRunOptions::default(),
            &codec,
            "repo-branch",
        )
        .expect("branches");
        assert!(branches.entries[0].current);
        assert_eq!(branches.entries[0].upstream.as_deref(), Some("origin/main"));
    }
}
