use super::*;
use crate::{AtomicWriteOptions, DurabilityMode, MetadataPolicy};
#[cfg(unix)]
use std::fs::File;
#[cfg(windows)]
use std::path::PathBuf;
use std::{
    fs,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};

pub(super) const WRITE_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(super) struct CommitOutcome {
    pub created: bool,
    pub bytes_written: u64,
    pub metadata_preserved: bool,
    pub durability_achieved: DurabilityMode,
    pub warnings: Vec<String>,
}

pub(super) fn write_reader<R: Read>(
    workspace: &WorkspaceService,
    target: &CreationWorkspacePath,
    existing: Option<&ExistingWorkspacePath>,
    reader: R,
    options: &AtomicWriteOptions,
    context: &OperationContext,
    require_utf8: bool,
) -> RuntimeResult<CommitOutcome> {
    let mut reader = BufReader::with_capacity(WRITE_BUFFER_BYTES, reader);
    let mut temporary =
        tempfile::NamedTempFile::new_in(&target.canonical_parent).map_err(io_error)?;
    let mut bytes_written = 0_u64;
    let mut utf8_tail = Vec::with_capacity(4);
    {
        let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, temporary.as_file_mut());
        let mut buffer = [0_u8; WRITE_BUFFER_BYTES];
        loop {
            if context.cancellation.is_cancelled() {
                return Err(RuntimeError::new(
                    "operationCancelled",
                    "write cancelled before commit",
                ));
            }
            let count = reader.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            if require_utf8 {
                validate_utf8_chunk(&mut utf8_tail, &buffer[..count])?;
            }
            writer.write_all(&buffer[..count]).map_err(io_error)?;
            bytes_written = bytes_written.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        }
        if require_utf8 && !utf8_tail.is_empty() {
            return Err(RuntimeError::new(
                "invalid_utf8",
                "text ends with an incomplete UTF-8 sequence",
            ));
        }
        writer.flush().map_err(io_error)?;
    }

    let metadata_preserved = if options.metadata_policy == MetadataPolicy::Preserve {
        if let Some(existing) = existing {
            let permissions = fs::symlink_metadata(existing.as_ref())
                .map_err(io_error)?
                .permissions();
            fs::set_permissions(temporary.path(), permissions).map_err(io_error)?;
            true
        } else {
            false
        }
    } else {
        false
    };
    if matches!(
        options.durability,
        DurabilityMode::Data | DurabilityMode::Full
    ) {
        temporary.as_file().sync_all().map_err(io_error)?;
    }

    target.revalidate_parent()?;
    let target_path = target.path();
    if let Some(expected) = options.expected_version.as_deref() {
        workspace.verify_expected_version_blocking(
            &target_path,
            expected,
            &context.cancellation,
            crate::FsStatBudget::default(),
        )?;
    }
    let existed = existing.is_some();
    match existing {
        Some(existing) => {
            existing.revalidate()?;
            if !options.overwrite {
                return Err(RuntimeError::new(
                    "already_exists",
                    "destination exists and overwrite is false",
                ));
            }
            atomic_replace(temporary, &target_path, options.durability)?;
        }
        None => {
            if target_path.exists() {
                return Err(RuntimeError::new(
                    "writeConflict",
                    "destination appeared before commit",
                ));
            }
            temporary.persist_noclobber(&target_path).map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    RuntimeError::new("writeConflict", "destination appeared before commit")
                } else {
                    io_error(error.error)
                }
            })?;
        }
    }

    let mut warnings = Vec::new();
    let durability_achieved = if options.durability == DurabilityMode::Full {
        sync_parent(&target.canonical_parent, &mut warnings)?
    } else {
        options.durability
    };
    Ok(CommitOutcome {
        created: !existed,
        bytes_written,
        metadata_preserved,
        durability_achieved,
        warnings,
    })
}

fn validate_utf8_chunk(tail: &mut Vec<u8>, chunk: &[u8]) -> RuntimeResult<()> {
    tail.extend_from_slice(chunk);
    match std::str::from_utf8(tail) {
        Ok(_) => tail.clear(),
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if tail.len().saturating_sub(valid) > 3 {
                return Err(RuntimeError::new("invalid_utf8", "text is not valid UTF-8"));
            }
            tail.drain(..valid);
        }
        Err(_) => return Err(RuntimeError::new("invalid_utf8", "text is not valid UTF-8")),
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn atomic_replace(
    temporary: tempfile::NamedTempFile,
    target: &Path,
    _durability: DurabilityMode,
) -> RuntimeResult<()> {
    temporary
        .persist(target)
        .map_err(|error| io_error(error.error))?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn atomic_replace(
    temporary: tempfile::NamedTempFile,
    target: &Path,
    durability: DurabilityMode,
) -> RuntimeResult<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let temp_path: PathBuf = temporary
        .into_temp_path()
        .keep()
        .map_err(|error| io_error(error.error))?;
    let source: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut flags = MOVEFILE_REPLACE_EXISTING;
    if durability != DurabilityMode::None {
        flags |= MOVEFILE_WRITE_THROUGH;
    }
    // SAFETY: both buffers are valid, NUL-terminated UTF-16 strings for the duration of the call.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
        let error = std::io::Error::last_os_error();
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn sync_parent(
    parent: &Path,
    _warnings: &mut Vec<String>,
) -> RuntimeResult<DurabilityMode> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;
    Ok(DurabilityMode::Full)
}

#[cfg(not(unix))]
pub(super) fn sync_parent(
    _parent: &Path,
    warnings: &mut Vec<String>,
) -> RuntimeResult<DurabilityMode> {
    warnings.push(
        "parent directory sync is unavailable on this platform; file data was synchronized"
            .to_owned(),
    );
    Ok(DurabilityMode::Data)
}
