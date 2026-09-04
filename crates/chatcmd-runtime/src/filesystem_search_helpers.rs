use super::search::{CompiledSearch, PendingMatch};
use crate::{FsSearchMatch, FsSearchRequest, RuntimeError, RuntimeResult, SearchMode};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::RegexBuilder;
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const BINARY_SAMPLE_BYTES: usize = 8 * 1024;

pub(super) fn compile_search(request: &FsSearchRequest) -> RuntimeResult<CompiledSearch> {
    if request.query.is_empty() {
        return Err(RuntimeError::new(
            "invalid_search_query",
            "search query must not be empty",
        ));
    }
    if request.query.len() > 64 * 1024 {
        return Err(RuntimeError::new(
            "invalid_search_query",
            "search query is too large",
        ));
    }
    let pattern = match request.mode {
        SearchMode::Literal => regex::escape(&request.query),
        SearchMode::Regex => request.query.clone(),
    };
    let pattern = if request.word_boundary {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!request.case_sensitive)
        .size_limit(2 << 20)
        .dfa_size_limit(4 << 20)
        .build()
        .map_err(|error| RuntimeError::new("invalid_search_regex", error.to_string()))?;
    let includes =
        if request.include.is_empty() {
            None
        } else {
            let mut builder = GlobSetBuilder::new();
            for pattern in &request.include {
                builder.add(Glob::new(pattern).map_err(|error| {
                    RuntimeError::new("invalid_search_include", error.to_string())
                })?);
            }
            Some(
                builder.build().map_err(|error| {
                    RuntimeError::new("invalid_search_include", error.to_string())
                })?,
            )
        };
    Ok(CompiledSearch { regex, includes })
}

pub(super) fn include_matches(root: &Path, path: &Path, includes: Option<&GlobSet>) -> bool {
    let Some(includes) = includes else {
        return true;
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    includes.is_match(relative)
}

pub(super) fn open_text_file(path: &Path) -> std::io::Result<Option<File>> {
    let mut file = File::open(path)?;
    let mut sample = [0_u8; BINARY_SAMPLE_BYTES];
    let read = file.read(&mut sample)?;
    if sample[..read].contains(&0) {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(Some(file))
}

pub(super) fn append_after_context(pending: &mut [PendingMatch], line: &str) {
    for item in pending {
        if item.after_remaining > 0 {
            item.value.context_after.push(line.to_owned());
            item.after_remaining -= 1;
        }
    }
}

pub(super) fn flush_pending(
    pending: &mut Vec<PendingMatch>,
    ready: &mut VecDeque<FsSearchMatch>,
    force: bool,
) {
    let mut index = 0;
    while index < pending.len() {
        if !force && pending[index].after_remaining > 0 {
            index += 1;
            continue;
        }
        ready.push_back(pending.remove(index).value);
    }
}

pub(super) fn drain_ready(
    ready: &mut VecDeque<FsSearchMatch>,
    output: &mut Vec<FsSearchMatch>,
    output_bytes: &mut u64,
    max_output: u64,
    limit: usize,
) -> RuntimeResult<bool> {
    while output.len() < limit {
        let Some(item) = ready.front() else {
            return Ok(true);
        };
        let size = serde_json::to_vec(item)
            .map_err(|error| RuntimeError::new("result_serialization_failed", error.to_string()))?
            .len() as u64;
        if output_bytes.saturating_add(size) > max_output {
            return Ok(false);
        }
        *output_bytes += size;
        output.push(ready.pop_front().expect("front checked above"));
    }
    Ok(ready.is_empty())
}

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}
