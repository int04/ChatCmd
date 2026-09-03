use super::*;
use crate::{AtomicWriteOptions, AtomicWriteResult, FsStatBudget, FsStatRequest, VersionStrength};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{fs, io::Cursor, path::Path};

impl WorkspaceService {
    pub async fn write_text(
        &self,
        context: &OperationContext,
        path: &Path,
        content: &str,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let result = self
            .write_text_atomic(
                context,
                path,
                content,
                AtomicWriteOptions {
                    overwrite,
                    ..AtomicWriteOptions::default()
                },
            )
            .await?;
        self.stat(&result.path).await
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
        let result = self
            .write_bytes_atomic(
                context,
                path,
                bytes,
                AtomicWriteOptions {
                    overwrite,
                    ..AtomicWriteOptions::default()
                },
                false,
            )
            .await?;
        self.stat(&result.path).await
    }

    pub async fn write_text_atomic(
        &self,
        context: &OperationContext,
        path: &Path,
        content: &str,
        options: AtomicWriteOptions,
    ) -> RuntimeResult<AtomicWriteResult> {
        self.write_bytes_atomic(context, path, content.as_bytes().to_vec(), options, true)
            .await
    }

    pub async fn write_raw_bytes_atomic(
        &self,
        context: &OperationContext,
        path: &Path,
        bytes: Vec<u8>,
        options: AtomicWriteOptions,
    ) -> RuntimeResult<AtomicWriteResult> {
        self.write_bytes_atomic(context, path, bytes, options, false)
            .await
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
        let result = self
            .write_blob_atomic(
                context,
                path,
                blob_path,
                AtomicWriteOptions {
                    overwrite,
                    ..AtomicWriteOptions::default()
                },
                require_utf8,
            )
            .await?;
        self.stat(&result.path).await
    }

    pub async fn write_blob_atomic(
        &self,
        context: &OperationContext,
        path: &Path,
        blob_path: &Path,
        options: AtomicWriteOptions,
        require_utf8: bool,
    ) -> RuntimeResult<AtomicWriteResult> {
        let source = blob_path.to_path_buf();
        self.write_atomic(context, path, options, require_utf8, move || {
            fs::File::open(source).map_err(io_error)
        })
        .await
    }

    async fn write_bytes_atomic(
        &self,
        context: &OperationContext,
        path: &Path,
        bytes: Vec<u8>,
        options: AtomicWriteOptions,
        require_utf8: bool,
    ) -> RuntimeResult<AtomicWriteResult> {
        self.write_atomic(context, path, options, require_utf8, move || {
            Ok(Cursor::new(bytes))
        })
        .await
    }

    async fn write_atomic<R, F>(
        &self,
        context: &OperationContext,
        path: &Path,
        options: AtomicWriteOptions,
        require_utf8: bool,
        source: F,
    ) -> RuntimeResult<AtomicWriteResult>
    where
        R: std::io::Read + Send + 'static,
        F: FnOnce() -> RuntimeResult<R> + Send + 'static,
    {
        let target = self.creation_for(
            path,
            if options.overwrite {
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
                destructive: target_path.exists() && options.overwrite,
            })
            .await?;
        let existing_target = if target_path.exists() {
            Some(self.existing_for(&target_path, PathAccess::Replace)?)
        } else {
            None
        };
        if options.expected_version.is_some() && existing_target.is_none() {
            return Err(RuntimeError::new(
                "targetMissing",
                "expectedVersion requires an existing target",
            ));
        }
        if let Some(expected) = options.expected_version.as_deref() {
            self.verify_expected_version(&target_path, expected, Some(context))
                .await?;
        }
        let old_version = if existing_target.is_some() {
            Some(
                self.stat_v2(
                    Some(context),
                    &FsStatRequest {
                        path: target_path.clone(),
                        version_strength: VersionStrength::Metadata,
                        hash_algorithm: None,
                        budget: FsStatBudget::default(),
                    },
                )
                .await?
                .version_token,
            )
        } else {
            None
        };
        let workspace = self.clone();
        let owned_context = context.clone();
        let requested = options.durability;
        let outcome = tokio::task::spawn_blocking(move || {
            let reader = source()?;
            atomic_writer::write_reader(
                &workspace,
                &target,
                existing_target.as_ref(),
                reader,
                &options,
                &owned_context,
                require_utf8,
            )
        })
        .await
        .map_err(join_error)??;
        let new_version = self
            .stat_v2(
                None,
                &FsStatRequest {
                    path: target_path.clone(),
                    version_strength: VersionStrength::Metadata,
                    hash_algorithm: None,
                    budget: FsStatBudget::default(),
                },
            )
            .await?
            .version_token;
        Ok(AtomicWriteResult {
            path: target_path,
            committed: true,
            created: outcome.created,
            atomic: true,
            durability_requested: requested,
            durability_achieved: outcome.durability_achieved,
            bytes_written: outcome.bytes_written,
            old_version,
            new_version,
            metadata_preserved: outcome.metadata_preserved,
            warnings: outcome.warnings,
        })
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
