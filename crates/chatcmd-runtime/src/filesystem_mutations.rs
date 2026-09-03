use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    fs,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

impl WorkspaceService {
    pub async fn write_text(
        &self,
        context: &OperationContext,
        path: &Path,
        content: &str,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        self.write_bytes(context, path, content.as_bytes(), overwrite)
            .await
    }

    pub async fn write_raw(
        &self,
        context: &OperationContext,
        path: &Path,
        base64: &str,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let bytes = STANDARD.decode(base64).map_err(|_| {
            RuntimeError::new("invalid_base64", "raw file content is not valid Base64")
        })?;
        self.write_bytes(context, path, &bytes, overwrite).await
    }

    /// Streams an already-authorized temporary blob into the destination's
    /// same-directory temporary file before committing it.
    pub async fn write_blob(
        &self,
        context: &OperationContext,
        path: &Path,
        blob_path: &Path,
        overwrite: bool,
        require_utf8: bool,
    ) -> RuntimeResult<FsEntry> {
        let target = self.creation_for(
            path,
            if overwrite {
                PathAccess::Replace
            } else {
                PathAccess::Create
            },
        )?;
        let target_path = target.path();
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: context.tool_name.clone(),
                root: Some(target.root.clone()),
                destructive: target_path.exists() && overwrite,
            })
            .await?;
        if target_path.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let existing_target = if target_path.exists() {
            Some(self.existing_for(&target_path, PathAccess::Replace)?)
        } else {
            None
        };
        let source = blob_path.to_path_buf();
        let parent = target.canonical_parent.clone();
        let target_clone = target_path.clone();
        tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
            stream_blob_to_temporary(&source, &parent, require_utf8, |temporary| {
                target.revalidate_parent()?;
                if overwrite && target_clone.exists() {
                    if let Some(existing) = existing_target.as_ref() {
                        existing.revalidate()?;
                    }
                    fs::remove_file(&target_clone).map_err(io_error)?;
                }
                temporary
                    .persist(&target_clone)
                    .map_err(|error| io_error(error.error))?;
                Ok(())
            })
        })
        .await
        .map_err(join_error)??;
        self.stat(&target_path).await
    }

    async fn write_bytes(
        &self,
        context: &OperationContext,
        path: &Path,
        bytes: &[u8],
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let target = self.creation_for(
            path,
            if overwrite {
                PathAccess::Replace
            } else {
                PathAccess::Create
            },
        )?;
        let target_path = target.path();
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: context.tool_name.clone(),
                root: Some(target.root.clone()),
                destructive: target_path.exists() && overwrite,
            })
            .await?;
        if target_path.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let existing_target = if target_path.exists() {
            Some(self.existing_for(&target_path, PathAccess::Replace)?)
        } else {
            None
        };
        let parent = target.canonical_parent.clone();
        let target_clone = target_path.clone();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            temporary.write_all(&bytes).map_err(io_error)?;
            temporary.flush().map_err(io_error)?;
            target.revalidate_parent()?;
            if overwrite && target_clone.exists() {
                if let Some(existing) = existing_target.as_ref() {
                    existing.revalidate()?;
                }
                fs::remove_file(&target_clone).map_err(io_error)?;
            }
            temporary
                .persist(&target_clone)
                .map_err(|error| io_error(error.error))?;
            Ok(())
        })
        .await
        .map_err(join_error)??;
        self.stat(&target_path).await
    }

    pub async fn create_directory(&self, path: &Path) -> RuntimeResult<FsEntry> {
        let target = self.creation(path)?;
        target.revalidate_parent()?;
        let target_path = target.path();
        tokio::fs::create_dir(&target_path)
            .await
            .map_err(io_error)?;
        self.stat(&target_path).await
    }

    pub async fn copy(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing_for(source, PathAccess::Read)?;
        let destination = self.creation_for(
            destination,
            if overwrite {
                PathAccess::Replace
            } else {
                PathAccess::Create
            },
        )?;
        let destination_path = destination.path();
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_copy".into(),
                root: Some(destination.root.clone()),
                destructive: overwrite,
            })
            .await?;
        if destination_path.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        if destination_path.exists() {
            self.existing_for(&destination_path, PathAccess::Replace)?
                .revalidate()?;
        }
        source.revalidate()?;
        destination.revalidate_parent()?;
        let source_clone = source.clone();
        let destination_clone = destination_path.clone();
        tokio::task::spawn_blocking(move || {
            source_clone.revalidate()?;
            destination.revalidate_parent()?;
            copy_recursive(&source_clone, &destination_clone, overwrite)
        })
        .await
        .map_err(join_error)??;
        self.stat(&destination_path).await
    }

    pub async fn move_path(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing_for(source, PathAccess::MoveSource)?;
        let destination = self.creation_for(destination, PathAccess::MoveDestination)?;
        if self
            .allowed_scopes
            .iter()
            .any(|scope| scope == source.as_ref())
        {
            return Err(RuntimeError::new(
                "root_path_rejected",
                "workspace and explicit grant roots cannot be moved",
            ));
        }
        let destination_path = destination.path();
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_move".into(),
                root: Some(source.root.clone()),
                destructive: true,
            })
            .await?;
        source.revalidate()?;
        destination.revalidate_parent()?;
        if destination_path.exists() {
            if !overwrite {
                return Err(RuntimeError::new(
                    "already_exists",
                    "destination exists and overwrite is false",
                ));
            }
            let existing_destination =
                self.existing_for(&destination_path, PathAccess::MoveDestination)?;
            existing_destination.revalidate()?;
            remove_recursive(&destination_path)?;
        }
        source.revalidate()?;
        destination.revalidate_parent()?;
        match tokio::fs::rename(&source, &destination_path).await {
            Ok(()) => {}
            Err(_) => {
                let source_clone = source.clone();
                let destination_clone = destination_path.clone();
                tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
                    source_clone.revalidate()?;
                    destination.revalidate_parent()?;
                    copy_recursive(&source_clone, &destination_clone, true)?;
                    remove_recursive(&source_clone)
                })
                .await
                .map_err(join_error)??;
            }
        }
        self.stat(&destination_path).await
    }

    pub async fn delete(
        &self,
        context: &OperationContext,
        path: &Path,
        recursive: bool,
    ) -> RuntimeResult<bool> {
        let resolved = self.existing_for(path, PathAccess::Delete)?;
        if self
            .allowed_scopes
            .iter()
            .any(|scope| scope == resolved.as_ref())
        {
            return Err(RuntimeError::new(
                "root_path_rejected",
                "workspace and explicit grant roots cannot be deleted",
            ));
        }
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_delete".into(),
                root: Some(resolved.root.clone()),
                destructive: true,
            })
            .await?;
        tokio::task::spawn_blocking(move || -> RuntimeResult<bool> {
            resolved.revalidate()?;
            if resolved.is_dir() {
                if recursive {
                    fs::remove_dir_all(resolved).map_err(io_error)?;
                } else {
                    fs::remove_dir(resolved).map_err(io_error)?;
                }
            } else {
                fs::remove_file(resolved).map_err(io_error)?;
            }
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }
}

fn stream_blob_to_temporary(
    source: &PathBuf,
    parent: &Path,
    require_utf8: bool,
    commit: impl FnOnce(tempfile::NamedTempFile) -> RuntimeResult<()>,
) -> RuntimeResult<()> {
    let input = fs::File::open(source).map_err(io_error)?;
    let mut reader = BufReader::with_capacity(64 * 1024, input);
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    let mut writer = BufWriter::with_capacity(64 * 1024, temporary.as_file_mut());
    let mut buffer = [0_u8; 64 * 1024];
    let mut utf8_tail = Vec::with_capacity(4);
    loop {
        let count = reader.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if require_utf8 {
            utf8_tail.extend_from_slice(&buffer[..count]);
            match std::str::from_utf8(&utf8_tail) {
                Ok(_) => utf8_tail.clear(),
                Err(error) if error.error_len().is_none() => {
                    let valid = error.valid_up_to();
                    if utf8_tail.len().saturating_sub(valid) > 3 {
                        return Err(RuntimeError::new(
                            "invalid_utf8",
                            "text blob is not valid UTF-8",
                        ));
                    }
                    utf8_tail.drain(..valid);
                }
                Err(_) => {
                    return Err(RuntimeError::new(
                        "invalid_utf8",
                        "text blob is not valid UTF-8",
                    ));
                }
            }
        }
        writer.write_all(&buffer[..count]).map_err(io_error)?;
    }
    if require_utf8 && !utf8_tail.is_empty() {
        return Err(RuntimeError::new(
            "invalid_utf8",
            "text blob ends with an incomplete UTF-8 sequence",
        ));
    }
    writer.flush().map_err(io_error)?;
    drop(writer);
    temporary.as_file().sync_all().map_err(io_error)?;
    commit(temporary)
}
