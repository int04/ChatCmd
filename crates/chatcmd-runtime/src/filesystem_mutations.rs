use super::*;
use crate::{
    AtomicWriteOptions, AtomicWriteResult, FsConflictPolicy, FsDeleteMode, FsDeleteRequest,
    FsMutationBudget, FsMutationResult, FsQuarantineGcRequest, FsQuarantineGcResult,
    FsQuarantineRestoreRequest, FsStatBudget, FsStatRequest, FsTransferRequest, FsVerifyMode,
    VersionStrength,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{Cursor, Read as _, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
        let request = FsTransferRequest {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            conflict_policy: if overwrite {
                FsConflictPolicy::Replace
            } else {
                FsConflictPolicy::Error
            },
            atomic_publish: true,
            verify: FsVerifyMode::Metadata,
            preserve_metadata: true,
            follow_symlinks: false,
            dry_run: false,
            expected_source_version: None,
            expected_destination_version: None,
            budget: FsMutationBudget::default(),
        };
        let result = self.copy_safe(context, &request).await?;
        if result.state != "completed" && result.state != "skipped" {
            return Err(RuntimeError::new("mutation_incomplete", result.state));
        }
        self.stat(destination).await
    }

    pub async fn copy_safe(
        &self,
        context: &OperationContext,
        request: &FsTransferRequest,
    ) -> RuntimeResult<FsMutationResult> {
        self.transfer_safe(context, request, false).await
    }

    pub async fn move_path(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let request = FsTransferRequest {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            conflict_policy: if overwrite {
                FsConflictPolicy::Replace
            } else {
                FsConflictPolicy::Error
            },
            atomic_publish: true,
            verify: FsVerifyMode::Metadata,
            preserve_metadata: true,
            follow_symlinks: false,
            dry_run: false,
            expected_source_version: None,
            expected_destination_version: None,
            budget: FsMutationBudget::default(),
        };
        let result = self.move_safe(context, &request).await?;
        if result.state != "completed" && result.state != "completedWithSourceRemaining" {
            return Err(RuntimeError::new("mutation_incomplete", result.state));
        }
        self.stat(destination).await
    }

    pub async fn move_safe(
        &self,
        context: &OperationContext,
        request: &FsTransferRequest,
    ) -> RuntimeResult<FsMutationResult> {
        self.transfer_safe(context, request, true).await
    }

    async fn transfer_safe(
        &self,
        context: &OperationContext,
        request: &FsTransferRequest,
        remove_source: bool,
    ) -> RuntimeResult<FsMutationResult> {
        let mut effective_request = request.clone();
        effective_request.budget.timeout_ms = effective_request.budget.timeout_ms.min(30 * 60_000);
        effective_request.budget.max_files = effective_request.budget.max_files.min(2_000_000);
        effective_request.budget.max_bytes_read = effective_request
            .budget
            .max_bytes_read
            .min(2 * 1024 * 1024 * 1024 * 1024);
        effective_request.budget.max_bytes_written = effective_request
            .budget
            .max_bytes_written
            .min(2 * 1024 * 1024 * 1024 * 1024);
        effective_request.budget.max_open_files =
            effective_request.budget.max_open_files.clamp(1, 64);
        let request = &effective_request;
        let memory = request.budget.max_bytes_written.min(128 * 1024 * 1024);
        let _admission = self.admission.try_admit(&context.agent_id, 2, memory)?;
        if request.budget.max_open_files < 2 {
            return Err(RuntimeError::new(
                "openFileBudgetExceeded",
                "copy/move requires at least two open-file slots",
            ));
        }
        let _open_files = self.io_resources.try_open_files(2)?;
        if request.follow_symlinks {
            return Err(RuntimeError::new(
                "unsupported_symlink_policy",
                "followSymlinks=true is not supported for safe mutations",
            ));
        }
        let source = self.existing_for(
            &request.source,
            if remove_source {
                PathAccess::MoveSource
            } else {
                PathAccess::Read
            },
        )?;
        if remove_source
            && self
                .allowed_scopes
                .iter()
                .any(|scope| scope == source.as_ref())
        {
            return Err(RuntimeError::new(
                "root_path_rejected",
                "workspace and explicit grant roots cannot be moved",
            ));
        }
        let destination = self.creation_for(
            &request.destination,
            if remove_source {
                PathAccess::MoveDestination
            } else if request.conflict_policy == FsConflictPolicy::Replace {
                PathAccess::Replace
            } else {
                PathAccess::Create
            },
        )?;
        let destination_path = destination.path();
        reject_overlapping_transfer(&source, &destination_path)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: if remove_source { "fs_move" } else { "fs_copy" }.into(),
                root: Some(destination.root.clone()),
                destructive: remove_source || request.conflict_policy == FsConflictPolicy::Replace,
            })
            .await?;
        source.revalidate()?;
        destination.revalidate_parent()?;
        if let Some(expected) = request.expected_source_version.as_deref() {
            self.verify_expected_version(&source, expected, Some(context))
                .await?;
        }
        if let Some(expected) = request.expected_destination_version.as_deref() {
            if !destination_path.exists() {
                return Err(RuntimeError::new(
                    "targetMissing",
                    "expectedDestinationVersion requires an existing destination",
                ));
            }
            self.verify_expected_version(&destination_path, expected, Some(context))
                .await?;
        }
        let destination_exists = destination_path.exists();
        let existing_destination = if destination_exists {
            let existing = self.existing_for(
                &destination_path,
                if remove_source {
                    PathAccess::MoveDestination
                } else {
                    PathAccess::Replace
                },
            )?;
            existing.revalidate()?;
            Some(existing)
        } else {
            None
        };
        if destination_exists && request.conflict_policy == FsConflictPolicy::Error {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and conflictPolicy is error",
            ));
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut result = empty_mutation_result(&operation_id, request.dry_run);
        let source_path = source.as_ref().to_path_buf();
        let parent = destination_path
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "destination has no parent"))?
            .to_path_buf();
        let stage = parent.join(format!(".chatcmd-stage-{operation_id}"));
        let backup = parent.join(format!(".chatcmd-backup-{operation_id}"));
        let journal_path = parent.join(format!(".chatcmd-operation-{operation_id}.json"));
        let mut journal = MutationJournal::new(
            &operation_id,
            if remove_source { "move" } else { "copy" },
            context,
            &source_path,
            &destination_path,
            &stage,
            &backup,
            serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
            self.mutation_journal_sink.clone(),
            self.mutation_fault_injector.clone(),
        );
        let preflight = tokio::task::spawn_blocking({
            let source = source.clone();
            let budget = request.budget.clone();
            let cancellation = context.cancellation.clone();
            move || scan_tree(&source, &budget, &cancellation)
        })
        .await
        .map_err(join_error)?;
        let preflight = match preflight {
            Ok(value) => value,
            Err(error) if error.code == "cancelled" => {
                result.state = "cancelledNoChange".into();
                return Ok(result);
            }
            Err(error) => return Err(error),
        };
        result.files_processed = preflight.files;
        result.directories_processed = preflight.directories;
        result.bytes_copied = preflight.bytes;
        if !request.atomic_publish {
            let warning = "atomicPublish=false was requested; the safety implementation still uses staged publish".to_owned();
            result.warnings.push(warning.clone());
            journal.warnings.push(warning);
        }
        if request.dry_run {
            result.state = "planned".into();
            result.bytes_copied = 0;
            return Ok(result);
        }
        if destination_exists && request.conflict_policy == FsConflictPolicy::Skip {
            result.state = "skipped".into();
            result.bytes_copied = 0;
            return Ok(result);
        }
        let _disk_reservation = self.io_resources.try_reserve_disk(preflight.bytes)?;
        write_journal(&journal_path, &journal)?;
        let worker_request = request.clone();
        let cancellation = context.cancellation.clone();
        let destination_guard = destination.clone();
        let fault_injector = self.mutation_fault_injector.clone();
        let worker = tokio::task::spawn_blocking(move || {
            journal.phase = "staging".into();
            write_journal(&journal_path, &journal)?;
            let deadline = Instant::now() + Duration::from_millis(worker_request.budget.timeout_ms);
            let mut copied = MutationCounts::default();
            if let Err(error) = copy_tree_checked(
                &source_path,
                &stage,
                &worker_request,
                &cancellation,
                deadline,
                &mut copied,
                fault_injector.as_deref(),
            ) {
                let rollback = remove_checked_best_effort(&stage);
                journal.phase = if rollback {
                    "failedRolledBack"
                } else {
                    "failedPartial"
                }
                .into();
                journal.error = Some(error.to_string());
                write_journal(&journal_path, &journal)?;
                return Ok(failed_result(
                    &operation_id,
                    copied,
                    false,
                    rollback,
                    &error,
                ));
            }
            journal.phase = "verifying".into();
            journal.counts = copied;
            write_journal(&journal_path, &journal)?;
            if let Err(error) = verify_trees(
                &source_path,
                &stage,
                worker_request.verify,
                &cancellation,
                deadline,
            ) {
                let rollback = remove_checked_best_effort(&stage);
                journal.phase = if rollback {
                    "failedRolledBack"
                } else {
                    "failedPartial"
                }
                .into();
                journal.error = Some(error.to_string());
                write_journal(&journal_path, &journal)?;
                return Ok(failed_result(
                    &operation_id,
                    copied,
                    false,
                    rollback,
                    &error,
                ));
            }
            journal.phase = "readyToPublish".into();
            write_journal(&journal_path, &journal)?;
            if let Err(error) = check_checkpoint(&cancellation, deadline) {
                let rollback = remove_checked_best_effort(&stage);
                journal.phase = if rollback {
                    "cancelledRolledBack"
                } else {
                    "cancelledPartial"
                }
                .into();
                journal.error = Some(error.to_string());
                write_journal(&journal_path, &journal)?;
                return Ok(failed_result(
                    &operation_id,
                    copied,
                    false,
                    rollback,
                    &error,
                ));
            }
            let destination_validation = destination_guard.revalidate_parent().and_then(|()| {
                existing_destination
                    .as_ref()
                    .map_or(Ok(()), ExistingWorkspacePath::revalidate)
            });
            if let Err(error) = destination_validation {
                let rollback = remove_checked_best_effort(&stage);
                journal.phase = if rollback {
                    "failedRolledBack"
                } else {
                    "failedPartial"
                }
                .into();
                journal.error = Some(error.to_string());
                write_journal(&journal_path, &journal)?;
                return Ok(failed_result(
                    &operation_id,
                    copied,
                    false,
                    rollback,
                    &error,
                ));
            }
            let mut backup_created = false;
            if destination_path.exists() {
                journal.phase = "backingUpDestination".into();
                journal.backup_created = true;
                write_journal(&journal_path, &journal)?;
                fs::rename(&destination_path, &backup).map_err(io_error)?;
                backup_created = true;
                if let Some(injector) = fault_injector.as_deref() {
                    injector.checkpoint(
                        "afterBackupRenameBeforeJournal",
                        copied.files,
                        copied.bytes,
                    )?;
                }
                journal.phase = "destinationBackedUp".into();
                write_journal(&journal_path, &journal)?;
            }
            journal.phase = "publishing".into();
            write_journal(&journal_path, &journal)?;
            if let Err(error) = fs::rename(&stage, &destination_path) {
                let mut rollback = remove_checked_best_effort(&stage);
                if backup_created {
                    rollback &= fs::rename(&backup, &destination_path).is_ok();
                }
                journal.phase = if rollback {
                    "failedRolledBack"
                } else {
                    "failedPartial"
                }
                .into();
                journal.error = Some(error.to_string());
                write_journal(&journal_path, &journal)?;
                let runtime_error = io_error(error);
                return Ok(failed_result(
                    &operation_id,
                    copied,
                    false,
                    rollback,
                    &runtime_error,
                ));
            }
            if let Some(injector) = fault_injector.as_deref() {
                injector.checkpoint("afterPublishBeforeJournal", copied.files, copied.bytes)?;
            }
            journal.phase = "published".into();
            write_journal(&journal_path, &journal)?;
            if backup_created && !remove_checked_best_effort(&backup) {
                journal.warnings.push(format!(
                    "old destination backup remains at {}",
                    backup.display()
                ));
            }
            let mut source_removed = false;
            let mut state = "completed".to_owned();
            if remove_source {
                journal.phase = "removingSource".into();
                write_journal(&journal_path, &journal)?;
                source_removed =
                    remove_tree_checked(&source_path, true, &cancellation, deadline).is_ok();
                if !source_removed {
                    state = "completedWithSourceRemaining".into();
                    journal
                        .warnings
                        .push("destination published but source cleanup failed".into());
                }
            }
            journal.phase.clone_from(&state);
            write_journal(&journal_path, &journal)?;
            remove_journal(&journal_path, &journal)?;
            Ok(FsMutationResult {
                operation_id,
                state,
                files_processed: copied.files,
                directories_processed: copied.directories,
                bytes_copied: copied.bytes,
                source_removed,
                destination_published: true,
                verified: worker_request.verify != FsVerifyMode::None,
                rollback_attempted: false,
                rollback_completed: false,
                dry_run: false,
                warnings: journal.warnings,
                detail_artifact_ref: None,
            })
        })
        .await
        .map_err(join_error)??;
        Ok(worker)
    }

    pub async fn delete(
        &self,
        context: &OperationContext,
        path: &Path,
        recursive: bool,
    ) -> RuntimeResult<bool> {
        let result = self
            .delete_safe(
                context,
                &FsDeleteRequest {
                    path: path.to_path_buf(),
                    recursive,
                    mode: FsDeleteMode::Permanent,
                    expected_version: None,
                    dry_run: false,
                    budget: FsMutationBudget::default(),
                },
            )
            .await?;
        Ok(result.state == "completed")
    }

    pub async fn delete_safe(
        &self,
        context: &OperationContext,
        request: &FsDeleteRequest,
    ) -> RuntimeResult<FsMutationResult> {
        let mut effective_request = request.clone();
        effective_request.budget.timeout_ms = effective_request.budget.timeout_ms.min(30 * 60_000);
        effective_request.budget.max_files = effective_request.budget.max_files.min(2_000_000);
        effective_request.budget.max_bytes_read = effective_request
            .budget
            .max_bytes_read
            .min(2 * 1024 * 1024 * 1024 * 1024);
        effective_request.budget.max_bytes_written = effective_request
            .budget
            .max_bytes_written
            .min(2 * 1024 * 1024 * 1024 * 1024);
        effective_request.budget.max_open_files =
            effective_request.budget.max_open_files.clamp(1, 64);
        let request = &effective_request;
        let _admission = self
            .admission
            .try_admit(&context.agent_id, 2, 8 * 1024 * 1024)?;
        let _open_files = self.io_resources.try_open_files(1)?;
        let resolved = self.existing_for(&request.path, PathAccess::Delete)?;
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
        if let Some(expected) = request.expected_version.as_deref() {
            self.verify_expected_version(&resolved, expected, Some(context))
                .await?;
        }
        let counts = tokio::task::spawn_blocking({
            let resolved = resolved.clone();
            let budget = request.budget.clone();
            let cancellation = context.cancellation.clone();
            move || scan_tree(&resolved, &budget, &cancellation)
        })
        .await
        .map_err(join_error)?;
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut result = empty_mutation_result(&operation_id, request.dry_run);
        let counts = match counts {
            Ok(value) => value,
            Err(error) if error.code == "cancelled" => {
                result.state = "cancelledNoChange".into();
                return Ok(result);
            }
            Err(error) => return Err(error),
        };
        result.files_processed = counts.files;
        result.directories_processed = counts.directories;
        if request.dry_run {
            result.state = "planned".into();
            return Ok(result);
        }
        let source = resolved.as_ref().to_path_buf();
        let parent = source
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "path has no parent"))?
            .to_path_buf();
        let quarantine = parent.join(format!(".chatcmd-quarantine-{operation_id}"));
        let journal_path = parent.join(format!(".chatcmd-operation-{operation_id}.json"));
        let mut journal = MutationJournal::new(
            &operation_id,
            "delete",
            context,
            &source,
            &quarantine,
            &quarantine,
            &quarantine,
            serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
            self.mutation_journal_sink.clone(),
            self.mutation_fault_injector.clone(),
        );
        write_journal(&journal_path, &journal)?;
        let cancellation = context.cancellation.clone();
        let recursive = request.recursive;
        let mode = request.mode;
        let timeout_ms = request.budget.timeout_ms;
        tokio::task::spawn_blocking(move || {
            resolved.revalidate()?;
            if resolved.is_dir()
                && !recursive
                && fs::read_dir(&source).map_err(io_error)?.next().is_some()
            {
                return Err(RuntimeError::new(
                    "directory_not_empty",
                    "recursive=true is required for a non-empty directory",
                ));
            }
            if mode == FsDeleteMode::Quarantine {
                journal.phase = "quarantining".into();
                write_journal(&journal_path, &journal)?;
                fs::rename(&source, &quarantine).map_err(|error| RuntimeError::new(
                    "quarantine_unavailable",
                    format!("same-filesystem quarantine failed; use explicit permanent mode if intended: {error}"),
                ))?;
                journal.phase = "completed".into();
                journal.warnings.push(format!("quarantined data retained at {}", quarantine.display()));
                write_journal(&journal_path, &journal)?;
            } else {
                journal.phase = "deleting".into();
                write_journal(&journal_path, &journal)?;
                remove_tree_checked(&source, recursive, &cancellation, Instant::now() + Duration::from_millis(timeout_ms))?;
                journal.phase = "completed".into();
                write_journal(&journal_path, &journal)?;
            }
            remove_journal(&journal_path, &journal)?;
            Ok(FsMutationResult {
                operation_id,
                state: "completed".into(),
                files_processed: counts.files,
                directories_processed: counts.directories,
                bytes_copied: 0,
                source_removed: true,
                destination_published: false,
                verified: true,
                rollback_attempted: false,
                rollback_completed: false,
                dry_run: false,
                warnings: journal.warnings,
                detail_artifact_ref: None,
            })
        }).await.map_err(join_error)?
    }

    pub async fn restore_quarantine(
        &self,
        context: &OperationContext,
        request: &FsQuarantineRestoreRequest,
    ) -> RuntimeResult<FsMutationResult> {
        let quarantine = self.existing_for(&request.quarantine_path, PathAccess::MoveSource)?;
        let name = quarantine
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !name.starts_with(".chatcmd-quarantine-") {
            return Err(RuntimeError::new(
                "invalid_quarantine_path",
                "restore source must be a ChatCMD-managed quarantine path",
            ));
        }
        let transfer = FsTransferRequest {
            source: quarantine.as_ref().to_path_buf(),
            destination: request.destination.clone(),
            conflict_policy: if request.replace {
                FsConflictPolicy::Replace
            } else {
                FsConflictPolicy::Error
            },
            atomic_publish: true,
            verify: FsVerifyMode::Metadata,
            preserve_metadata: true,
            follow_symlinks: false,
            dry_run: false,
            expected_source_version: None,
            expected_destination_version: None,
            budget: FsMutationBudget::default(),
        };
        self.move_safe(context, &transfer).await
    }

    pub async fn quarantine_gc(
        &self,
        context: &OperationContext,
        request: &FsQuarantineGcRequest,
    ) -> RuntimeResult<FsQuarantineGcResult> {
        let root = self.existing_for(&request.path, PathAccess::Read)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_quarantine_gc".into(),
                root: Some(root.root.clone()),
                destructive: !request.dry_run,
            })
            .await?;
        root.revalidate()?;
        if !root.is_dir() {
            return Err(RuntimeError::new(
                "invalid_path",
                "quarantine GC path must be a directory",
            ));
        }
        let root_path = root.as_ref().to_path_buf();
        let retention = Duration::from_secs(request.retention_seconds);
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(UNIX_EPOCH);
        let max_items = request.max_items.min(100_000);
        let cancellation = context.cancellation.clone();
        let dry_run = request.dry_run;
        let max_total_bytes = request.max_total_bytes;
        tokio::task::spawn_blocking(move || {
            let mut candidates = Vec::<QuarantineCandidate>::new();
            let mut stack = vec![root_path];
            let scan_deadline = Instant::now() + Duration::from_secs(300);
            while let Some(directory) = stack.pop() {
                check_checkpoint(&cancellation, scan_deadline)?;
                for entry in fs::read_dir(&directory).map_err(io_error)? {
                    let entry = entry.map_err(io_error)?;
                    let path = entry.path();
                    let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
                    reject_reparse_metadata(&metadata)?;
                    let managed = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .is_some_and(|name| name.starts_with(".chatcmd-quarantine-"));
                    if managed {
                        if u64::try_from(candidates.len()).unwrap_or(u64::MAX) >= max_items {
                            return Err(RuntimeError::new(
                                "budget_exceeded",
                                "quarantine GC exceeded maxItems",
                            ));
                        }
                        let bytes = tree_size_no_follow(&path)?;
                        candidates.push(QuarantineCandidate {
                            path,
                            bytes,
                            modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                        });
                    } else if metadata.is_dir() {
                        stack.push(path);
                    }
                }
            }
            candidates.sort_by_key(|item| item.modified);
            let total_bytes = candidates
                .iter()
                .fold(0_u64, |total, item| total.saturating_add(item.bytes));
            let mut retained_bytes = total_bytes;
            let mut removed_items = 0_u64;
            let mut bytes_removed = 0_u64;
            let deadline = Instant::now() + Duration::from_secs(300);
            for candidate in &candidates {
                let expired = candidate.modified <= cutoff;
                let over_quota = retained_bytes > max_total_bytes;
                if !expired && !over_quota {
                    continue;
                }
                if !dry_run {
                    remove_tree_checked(&candidate.path, true, &cancellation, deadline)?;
                }
                removed_items = removed_items.saturating_add(1);
                bytes_removed = bytes_removed.saturating_add(candidate.bytes);
                retained_bytes = retained_bytes.saturating_sub(candidate.bytes);
            }
            Ok(FsQuarantineGcResult {
                scanned_items: u64::try_from(candidates.len()).unwrap_or(u64::MAX),
                removed_items,
                bytes_removed,
                retained_bytes,
                dry_run,
                warnings: Vec::new(),
            })
        })
        .await
        .map_err(join_error)?
    }

    /// Recovers operation-owned staging/backup paths left behind by an interrupted
    /// filesystem mutation. Sidecar journals are authoritative when present; the
    /// optional durable sink is also queried so a SQLite row cannot remain orphaned
    /// indefinitely when its sidecar has already disappeared. Every recorded path
    /// must remain inside one configured workspace root before recovery touches it.
    pub async fn recover_interrupted_mutations(&self) -> RuntimeResult<u64> {
        let roots = self.roots.clone();
        let sink = self.mutation_journal_sink.clone();
        tokio::task::spawn_blocking(move || {
            let mut recovered = 0_u64;
            let mut seen = std::collections::HashSet::new();
            for root in &roots {
                let mut stack = vec![root.clone()];
                while let Some(directory) = stack.pop() {
                    let entries = match fs::read_dir(&directory) {
                        Ok(entries) => entries,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(io_error(error)),
                    };
                    for entry in entries {
                        let entry = entry.map_err(io_error)?;
                        let path = entry.path();
                        let metadata = match fs::symlink_metadata(&path) {
                            Ok(metadata) => metadata,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(io_error(error)),
                        };
                        if let Err(error) = reject_reparse_metadata(&metadata) {
                            if error.code == "symlink_traversal_rejected" {
                                continue;
                            }
                            return Err(error);
                        }
                        if metadata.is_dir() {
                            stack.push(path);
                            continue;
                        }
                        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                            continue;
                        };
                        if !name.starts_with(".chatcmd-operation-") || !name.ends_with(".json") {
                            continue;
                        }
                        let bytes = fs::read(&path).map_err(io_error)?;
                        let mut journal: MutationJournal =
                            serde_json::from_slice(&bytes).map_err(|error| {
                                RuntimeError::new(
                                    "journal_error",
                                    format!(
                                        "invalid recovery journal {}: {error}",
                                        path.display()
                                    ),
                                )
                            })?;
                        journal.sink = sink.clone();
                        if !journal_paths_within_root(&journal, root) {
                            return Err(RuntimeError::new(
                                "journal_path_escape",
                                format!(
                                    "recovery journal {} references a path outside its workspace root",
                                    path.display()
                                ),
                            ));
                        }
                        seen.insert(journal.operation_id.clone());
                        if recover_mutation_journal(&path, &journal)? {
                            recovered = recovered.saturating_add(1);
                        }
                    }
                }
            }

            if let Some(sink) = sink.as_ref() {
                for journal_json in sink.list_json()? {
                    let mut journal: MutationJournal =
                        serde_json::from_str(&journal_json).map_err(|error| {
                            RuntimeError::new(
                                "journal_error",
                                format!("invalid durable recovery journal: {error}"),
                            )
                        })?;
                    if seen.contains(&journal.operation_id) {
                        continue;
                    }
                    let Some(root) = roots
                        .iter()
                        .find(|root| journal_paths_within_root(&journal, root))
                    else {
                        return Err(RuntimeError::new(
                            "journal_path_escape",
                            format!(
                                "durable recovery journal {} references a path outside configured workspace roots",
                                journal.operation_id
                            ),
                        ));
                    };
                    journal.sink = Some(sink.clone());
                    let journal_parent = if journal.operation_type == "delete" {
                        journal.source.parent()
                    } else {
                        journal.destination.parent()
                    }
                    .ok_or_else(|| {
                        RuntimeError::new(
                            "journal_error",
                            format!(
                                "durable recovery journal {} has no journal parent",
                                journal.operation_id
                            ),
                        )
                    })?;
                    let journal_path = journal_parent.join(format!(
                        ".chatcmd-operation-{}.json",
                        journal.operation_id
                    ));
                    if !journal_path.starts_with(root) {
                        return Err(RuntimeError::new(
                            "journal_path_escape",
                            "durable recovery sidecar path escapes workspace root",
                        ));
                    }
                    if recover_mutation_journal(&journal_path, &journal)? {
                        recovered = recovered.saturating_add(1);
                    }
                }
            }
            Ok(recovered)
        })
        .await
        .map_err(join_error)?
    }
}

fn recover_mutation_journal(path: &Path, journal: &MutationJournal) -> RuntimeResult<bool> {
    if journal.operation_type == "delete" {
        let mode = journal
            .requested_options
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("quarantine");
        if mode == "quarantine" {
            let source_exists = journal.source.exists();
            let quarantine_exists = journal.destination.exists();
            if source_exists && quarantine_exists {
                return Ok(false);
            }
            remove_journal(path, journal)?;
            return Ok(true);
        }
        if journal.phase == "deleting" && journal.source.exists() {
            return Ok(false);
        }
        remove_journal(path, journal)?;
        return Ok(true);
    }

    let published = matches!(
        journal.phase.as_str(),
        "published" | "removingSource" | "completed" | "completedWithSourceRemaining"
    );
    let mut complete = true;
    if journal.staging_path.exists() {
        complete &= remove_checked_best_effort(&journal.staging_path);
    }
    if journal.backup_path.exists() {
        if published || journal.destination.exists() {
            complete &= remove_checked_best_effort(&journal.backup_path);
        } else {
            complete &= fs::rename(&journal.backup_path, &journal.destination).is_ok();
        }
    }
    if complete {
        remove_journal(path, journal)?;
    }
    Ok(complete)
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct MutationCounts {
    files: u64,
    directories: u64,
    bytes: u64,
}

#[derive(Debug)]
struct QuarantineCandidate {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutationJournal {
    operation_id: String,
    operation_type: String,
    owner_agent: String,
    owner_task: Option<String>,
    source: PathBuf,
    destination: PathBuf,
    staging_path: PathBuf,
    backup_path: PathBuf,
    #[serde(default)]
    requested_options: serde_json::Value,
    phase: String,
    counts: MutationCounts,
    backup_created: bool,
    rollback_actions: Vec<String>,
    warnings: Vec<String>,
    error: Option<String>,
    updated_at_unix_ms: u128,
    #[serde(skip)]
    sink: Option<std::sync::Arc<MutationJournalSinkDyn>>,
    #[serde(skip)]
    fault_injector: Option<std::sync::Arc<MutationFaultInjectorDyn>>,
}

type MutationJournalSinkDyn = dyn crate::MutationJournalSink;
type MutationFaultInjectorDyn = dyn crate::MutationFaultInjector;

impl MutationJournal {
    #[allow(clippy::too_many_arguments)]
    fn new(
        operation_id: &str,
        operation_type: &str,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        staging_path: &Path,
        backup_path: &Path,
        requested_options: serde_json::Value,
        sink: Option<std::sync::Arc<MutationJournalSinkDyn>>,
        fault_injector: Option<std::sync::Arc<MutationFaultInjectorDyn>>,
    ) -> Self {
        Self {
            operation_id: operation_id.to_owned(),
            operation_type: operation_type.to_owned(),
            owner_agent: context.agent_id.clone(),
            owner_task: context.task_id.clone(),
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            staging_path: staging_path.to_path_buf(),
            backup_path: backup_path.to_path_buf(),
            requested_options,
            phase: "planned".into(),
            counts: MutationCounts::default(),
            backup_created: false,
            rollback_actions: vec![
                "remove staging path".into(),
                "restore destination backup".into(),
            ],
            warnings: Vec::new(),
            error: None,
            updated_at_unix_ms: now_unix_ms(),
            sink,
            fault_injector,
        }
    }
}

fn journal_paths_within_root(journal: &MutationJournal, root: &Path) -> bool {
    [
        journal.source.as_path(),
        journal.destination.as_path(),
        journal.staging_path.as_path(),
        journal.backup_path.as_path(),
    ]
    .into_iter()
    .all(|path| path.starts_with(root))
}

fn empty_mutation_result(operation_id: &str, dry_run: bool) -> FsMutationResult {
    FsMutationResult {
        operation_id: operation_id.to_owned(),
        state: "completed".into(),
        files_processed: 0,
        directories_processed: 0,
        bytes_copied: 0,
        source_removed: false,
        destination_published: false,
        verified: false,
        rollback_attempted: false,
        rollback_completed: false,
        dry_run,
        warnings: Vec::new(),
        detail_artifact_ref: None,
    }
}

fn failed_result(
    operation_id: &str,
    counts: MutationCounts,
    published: bool,
    rollback_completed: bool,
    error: &RuntimeError,
) -> FsMutationResult {
    let cancelled = error.code == "cancelled";
    FsMutationResult {
        operation_id: operation_id.to_owned(),
        state: if cancelled {
            if rollback_completed {
                "cancelledRolledBack"
            } else {
                "cancelledPartial"
            }
        } else if rollback_completed {
            "failedRolledBack"
        } else {
            "failedPartial"
        }
        .into(),
        files_processed: counts.files,
        directories_processed: counts.directories,
        bytes_copied: counts.bytes,
        source_removed: false,
        destination_published: published,
        verified: false,
        rollback_attempted: true,
        rollback_completed,
        dry_run: false,
        warnings: vec![error.to_string()],
        detail_artifact_ref: None,
    }
}

fn reject_overlapping_transfer(source: &Path, destination: &Path) -> RuntimeResult<()> {
    if source == destination || destination.starts_with(source) || source.starts_with(destination) {
        return Err(RuntimeError::new(
            "overlapping_transfer",
            "source and destination must not contain one another",
        ));
    }
    Ok(())
}

fn scan_tree(
    root: &Path,
    budget: &FsMutationBudget,
    cancellation: &tokio_util::sync::CancellationToken,
) -> RuntimeResult<MutationCounts> {
    let deadline = Instant::now() + Duration::from_millis(budget.timeout_ms);
    let mut stack = vec![root.to_path_buf()];
    let mut counts = MutationCounts::default();
    while let Some(path) = stack.pop() {
        check_checkpoint(cancellation, deadline)?;
        let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        if metadata.is_dir() {
            counts.directories = counts.directories.saturating_add(1);
            for entry in fs::read_dir(&path).map_err(io_error)? {
                stack.push(entry.map_err(io_error)?.path());
            }
        } else if metadata.is_file() {
            counts.files = counts.files.saturating_add(1);
            counts.bytes = counts.bytes.saturating_add(metadata.len());
        } else {
            return Err(RuntimeError::new(
                "unsupported_file_type",
                "mutation source contains an unsupported file type",
            ));
        }
        if counts.files.saturating_add(counts.directories) > budget.max_files {
            return Err(RuntimeError::new(
                "budget_exceeded",
                "preflight exceeded maxFiles",
            ));
        }
        if counts.bytes > budget.max_bytes_read || counts.bytes > budget.max_bytes_written {
            return Err(RuntimeError::new(
                "budget_exceeded",
                "preflight exceeded byte budget",
            ));
        }
    }
    Ok(counts)
}

fn copy_tree_checked(
    source: &Path,
    destination: &Path,
    request: &FsTransferRequest,
    cancellation: &tokio_util::sync::CancellationToken,
    deadline: Instant,
    counts: &mut MutationCounts,
    fault_injector: Option<&dyn crate::MutationFaultInjector>,
) -> RuntimeResult<()> {
    enum Work {
        Visit(PathBuf, PathBuf),
        FinalizeDirectory(PathBuf, fs::Permissions),
    }

    let mut stack = vec![Work::Visit(source.to_path_buf(), destination.to_path_buf())];
    while let Some(work) = stack.pop() {
        check_checkpoint(cancellation, deadline)?;
        match work {
            Work::FinalizeDirectory(path, permissions) => {
                if request.preserve_metadata {
                    fs::set_permissions(path, permissions).map_err(io_error)?;
                }
            }
            Work::Visit(source, destination) => {
                let before = fs::symlink_metadata(&source).map_err(io_error)?;
                reject_reparse_metadata(&before)?;
                if before.is_dir() {
                    fs::create_dir(&destination).map_err(io_error)?;
                    counts.directories = counts.directories.saturating_add(1);
                    if counts.files.saturating_add(counts.directories) > request.budget.max_files {
                        return Err(RuntimeError::new(
                            "budget_exceeded",
                            "copy exceeded maxFiles",
                        ));
                    }
                    stack.push(Work::FinalizeDirectory(
                        destination.clone(),
                        before.permissions(),
                    ));
                    for entry in fs::read_dir(&source).map_err(io_error)? {
                        let entry = entry.map_err(io_error)?;
                        stack.push(Work::Visit(
                            entry.path(),
                            destination.join(entry.file_name()),
                        ));
                    }
                } else if before.is_file() {
                    let mut input = fs::File::open(&source).map_err(io_error)?;
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&destination)
                        .map_err(io_error)?;
                    let mut buffer = vec![0_u8; 1024 * 1024];
                    loop {
                        check_checkpoint(cancellation, deadline)?;
                        let read = input.read(&mut buffer).map_err(io_error)?;
                        if read == 0 {
                            break;
                        }
                        counts.bytes = counts
                            .bytes
                            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                        if counts.bytes > request.budget.max_bytes_read
                            || counts.bytes > request.budget.max_bytes_written
                        {
                            return Err(RuntimeError::new(
                                "budget_exceeded",
                                "copy exceeded byte budget",
                            ));
                        }
                        output.write_all(&buffer[..read]).map_err(io_error)?;
                        if let Some(injector) = fault_injector {
                            injector.checkpoint("copyBytes", counts.files, counts.bytes)?;
                        }
                    }
                    output.sync_all().map_err(io_error)?;
                    if request.preserve_metadata {
                        fs::set_permissions(&destination, before.permissions())
                            .map_err(io_error)?;
                    }
                    let after = fs::symlink_metadata(&source).map_err(io_error)?;
                    if FileIdentity::from_metadata(&before) != FileIdentity::from_metadata(&after) {
                        return Err(RuntimeError::new(
                            "source_changed",
                            "source changed while it was copied",
                        ));
                    }
                    counts.files = counts.files.saturating_add(1);
                    if let Some(injector) = fault_injector {
                        injector.checkpoint("copyFile", counts.files, counts.bytes)?;
                    }
                    if counts.files.saturating_add(counts.directories) > request.budget.max_files {
                        return Err(RuntimeError::new(
                            "budget_exceeded",
                            "copy exceeded maxFiles",
                        ));
                    }
                } else {
                    return Err(RuntimeError::new(
                        "unsupported_file_type",
                        "mutation source contains an unsupported file type",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_trees(
    source: &Path,
    destination: &Path,
    mode: FsVerifyMode,
    cancellation: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> RuntimeResult<()> {
    if mode == FsVerifyMode::None {
        return Ok(());
    }
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((source, destination)) = stack.pop() {
        check_checkpoint(cancellation, deadline)?;
        let source_metadata = fs::symlink_metadata(&source).map_err(io_error)?;
        let destination_metadata = fs::symlink_metadata(&destination).map_err(io_error)?;
        reject_reparse_metadata(&source_metadata)?;
        reject_reparse_metadata(&destination_metadata)?;
        if source_metadata.is_dir() != destination_metadata.is_dir()
            || (!source_metadata.is_dir() && source_metadata.len() != destination_metadata.len())
        {
            return Err(RuntimeError::new(
                "verification_failed",
                "staged copy metadata differs from source",
            ));
        }
        if source_metadata.is_dir() {
            for entry in fs::read_dir(&source).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                stack.push((entry.path(), destination.join(entry.file_name())));
            }
        } else if mode == FsVerifyMode::Content
            && hash_file(&source, cancellation, deadline)?
                != hash_file(&destination, cancellation, deadline)?
        {
            return Err(RuntimeError::new(
                "verification_failed",
                "staged file content hash differs from source",
            ));
        }
    }
    Ok(())
}

fn hash_file(
    path: &Path,
    cancellation: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> RuntimeResult<[u8; 32]> {
    let mut input = fs::File::open(path).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        check_checkpoint(cancellation, deadline)?;
        let read = input.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn remove_tree_checked(
    path: &Path,
    recursive: bool,
    cancellation: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> RuntimeResult<()> {
    enum Work {
        Visit(PathBuf),
        RemoveDirectory(PathBuf),
    }

    let mut stack = vec![Work::Visit(path.to_path_buf())];
    while let Some(work) = stack.pop() {
        check_checkpoint(cancellation, deadline)?;
        match work {
            Work::RemoveDirectory(path) => fs::remove_dir(path).map_err(io_error)?,
            Work::Visit(path) => {
                let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
                reject_reparse_metadata(&metadata)?;
                if metadata.is_dir() {
                    if recursive {
                        stack.push(Work::RemoveDirectory(path.clone()));
                        for entry in fs::read_dir(&path).map_err(io_error)? {
                            stack.push(Work::Visit(entry.map_err(io_error)?.path()));
                        }
                    } else {
                        fs::remove_dir(path).map_err(io_error)?;
                    }
                } else {
                    fs::remove_file(path).map_err(io_error)?;
                }
            }
        }
    }
    Ok(())
}

fn tree_size_no_follow(path: &Path) -> RuntimeResult<u64> {
    let mut total = 0_u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let metadata = fs::symlink_metadata(&current).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(&current).map_err(io_error)? {
                stack.push(entry.map_err(io_error)?.path());
            }
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn remove_checked_best_effort(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    let token = tokio_util::sync::CancellationToken::new();
    remove_tree_checked(path, true, &token, Instant::now() + Duration::from_secs(30)).is_ok()
}

fn check_checkpoint(
    cancellation: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> RuntimeResult<()> {
    if cancellation.is_cancelled() {
        Err(RuntimeError::new(
            "cancelled",
            "filesystem mutation was cancelled",
        ))
    } else if Instant::now() >= deadline {
        Err(RuntimeError::new(
            "timeout",
            "filesystem mutation timed out",
        ))
    } else {
        Ok(())
    }
}

fn write_journal(path: &Path, journal: &MutationJournal) -> RuntimeResult<()> {
    let mut value = serde_json::to_vec_pretty(journal)
        .map_err(|error| RuntimeError::new("journal_error", error.to_string()))?;
    value.push(b'\n');
    let temporary = path.with_extension("json.tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(io_error)?;
    file.write_all(&value).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    fs::rename(&temporary, path).map_err(io_error)?;
    if let Some(sink) = &journal.sink {
        let json = std::str::from_utf8(&value)
            .map_err(|error| RuntimeError::new("journal_error", error.to_string()))?;
        sink.upsert_json(json)?;
    }
    if let Some(injector) = &journal.fault_injector {
        injector.checkpoint(&journal.phase, journal.counts.files, journal.counts.bytes)?;
    }
    Ok(())
}

fn remove_journal(path: &Path, journal: &MutationJournal) -> RuntimeResult<()> {
    if let Some(sink) = &journal.sink {
        sink.remove(&journal.operation_id)?;
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}
