use crate::{
    OperationContext, RuntimeError, RuntimeResult, TextReadRange, TextReadRangeResult,
    TextReadRequestV2, TextReadResultV2,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader},
};

const READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    size: u64,
    modified_ns: u128,
}

impl FileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            modified_ns: metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos(),
        }
    }

    fn token(&self, path: &Path) -> String {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        self.size.hash(&mut hasher);
        self.modified_ns.hash(&mut hasher);
        format!("v1-{:016x}", hasher.finish())
    }
}

pub(crate) async fn read_text_v2(
    resolved: PathBuf,
    context: Option<&OperationContext>,
    request: &TextReadRequestV2,
) -> RuntimeResult<TextReadResultV2> {
    validate_request(request)?;
    let before = tokio::fs::metadata(&resolved).await.map_err(io_error)?;
    if !before.is_file() {
        return Err(RuntimeError::new("not_file", "path is not a regular file"));
    }
    let identity = FileIdentity::from_metadata(&before);
    let version_token = identity.token(&resolved);
    if request
        .expected_version
        .as_deref()
        .is_some_and(|expected| expected != version_token)
    {
        return Err(RuntimeError::new(
            "version_mismatch",
            "expectedVersion does not match the current file version",
        ));
    }

    let started = Instant::now();
    let deadline = Duration::from_millis(request.budget.timeout_ms.max(1));
    let mut result = match request.range {
        TextReadRange::Line { start, limit } => {
            read_line_range(
                &resolved,
                context,
                request,
                start,
                limit,
                identity.size,
                started,
                deadline,
            )
            .await?
        }
        TextReadRange::Byte { start, limit } => {
            read_byte_range(&resolved, context, request, start, limit, started, deadline).await?
        }
    };

    let after = tokio::fs::metadata(&resolved).await.map_err(io_error)?;
    if FileIdentity::from_metadata(&after) != identity {
        return Err(RuntimeError::new(
            "file_changed_during_read",
            "file changed while the requested range was being read",
        ));
    }
    result.path = resolved;
    result.size_bytes = identity.size;
    result.version_token = version_token;
    Ok(result)
}

async fn read_line_range(
    path: &Path,
    context: Option<&OperationContext>,
    request: &TextReadRequestV2,
    start: usize,
    limit: usize,
    file_size: u64,
    started: Instant,
    deadline: Duration,
) -> RuntimeResult<TextReadResultV2> {
    let file = File::open(path).await.map_err(io_error)?;
    let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    let output_limit = request.max_bytes.max(1);
    let mut output = Vec::with_capacity(output_limit.min(READ_BUFFER_BYTES));
    let mut bytes_read = 0_u64;
    let mut byte_offset = 0_u64;
    let mut current_line = 1_usize;
    let mut emitted_lines = 0_usize;
    let mut last_emitted_line = None;
    let mut saw_any_byte = false;
    let mut last_was_terminator = false;
    let mut eof = false;
    let mut output_limited = false;
    let mut read_budget_limited = false;
    let mut pending_cr = false;
    let mut endings = NewlineStats::default();

    while emitted_lines < limit && !output_limited {
        check_budget(context, started, deadline)?;
        if bytes_read >= request.budget.max_bytes_read {
            read_budget_limited = true;
            break;
        }
        let buf = reader.fill_buf().await.map_err(io_error)?;
        if buf.is_empty() {
            eof = true;
            if pending_cr {
                endings.cr += 1;
                finish_line(
                    current_line,
                    start,
                    limit,
                    request.include_line_endings,
                    b'\r',
                    &mut output,
                    output_limit,
                    &mut output_limited,
                    &mut emitted_lines,
                    &mut last_emitted_line,
                );
                current_line += 1;
                last_was_terminator = true;
            }
            break;
        }
        let available_budget = request.budget.max_bytes_read.saturating_sub(bytes_read);
        let allowed = buf
            .len()
            .min(usize::try_from(available_budget).unwrap_or(usize::MAX));
        if allowed == 0 {
            read_budget_limited = true;
            break;
        }

        let mut consumed = 0_usize;
        for &byte in &buf[..allowed] {
            saw_any_byte = true;
            last_was_terminator = false;
            if pending_cr {
                if byte == b'\n' {
                    endings.crlf += 1;
                    finish_crlf_line(
                        current_line,
                        start,
                        limit,
                        request.include_line_endings,
                        &mut output,
                        output_limit,
                        &mut output_limited,
                        &mut emitted_lines,
                        &mut last_emitted_line,
                    );
                    current_line += 1;
                    pending_cr = false;
                    consumed += 1;
                    byte_offset += 1;
                    last_was_terminator = true;
                    if emitted_lines >= limit || output_limited {
                        break;
                    }
                    continue;
                }
                endings.cr += 1;
                finish_line(
                    current_line,
                    start,
                    limit,
                    request.include_line_endings,
                    b'\r',
                    &mut output,
                    output_limit,
                    &mut output_limited,
                    &mut emitted_lines,
                    &mut last_emitted_line,
                );
                current_line += 1;
                pending_cr = false;
                last_was_terminator = true;
                if emitted_lines >= limit || output_limited {
                    break;
                }
            }

            match byte {
                b'\r' => pending_cr = true,
                b'\n' => {
                    endings.lf += 1;
                    finish_line(
                        current_line,
                        start,
                        limit,
                        request.include_line_endings,
                        b'\n',
                        &mut output,
                        output_limit,
                        &mut output_limited,
                        &mut emitted_lines,
                        &mut last_emitted_line,
                    );
                    current_line += 1;
                    last_was_terminator = true;
                }
                _ if current_line >= start && emitted_lines < limit => {
                    push_bounded(&mut output, byte, output_limit, &mut output_limited);
                }
                _ => {}
            }
            consumed += 1;
            byte_offset += 1;
            if emitted_lines >= limit || output_limited {
                break;
            }
        }
        reader.consume(consumed);
        bytes_read += consumed as u64;
        if consumed < allowed {
            break;
        }
    }

    if eof
        && !pending_cr
        && saw_any_byte
        && !last_was_terminator
        && current_line >= start
        && emitted_lines < limit
    {
        emitted_lines += 1;
        last_emitted_line = Some(current_line);
    }

    let (bom, content) = decode_utf8(output, start == 1)?;
    let reached_physical_eof = eof || byte_offset >= file_size;
    let range_limited = !reached_physical_eof && emitted_lines >= limit;
    let truncated = output_limited || read_budget_limited || range_limited;
    let truncation_reason = if output_limited {
        Some("outputLimit".to_owned())
    } else if read_budget_limited {
        Some("readBudget".to_owned())
    } else if range_limited {
        Some("rangeLimit".to_owned())
    } else {
        None
    };
    let total_lines = if reached_physical_eof {
        Some(if saw_any_byte {
            if last_was_terminator {
                current_line.saturating_sub(1)
            } else {
                current_line
            }
        } else {
            0
        })
    } else {
        None
    };

    Ok(TextReadResultV2 {
        path: PathBuf::new(),
        content,
        range: TextReadRangeResult {
            start_line: Some(start),
            end_line: last_emitted_line,
            start_byte: None,
            end_byte: Some(byte_offset),
        },
        next_start_line: truncated
            .then_some(last_emitted_line.unwrap_or(start.saturating_sub(1)) + 1),
        next_byte_offset: truncated.then_some(byte_offset),
        truncated,
        truncation_reason,
        bytes_read,
        size_bytes: 0,
        version_token: String::new(),
        encoding: "utf-8".to_owned(),
        bom,
        line_ending: endings.kind(),
        line_ending_detection: if reached_physical_eof {
            "complete"
        } else {
            "sampled"
        }
        .to_owned(),
        total_lines,
        total_lines_known: reached_physical_eof,
    })
}

async fn read_byte_range(
    path: &Path,
    context: Option<&OperationContext>,
    request: &TextReadRequestV2,
    start: u64,
    limit: usize,
    started: Instant,
    deadline: Duration,
) -> RuntimeResult<TextReadResultV2> {
    check_budget(context, started, deadline)?;
    let mut file = File::open(path).await.map_err(io_error)?;
    let metadata = file.metadata().await.map_err(io_error)?;
    if start > metadata.len() {
        return Err(RuntimeError::new(
            "invalid_byte_range",
            "byte range start is beyond end of file",
        ));
    }
    let (actual_start, boundary_bytes_read) =
        validated_utf8_start(&mut file, start, metadata.len()).await?;
    check_budget(context, started, deadline)?;
    let output_cap = request.max_bytes.max(1).min(limit.max(1));
    let desired_read_cap = output_cap.saturating_add(3);
    let remaining_budget = request
        .budget
        .max_bytes_read
        .saturating_sub(boundary_bytes_read);
    let read_cap = desired_read_cap.min(usize::try_from(remaining_budget).unwrap_or(usize::MAX));
    file.seek(std::io::SeekFrom::Start(actual_start))
        .await
        .map_err(io_error)?;
    let mut bytes = vec![0_u8; read_cap];
    let read = file.read(&mut bytes).await.map_err(io_error)?;
    bytes.truncate(read);
    check_budget(context, started, deadline)?;

    let mut selected = bytes;
    if selected.len() > output_cap {
        selected.truncate(output_cap);
    }
    trim_incomplete_utf8_tail(&mut selected)?;
    let selected_len = selected.len();
    let (bom, content) = decode_utf8(selected, actual_start == 0)?;
    let end = actual_start.saturating_add(selected_len as u64);
    let truncated = end < metadata.len();
    let read_budget_limited = boundary_bytes_read.saturating_add(read as u64)
        >= request.budget.max_bytes_read
        && read_cap < desired_read_cap;
    let mut endings = NewlineStats::default();
    endings.observe(content.as_bytes());

    Ok(TextReadResultV2 {
        path: PathBuf::new(),
        content,
        range: TextReadRangeResult {
            start_line: None,
            end_line: None,
            start_byte: Some(actual_start),
            end_byte: Some(end),
        },
        next_start_line: None,
        next_byte_offset: truncated.then_some(end),
        truncated,
        truncation_reason: truncated.then(|| {
            if read_budget_limited {
                "readBudget".to_owned()
            } else {
                "rangeLimit".to_owned()
            }
        }),
        bytes_read: boundary_bytes_read.saturating_add(read as u64),
        size_bytes: 0,
        version_token: String::new(),
        encoding: "utf-8".to_owned(),
        bom,
        line_ending: endings.kind(),
        line_ending_detection: "sampled".to_owned(),
        total_lines: None,
        total_lines_known: false,
    })
}

async fn validated_utf8_start(
    file: &mut File,
    start: u64,
    file_size: u64,
) -> RuntimeResult<(u64, u64)> {
    if start == 0 || start == file_size {
        return Ok((start, 0));
    }

    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(io_error)?;
    let mut current = [0_u8; 1];
    let read = file.read(&mut current).await.map_err(io_error)?;
    if read == 0 || (current[0] & 0b1100_0000) != 0b1000_0000 {
        return Ok((start, read as u64));
    }

    let probe_start = start.saturating_sub(3);
    let probe_len = usize::try_from((start + 4).min(file_size) - probe_start).unwrap_or(7);
    file.seek(std::io::SeekFrom::Start(probe_start))
        .await
        .map_err(io_error)?;
    let mut probe = vec![0_u8; probe_len];
    let probe_read = file.read(&mut probe).await.map_err(io_error)?;
    probe.truncate(probe_read);
    let local_start = usize::try_from(start - probe_start).unwrap_or(3);
    let lead_index = (local_start.saturating_sub(3)..local_start)
        .rev()
        .find(|&index| (probe[index] & 0b1100_0000) != 0b1000_0000)
        .ok_or_else(|| {
            RuntimeError::new(
                "invalid_utf8",
                "byte range starts at an invalid UTF-8 continuation byte",
            )
        })?;
    let width = utf8_width(probe[lead_index]).ok_or_else(|| {
        RuntimeError::new(
            "invalid_utf8",
            "byte range starts inside an invalid UTF-8 sequence",
        )
    })?;
    let end_index = lead_index.saturating_add(width);
    if end_index > probe.len() || !(lead_index < local_start && local_start < end_index) {
        return Err(RuntimeError::new(
            "invalid_utf8",
            "byte range starts at an invalid UTF-8 continuation byte",
        ));
    }
    std::str::from_utf8(&probe[lead_index..end_index]).map_err(|_| {
        RuntimeError::new(
            "invalid_utf8",
            "byte range starts inside an invalid UTF-8 sequence",
        )
    })?;
    Ok((probe_start + end_index as u64, 1 + probe_read as u64))
}

fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_line(
    line: usize,
    start: usize,
    limit: usize,
    include_line_endings: bool,
    ending: u8,
    output: &mut Vec<u8>,
    output_limit: usize,
    output_limited: &mut bool,
    emitted_lines: &mut usize,
    last_emitted_line: &mut Option<usize>,
) {
    if line < start || *emitted_lines >= limit {
        return;
    }
    if include_line_endings {
        push_bounded(output, ending, output_limit, output_limited);
    } else if *emitted_lines + 1 < limit {
        push_bounded(output, b'\n', output_limit, output_limited);
    }
    *emitted_lines += 1;
    *last_emitted_line = Some(line);
}

#[allow(clippy::too_many_arguments)]
fn finish_crlf_line(
    line: usize,
    start: usize,
    limit: usize,
    include_line_endings: bool,
    output: &mut Vec<u8>,
    output_limit: usize,
    output_limited: &mut bool,
    emitted_lines: &mut usize,
    last_emitted_line: &mut Option<usize>,
) {
    if line < start || *emitted_lines >= limit {
        return;
    }
    if include_line_endings {
        push_bounded(output, b'\r', output_limit, output_limited);
        push_bounded(output, b'\n', output_limit, output_limited);
    } else if *emitted_lines + 1 < limit {
        push_bounded(output, b'\n', output_limit, output_limited);
    }
    *emitted_lines += 1;
    *last_emitted_line = Some(line);
}

fn validate_request(request: &TextReadRequestV2) -> RuntimeResult<()> {
    match request.range {
        TextReadRange::Line { start, limit } if start == 0 || limit == 0 => Err(RuntimeError::new(
            "invalid_line_range",
            "line range start and limit must both be at least 1",
        )),
        TextReadRange::Byte { limit, .. } if limit == 0 => Err(RuntimeError::new(
            "invalid_byte_range",
            "byte range limit must be at least 1",
        )),
        _ if request.max_bytes == 0 => Err(RuntimeError::new(
            "invalid_output_limit",
            "maxBytes must be at least 1",
        )),
        _ if request.budget.max_bytes_read == 0 => Err(RuntimeError::new(
            "invalid_read_budget",
            "budget.maxBytesRead must be at least 1",
        )),
        _ => Ok(()),
    }
}

fn check_budget(
    context: Option<&OperationContext>,
    started: Instant,
    deadline: Duration,
) -> RuntimeResult<()> {
    if context.is_some_and(|value| value.cancellation.is_cancelled()) {
        return Err(RuntimeError::new(
            "cancelled",
            "read operation was cancelled",
        ));
    }
    if started.elapsed() >= deadline {
        return Err(RuntimeError::new(
            "timeout",
            "read operation exceeded timeoutMs",
        ));
    }
    Ok(())
}

fn push_bounded(output: &mut Vec<u8>, byte: u8, limit: usize, limited: &mut bool) {
    if output.len() < limit {
        output.push(byte);
    } else {
        *limited = true;
    }
}

fn trim_incomplete_utf8_tail(bytes: &mut Vec<u8>) -> RuntimeResult<()> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(()),
        Err(error) if error.error_len().is_none() => {
            bytes.truncate(error.valid_up_to());
            Ok(())
        }
        Err(error) => Err(RuntimeError::new(
            "invalid_utf8",
            format!(
                "file contains invalid UTF-8 near byte offset {}",
                error.valid_up_to()
            ),
        )),
    }
}

fn decode_utf8(mut bytes: Vec<u8>, at_file_start: bool) -> RuntimeResult<(bool, String)> {
    trim_incomplete_utf8_tail(&mut bytes)?;
    let bom = at_file_start && bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bom {
        bytes.drain(..3);
    }
    String::from_utf8(bytes)
        .map(|content| (bom, content))
        .map_err(|error| {
            RuntimeError::new(
                "invalid_utf8",
                format!(
                    "file contains invalid UTF-8 near byte offset {}",
                    error.utf8_error().valid_up_to()
                ),
            )
        })
}

#[derive(Default)]
struct NewlineStats {
    lf: usize,
    crlf: usize,
    cr: usize,
}

impl NewlineStats {
    fn observe(&mut self, bytes: &[u8]) {
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                    self.crlf += 1;
                    index += 2;
                }
                b'\r' => {
                    self.cr += 1;
                    index += 1;
                }
                b'\n' => {
                    self.lf += 1;
                    index += 1;
                }
                _ => index += 1,
            }
        }
    }

    fn kind(&self) -> String {
        let kinds =
            usize::from(self.lf > 0) + usize::from(self.crlf > 0) + usize::from(self.cr > 0);
        match kinds {
            0 => "none",
            1 if self.crlf > 0 => "crlf",
            1 if self.cr > 0 => "cr",
            1 => "lf",
            _ => "mixed",
        }
        .to_owned()
    }
}

pub(crate) async fn legacy_text_total_lines(path: &Path) -> RuntimeResult<usize> {
    let mut file = File::open(path).await.map_err(io_error)?;
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    let mut carry = Vec::with_capacity(3);
    let mut saw_any = false;
    let mut last_was_lf = false;
    let mut lf_count = 0_usize;

    loop {
        let read = file.read(&mut buffer).await.map_err(io_error)?;
        if read == 0 {
            break;
        }
        saw_any = true;
        for &byte in &buffer[..read] {
            if byte == b'\n' {
                lf_count += 1;
            }
            last_was_lf = byte == b'\n';
        }

        let mut chunk = Vec::with_capacity(carry.len() + read);
        chunk.extend_from_slice(&carry);
        chunk.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&chunk) {
            Ok(_) => carry.clear(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                carry.clear();
                carry.extend_from_slice(&chunk[valid..]);
            }
            Err(error) => {
                return Err(RuntimeError::new(
                    "invalid_utf8",
                    format!(
                        "file contains invalid UTF-8 near byte offset {}",
                        error.valid_up_to()
                    ),
                ));
            }
        }
    }

    if !carry.is_empty() {
        return Err(RuntimeError::new(
            "invalid_utf8",
            "file ends with incomplete UTF-8",
        ));
    }
    Ok(if !saw_any {
        0
    } else if last_was_lf {
        lf_count
    } else {
        lf_count + 1
    })
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("io_error", error.to_string())
}
