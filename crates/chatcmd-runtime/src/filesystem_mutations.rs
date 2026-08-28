use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{fs, io::Write, path::Path};

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

    async fn write_bytes(
        &self,
        context: &OperationContext,
        path: &Path,
        bytes: &[u8],
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let target = self.creation(path)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: context.tool_name.clone(),
                root: self.containing_root(&target),
                destructive: target.exists() && overwrite,
            })
            .await?;
        if target.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let parent = target
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "destination has no parent"))?
            .to_path_buf();
        let target_clone = target.clone();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            temporary.write_all(&bytes).map_err(io_error)?;
            temporary.flush().map_err(io_error)?;
            if overwrite && target_clone.exists() {
                fs::remove_file(&target_clone).map_err(io_error)?;
            }
            temporary
                .persist(&target_clone)
                .map_err(|error| io_error(error.error))?;
            Ok(())
        })
        .await
        .map_err(join_error)??;
        self.stat(&target).await
    }

    pub async fn create_directory(&self, path: &Path) -> RuntimeResult<FsEntry> {
        let target = self.creation(path)?;
        tokio::fs::create_dir_all(&target).await.map_err(io_error)?;
        self.stat(&target).await
    }

    pub async fn copy(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing(source)?;
        let destination = self.creation(destination)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_copy".into(),
                root: self.containing_root(&destination),
                destructive: overwrite,
            })
            .await?;
        if destination.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let source_clone = source.clone();
        let destination_clone = destination.clone();
        tokio::task::spawn_blocking(move || {
            copy_recursive(&source_clone, &destination_clone, overwrite)
        })
        .await
        .map_err(join_error)??;
        self.stat(&destination).await
    }

    pub async fn move_path(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing(source)?;
        let destination = self.creation(destination)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_move".into(),
                root: self.containing_root(&source),
                destructive: true,
            })
            .await?;
        if destination.exists() {
            if !overwrite {
                return Err(RuntimeError::new(
                    "already_exists",
                    "destination exists and overwrite is false",
                ));
            }
            remove_recursive(&destination)?;
        }
        match tokio::fs::rename(&source, &destination).await {
            Ok(()) => {}
            Err(_) => {
                let source_clone = source.clone();
                let destination_clone = destination.clone();
                tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
                    copy_recursive(&source_clone, &destination_clone, true)?;
                    remove_recursive(&source_clone)
                })
                .await
                .map_err(join_error)??;
            }
        }
        self.stat(&destination).await
    }

    pub async fn delete(
        &self,
        context: &OperationContext,
        path: &Path,
        recursive: bool,
    ) -> RuntimeResult<bool> {
        let resolved = self.existing(path)?;
        if self.roots.contains(&resolved) {
            return Err(RuntimeError::new(
                "root_path_rejected",
                "configured workspace roots cannot be deleted",
            ));
        }
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_delete".into(),
                root: self.containing_root(&resolved),
                destructive: true,
            })
            .await?;
        tokio::task::spawn_blocking(move || -> RuntimeResult<bool> {
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
