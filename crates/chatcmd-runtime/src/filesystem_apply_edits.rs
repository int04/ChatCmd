use super::*;
use crate::{
    ApplyEditsRequest, ApplyEditsResult, EditColumnEncoding, EditCoordinateSystem, FsStatBudget,
    FsStatRequest, TextPosition,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    time::{Duration, Instant},
};

const CHUNK_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 8 * 1024;

#[derive(Debug)]
struct ResolvedEdit {
    start: u64,
    end: u64,
    text: Vec<u8>,
}

impl WorkspaceService {
    /// Applies versioned UTF-8 range edits through a same-directory temporary file.
    pub async fn apply_edits(
        &self,
        context: &OperationContext,
        request: &ApplyEditsRequest,
    ) -> RuntimeResult<ApplyEditsResult> {
        const HARD_EDIT_TIMEOUT_MS: u64 = 5 * 60_000;
        const HARD_EDIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
        const HARD_EDIT_COUNT: usize = 10_000;
        let mut effective_request = request.clone();
        effective_request.budget.timeout_ms = effective_request
            .budget
            .timeout_ms
            .min(HARD_EDIT_TIMEOUT_MS);
        effective_request.budget.max_bytes_read =
            effective_request.budget.max_bytes_read.min(HARD_EDIT_BYTES);
        effective_request.budget.max_bytes_written = effective_request
            .budget
            .max_bytes_written
            .min(HARD_EDIT_BYTES);
        effective_request.budget.max_edits =
            effective_request.budget.max_edits.min(HARD_EDIT_COUNT);
        let request = &effective_request;
        validate_request(request)?;
        let resolved = self.existing_for(&request.path, PathAccess::Replace)?;
        if resolved.kind != EntryKind::File {
            return Err(RuntimeError::new(
                "notAFile",
                "fs_apply_edits requires a regular file",
            ));
        }
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: context.tool_name.clone(),
                root: Some(resolved.root.clone()),
                destructive: true,
            })
            .await?;
        self.verify_expected_version_with_budget(
            &resolved,
            &request.expected_version,
            Some(context),
            FsStatBudget {
                timeout_ms: request.budget.timeout_ms,
                max_bytes_read: request.budget.max_bytes_read,
            },
        )
        .await?;
        let target = resolved.canonical_path.clone();
        let target_for_stat = target.clone();
        let workspace = self.clone();
        let owned_request = request.clone();
        let owned_context = context.clone();
        let result = tokio::task::spawn_blocking(move || {
            apply_blocking(
                &workspace,
                &target,
                &owned_request,
                &owned_context,
                &resolved,
            )
        })
        .await
        .map_err(join_error)??;

        if result.dry_run {
            return Ok(result);
        }
        let stat_request = FsStatRequest {
            path: target_for_stat,
            version_strength: crate::VersionStrength::Metadata,
            hash_algorithm: None,
            budget: FsStatBudget {
                timeout_ms: request.budget.timeout_ms,
                max_bytes_read: request.budget.max_bytes_read,
            },
        };
        // The commit point has passed, so caller cancellation must not turn a
        // successful mutation into an ambiguous cancellation error.
        let captured = self.stat_v2(None, &stat_request).await?;
        Ok(ApplyEditsResult {
            new_version: captured.version_token,
            ..result
        })
    }
}

fn validate_request(request: &ApplyEditsRequest) -> RuntimeResult<()> {
    if request.expected_version.is_empty() {
        return Err(RuntimeError::new(
            "expectedVersionRequired",
            "expectedVersion is required",
        ));
    }
    if request.edits.is_empty() {
        return Err(RuntimeError::new(
            "editsEmpty",
            "edits must contain at least one edit",
        ));
    }
    if request.budget.max_edits == 0 || request.edits.len() > request.budget.max_edits {
        return Err(RuntimeError::new(
            "editLimitExceeded",
            "edits exceed budget.maxEdits",
        ));
    }
    if request.budget.timeout_ms == 0 {
        return Err(RuntimeError::new(
            "invalidBudget",
            "budget.timeoutMs must be positive",
        ));
    }
    match request.coordinate_system {
        EditCoordinateSystem::Byte => {
            if request.column_encoding.is_some()
                || request.edits.iter().any(|edit| {
                    edit.start_byte.is_none()
                        || edit.end_byte.is_none()
                        || edit.start.is_some()
                        || edit.end.is_some()
                })
            {
                return Err(RuntimeError::new(
                    "invalidEditCoordinates",
                    "byte edits require only startByte and endByte",
                ));
            }
        }
        EditCoordinateSystem::LineColumn => {
            if request.column_encoding != Some(EditColumnEncoding::Utf8CodePoint)
                || request.edits.iter().any(|edit| {
                    edit.start.is_none()
                        || edit.end.is_none()
                        || edit.start_byte.is_some()
                        || edit.end_byte.is_some()
                })
            {
                return Err(RuntimeError::new(
                    "invalidEditCoordinates",
                    "lineColumn edits require start/end and columnEncoding=utf8CodePoint",
                ));
            }
        }
    }
    Ok(())
}

fn apply_blocking(
    workspace: &WorkspaceService,
    target: &Path,
    request: &ApplyEditsRequest,
    context: &OperationContext,
    authorized: &ExistingWorkspacePath,
) -> RuntimeResult<ApplyEditsResult> {
    let started = Instant::now();
    check_cancelled(context)?;
    let metadata = fs::symlink_metadata(target).map_err(io_error)?;
    let size = metadata.len();
    let required_reads = if request.dry_run {
        size
    } else {
        size.saturating_mul(2)
    };
    if required_reads > request.budget.max_bytes_read {
        return Err(RuntimeError::new(
            "readBudgetExceeded",
            "validation and streaming exceed budget.maxBytesRead",
        ));
    }
    let (mut edits, newline) = resolve_edits(target, request, size, started, context)?;
    edits.sort_by_key(|edit| (edit.start, edit.end));
    validate_ranges(&edits, size)?;
    let (additions, deletions) = edit_line_counts(&edits);
    let bytes_written = output_size(size, &edits)?;
    if bytes_written > request.budget.max_bytes_written {
        return Err(RuntimeError::new(
            "writeBudgetExceeded",
            "result exceeds budget.maxBytesWritten",
        ));
    }
    let preview = preview(&edits);
    if request.dry_run {
        authorized.revalidate()?;
        return Ok(ApplyEditsResult {
            path: target.to_path_buf(),
            applied: false,
            dry_run: true,
            old_version: request.expected_version.clone(),
            new_version: request.expected_version.clone(),
            edits_applied: edits.len(),
            bytes_read: size,
            bytes_written,
            additions,
            deletions,
            preview,
            diff_artifact_ref: None,
            commit_state: "notCommitted".to_owned(),
        });
    }

    check_deadline(started, request.budget.timeout_ms)?;
    let parent = target
        .parent()
        .ok_or_else(|| RuntimeError::new("invalid_path", "path has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    {
        let mut source =
            BufReader::with_capacity(CHUNK_BYTES, File::open(target).map_err(io_error)?);
        let mut output = BufWriter::with_capacity(CHUNK_BYTES, temporary.as_file_mut());
        stream_edits(
            &mut source,
            &mut output,
            &edits,
            newline.as_deref(),
            request,
            context,
            started,
        )?;
        output.flush().map_err(io_error)?;
    }
    temporary.as_file().sync_all().map_err(io_error)?;
    fs::set_permissions(temporary.path(), metadata.permissions()).map_err(io_error)?;
    check_cancelled(context)?;
    check_deadline(started, request.budget.timeout_ms)?;
    authorized.revalidate()?;
    workspace.verify_expected_version_blocking(
        target,
        &request.expected_version,
        &context.cancellation,
        FsStatBudget {
            timeout_ms: request.budget.timeout_ms,
            max_bytes_read: request.budget.max_bytes_read,
        },
    )?;
    atomic_writer::atomic_replace(temporary, target, crate::DurabilityMode::Full)?;
    let mut warnings = Vec::new();
    atomic_writer::sync_parent(parent, &mut warnings)?;
    Ok(ApplyEditsResult {
        path: target.to_path_buf(),
        applied: true,
        dry_run: false,
        old_version: request.expected_version.clone(),
        new_version: String::new(),
        edits_applied: edits.len(),
        bytes_read: required_reads,
        bytes_written,
        additions,
        deletions,
        preview,
        diff_artifact_ref: None,
        commit_state: "committed".to_owned(),
    })
}

fn resolve_edits(
    target: &Path,
    request: &ApplyEditsRequest,
    size: u64,
    started: Instant,
    context: &OperationContext,
) -> RuntimeResult<(Vec<ResolvedEdit>, Option<String>)> {
    match request.coordinate_system {
        EditCoordinateSystem::Byte => {
            let edits = request
                .edits
                .iter()
                .map(|edit| ResolvedEdit {
                    start: edit.start_byte.unwrap_or_default(),
                    end: edit.end_byte.unwrap_or_default(),
                    text: edit.text.as_bytes().to_vec(),
                })
                .collect::<Vec<_>>();
            validate_utf8_and_boundaries(target, size, edits, request, started, context)
        }
        EditCoordinateSystem::LineColumn => resolve_line_columns(target, request, started, context),
    }
}

fn validate_utf8_and_boundaries(
    target: &Path,
    size: u64,
    edits: Vec<ResolvedEdit>,
    request: &ApplyEditsRequest,
    started: Instant,
    context: &OperationContext,
) -> RuntimeResult<(Vec<ResolvedEdit>, Option<String>)> {
    let boundaries = edits
        .iter()
        .flat_map(|edit| [edit.start, edit.end])
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut reader = BufReader::with_capacity(CHUNK_BYTES, File::open(target).map_err(io_error)?);
    let mut scalar = Vec::with_capacity(4);
    let mut offset = 0_u64;
    let mut lf = 0_u64;
    let mut crlf = 0_u64;
    let mut previous_cr = false;
    let mut prefix = Vec::with_capacity(3);
    let mut chunk = [0_u8; CHUNK_BYTES];
    loop {
        check_cancelled(context)?;
        check_deadline(started, request.budget.timeout_ms)?;
        let count = reader.read(&mut chunk).map_err(io_error)?;
        if count == 0 {
            break;
        }
        for &byte in &chunk[..count] {
            if prefix.len() < 3 {
                prefix.push(byte);
            }
            if scalar.is_empty() && boundaries.contains(&offset) {
                seen.insert(offset);
            }
            if byte == b'\n' {
                if previous_cr {
                    crlf += 1;
                } else {
                    lf += 1;
                }
            }
            previous_cr = byte == b'\r';
            scalar.push(byte);
            match std::str::from_utf8(&scalar) {
                Ok(_) => scalar.clear(),
                Err(error) if error.error_len().is_none() && scalar.len() < 4 => {}
                Err(_) => {
                    return Err(RuntimeError::new(
                        "invalidUtf8",
                        "target is not valid UTF-8 text",
                    ));
                }
            }
            offset += 1;
        }
    }
    if boundaries.contains(&size) {
        seen.insert(size);
    }
    if !scalar.is_empty() {
        return Err(RuntimeError::new(
            "invalidUtf8",
            "target ends inside a UTF-8 code point",
        ));
    }
    if request.preserve_bom
        && prefix.as_slice() == [0xef, 0xbb, 0xbf]
        && edits.iter().any(|edit| edit.start < 3)
    {
        return Err(RuntimeError::new(
            "bomPreservationViolation",
            "edit would remove or precede the UTF-8 BOM while preserveBom is true",
        ));
    }
    if seen != boundaries {
        return Err(RuntimeError::new(
            "invalidUtf8Boundary",
            "byte range splits a UTF-8 code point or is outside the file",
        ));
    }
    let newline = dominant_newline(lf, crlf);
    let edits = edits
        .into_iter()
        .map(|edit| ResolvedEdit {
            start: edit.start,
            end: edit.end,
            text: normalize_replacement(&edit.text, newline.as_deref(), request),
        })
        .collect();
    Ok((edits, newline))
}

fn resolve_line_columns(
    target: &Path,
    request: &ApplyEditsRequest,
    started: Instant,
    context: &OperationContext,
) -> RuntimeResult<(Vec<ResolvedEdit>, Option<String>)> {
    let positions = request
        .edits
        .iter()
        .flat_map(|edit| [edit.start.as_ref(), edit.end.as_ref()])
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    if positions
        .iter()
        .any(|position| position.line == 0 || position.column == 0)
    {
        return Err(RuntimeError::new(
            "invalidRange",
            "line and column are 1-based",
        ));
    }
    let mut found = BTreeMap::new();
    let mut reader = BufReader::with_capacity(CHUNK_BYTES, File::open(target).map_err(io_error)?);
    let mut scalar = Vec::with_capacity(4);
    let mut offset = 0_u64;
    let mut line = 1_usize;
    let mut column = 1_usize;
    let mut lf = 0_u64;
    let mut crlf = 0_u64;
    let mut previous_cr = false;
    let mut chunk = [0_u8; CHUNK_BYTES];
    loop {
        check_cancelled(context)?;
        check_deadline(started, request.budget.timeout_ms)?;
        let count = reader.read(&mut chunk).map_err(io_error)?;
        if count == 0 {
            break;
        }
        for &byte in &chunk[..count] {
            scalar.push(byte);
            match std::str::from_utf8(&scalar) {
                Ok(text) => {
                    let scalar_start = offset + 1 - u64::try_from(scalar.len()).unwrap_or(0);
                    let position = TextPosition { line, column };
                    let ch = text.chars().next().expect("decoded scalar is non-empty");
                    let is_initial_bom = scalar_start == 0 && ch == '\u{feff}';
                    if !is_initial_bom && positions.contains(&position) {
                        found.insert(position, scalar_start);
                    }
                    if is_initial_bom {
                        // BOM is encoding metadata, not the first text column.
                    } else if ch == '\n' {
                        if previous_cr {
                            crlf += 1;
                        } else {
                            lf += 1;
                        }
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    previous_cr = ch == '\r';
                    scalar.clear();
                }
                Err(error) if error.error_len().is_none() && scalar.len() < 4 => {}
                Err(_) => {
                    return Err(RuntimeError::new(
                        "invalidUtf8",
                        "target is not valid UTF-8 text",
                    ));
                }
            }
            offset += 1;
        }
    }
    if !scalar.is_empty() {
        return Err(RuntimeError::new(
            "invalidUtf8",
            "target ends inside a UTF-8 code point",
        ));
    }
    let final_position = TextPosition { line, column };
    if positions.contains(&final_position) {
        found.insert(final_position, offset);
    }
    if found.len() != positions.len() {
        return Err(RuntimeError::new(
            "invalidRange",
            "line/column is outside the file",
        ));
    }
    let newline = dominant_newline(lf, crlf);
    let edits = request
        .edits
        .iter()
        .map(|edit| ResolvedEdit {
            start: found[edit.start.as_ref().expect("validated start")],
            end: found[edit.end.as_ref().expect("validated end")],
            text: normalize_replacement(edit.text.as_bytes(), newline.as_deref(), request),
        })
        .collect();
    Ok((edits, newline))
}

fn normalize_replacement(
    bytes: &[u8],
    newline: Option<&str>,
    request: &ApplyEditsRequest,
) -> Vec<u8> {
    if !request.preserve_line_endings || newline != Some("\r\n") {
        return bytes.to_vec();
    }
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

fn dominant_newline(lf: u64, crlf: u64) -> Option<String> {
    if lf == 0 && crlf == 0 {
        None
    } else if crlf >= lf {
        Some("\r\n".to_owned())
    } else {
        Some("\n".to_owned())
    }
}

fn validate_ranges(edits: &[ResolvedEdit], size: u64) -> RuntimeResult<()> {
    let mut previous_end = 0;
    let mut first = true;
    for edit in edits {
        if edit.start > edit.end || edit.end > size {
            return Err(RuntimeError::new(
                "invalidRange",
                "edit range is outside the file or reversed",
            ));
        }
        if !first && edit.start < previous_end {
            return Err(RuntimeError::new(
                "overlappingEdits",
                "edit ranges must not overlap",
            ));
        }
        previous_end = edit.end;
        first = false;
    }
    Ok(())
}

fn stream_edits(
    source: &mut BufReader<File>,
    output: &mut BufWriter<&mut File>,
    edits: &[ResolvedEdit],
    _newline: Option<&str>,
    request: &ApplyEditsRequest,
    context: &OperationContext,
    started: Instant,
) -> RuntimeResult<()> {
    let mut cursor = 0_u64;
    for edit in edits {
        copy_exact(
            source,
            output,
            edit.start - cursor,
            context,
            started,
            request.budget.timeout_ms,
        )?;
        source
            .seek(SeekFrom::Current(
                i64::try_from(edit.end - edit.start)
                    .map_err(|_| RuntimeError::new("rangeTooLarge", "edit range is too large"))?,
            ))
            .map_err(io_error)?;
        output.write_all(&edit.text).map_err(io_error)?;
        cursor = edit.end;
    }
    let mut chunk = [0_u8; CHUNK_BYTES];
    loop {
        check_cancelled(context)?;
        check_deadline(started, request.budget.timeout_ms)?;
        let count = source.read(&mut chunk).map_err(io_error)?;
        if count == 0 {
            break;
        }
        output.write_all(&chunk[..count]).map_err(io_error)?;
    }
    Ok(())
}

fn copy_exact(
    source: &mut BufReader<File>,
    output: &mut BufWriter<&mut File>,
    mut remaining: u64,
    context: &OperationContext,
    started: Instant,
    timeout_ms: u64,
) -> RuntimeResult<()> {
    let mut chunk = [0_u8; CHUNK_BYTES];
    while remaining > 0 {
        check_cancelled(context)?;
        check_deadline(started, timeout_ms)?;
        let wanted = usize::try_from(remaining.min(CHUNK_BYTES as u64)).unwrap_or(CHUNK_BYTES);
        source.read_exact(&mut chunk[..wanted]).map_err(io_error)?;
        output.write_all(&chunk[..wanted]).map_err(io_error)?;
        remaining -= u64::try_from(wanted).unwrap_or(u64::MAX);
    }
    Ok(())
}

fn output_size(size: u64, edits: &[ResolvedEdit]) -> RuntimeResult<u64> {
    edits.iter().try_fold(size, |total, edit| {
        total
            .checked_sub(edit.end - edit.start)
            .and_then(|value| value.checked_add(u64::try_from(edit.text.len()).ok()?))
            .ok_or_else(|| RuntimeError::new("resultTooLarge", "result size overflows u64"))
    })
}

fn edit_line_counts(edits: &[ResolvedEdit]) -> (u64, u64) {
    edits.iter().fold((0, 0), |(additions, deletions), edit| {
        (
            additions
                + u64::try_from(edit.text.iter().filter(|&&byte| byte == b'\n').count())
                    .unwrap_or(u64::MAX),
            deletions + u64::from(edit.end > edit.start),
        )
    })
}

fn preview(edits: &[ResolvedEdit]) -> String {
    let mut result = String::new();
    for edit in edits {
        let line = format!(
            "@@ bytes {}..{} @@\n+{}\n",
            edit.start,
            edit.end,
            String::from_utf8_lossy(&edit.text)
        );
        if result.len() + line.len() > PREVIEW_BYTES {
            result.push_str("... preview truncated");
            break;
        }
        result.push_str(&line);
    }
    result
}

fn check_cancelled(context: &OperationContext) -> RuntimeResult<()> {
    if context.cancellation.is_cancelled() {
        Err(RuntimeError::new(
            "operationCancelled",
            "operation cancelled before commit",
        ))
    } else {
        Ok(())
    }
}

fn check_deadline(started: Instant, timeout_ms: u64) -> RuntimeResult<()> {
    if started.elapsed() > Duration::from_millis(timeout_ms) {
        Err(RuntimeError::new(
            "operationTimedOut",
            "fs_apply_edits exceeded budget.timeoutMs",
        ))
    } else {
        Ok(())
    }
}
