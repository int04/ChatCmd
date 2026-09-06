impl RuntimeHost {
    pub(super) async fn authorize_execution(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<()> {
        let capabilities = tool_capabilities(tool);
        if capabilities.is_permission_change() {
            return Err(RuntimeError::new(
                "permission_change_requires_user",
                "execution permissions can only be changed through the authenticated local UI",
            ));
        }
        if !capabilities.is_execution_policy_controlled() {
            return Ok(());
        }
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let mode_task_id = self.execution_mode_task_id(&task_id).await?;
        match self
            .repository
            .execution_mode(Some(&mode_task_id))
            .await
            .map_err(storage_error)?
        {
            chatcmd_core::ExecutionMode::Allow => return Ok(()),
            chatcmd_core::ExecutionMode::Deny => {
                return Err(RuntimeError::new(
                    "policy_denied",
                    "conversation access mode denied this operation",
                ));
            }
            chatcmd_core::ExecutionMode::Approval => {}
        }

        let resolved_arguments = self
            .resolve_approval_paths(context, tool, arguments)
            .await?;
        if capabilities.risk_class.is_safe_read()
            && self
                .consume_safe_read_grant(context, tool, &resolved_arguments)
                .await?
        {
            return Ok(());
        }

        let approval_id = context.request_id.clone();
        let turn_id = context.turn_id.as_deref().unwrap_or_default();
        let grant_preview = if capabilities.risk_class.is_safe_read() {
            Some(self.safe_read_grant_preview(context, tool).await?)
        } else {
            None
        };
        let operation_digest = operation_digest(tool, &resolved_arguments);
        let mut summary = approval_summary(tool, capabilities.risk_class, &resolved_arguments);
        if let Some(preview) = grant_preview.as_ref() {
            summary["grantPreview"] = preview.clone();
        }
        let request_json = json!({
            "activityId": approval_id,
            "agentId": context.agent_id,
            "tool": tool,
            "turnId": turn_id,
            "riskClass": capabilities.risk_class,
            "summary": summary,
            "operationDigest": operation_digest,
            "catalogHash": catalog_hash(),
            "grantPreview": grant_preview,
        })
        .to_string();
        let created_at_ms = now_ms();
        // Approval IDs are server-derived request IDs and single-use. Never
        // reset a terminal decision to pending when a request is replayed.
        let inserted = sqlx::query("INSERT OR IGNORE INTO approvals(id,task_id,session_id,state,request_json,decision_json,created_at_ms,resolved_at_ms) VALUES(?,?,NULL,'pending',?,NULL,?,NULL)")
            .bind(&approval_id)
            .bind(task_id.as_str())
            .bind(&request_json)
            .bind(created_at_ms)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval creation failed"))?
            .rows_affected();
        if inserted != 1 {
            return Err(RuntimeError::new(
                "approval_replayed",
                "approval request identity was already used",
            ));
        }
        self.publish_event(
            format!("approval-pending:{approval_id}:{created_at_ms}"),
            "approval.pending",
            Some(task_id.as_str().to_owned()),
            None,
            Some(turn_id.to_owned()),
            json!({
                "activityId": approval_id,
                "tool": tool,
                "summary": summary,
                "riskClass": capabilities.risk_class,
                "grantPreview": grant_preview,
                "approvalDeadlineUtc": time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(created_at_ms.saturating_add(i64::try_from(APPROVAL_TIMEOUT.as_millis()).unwrap_or(i64::MAX))) * 1_000_000)
                    .ok()
                    .and_then(|value| value.format(&time::format_description::well_known::Rfc3339).ok()),
            }),
        );
        self.append_call_event(
            context,
            tool,
            "pending_approval",
            Some(&summary),
            None,
            None,
        )
        .await?;
        self.publish_parent_subagent_approval(
            &task_id,
            &approval_id,
            turn_id,
            tool,
            &summary,
            "subagent.approval_pending",
        )
        .await?;
        self.wait_for_approval(context, &task_id, &approval_id)
            .await?;
        self.recheck_approved_execution(&mode_task_id, &task_id, &approval_id, &operation_digest)
            .await
    }

    async fn recheck_approved_execution(
        &self,
        mode_task_id: &TaskId,
        task_id: &TaskId,
        approval_id: &str,
        operation_digest: &str,
    ) -> RuntimeResult<()> {
        let mode = self
            .repository
            .execution_mode(Some(mode_task_id))
            .await
            .map_err(storage_error)?;
        if mode == chatcmd_core::ExecutionMode::Deny {
            return Err(RuntimeError::new(
                "policy_denied",
                "conversation access mode was revoked before dispatch",
            ));
        }
        if mode == chatcmd_core::ExecutionMode::Allow {
            return Ok(());
        }
        let request_json = sqlx::query_scalar::<_, String>(
            "SELECT request_json FROM approvals WHERE id=? AND task_id=? AND state='approved' LIMIT 1",
        )
        .bind(approval_id)
        .bind(task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "approval dispatch recheck failed"))?
        .ok_or_else(|| {
            RuntimeError::new(
                "approval_stale",
                "approval was revoked or replaced before dispatch",
            )
        })?;
        let request = serde_json::from_str::<Value>(&request_json).map_err(|_| {
            RuntimeError::new("approval_stale", "approval request record is invalid")
        })?;
        if request.get("operationDigest").and_then(Value::as_str) != Some(operation_digest)
            || request.get("catalogHash").and_then(Value::as_str) != Some(catalog_hash().as_str())
        {
            return Err(RuntimeError::new(
                "approval_stale",
                "approval no longer matches the operation or tool catalog",
            ));
        }
        Ok(())
    }

    async fn publish_parent_subagent_approval(
        &self,
        child_task_id: &TaskId,
        approval_id: &str,
        child_turn_id: &str,
        tool: &str,
        arguments: &Value,
        event_type: &str,
    ) -> RuntimeResult<()> {
        let row = sqlx::query(
            "SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(child_task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent approval routing lookup failed"))?;
        let Some(row) = row else {
            return Ok(());
        };
        let subagent_id = row.get::<String, _>("id");
        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let agent_name = row.get::<String, _>("name");
        self.publish_event(
            format!("subagent-approval:{approval_id}:{}", now_ms()),
            event_type,
            Some(parent_task_id),
            Some(child_task_id.as_str().to_owned()),
            Some(parent_turn_id),
            json!({
                "subagentId": subagent_id,
                "childTaskId": child_task_id.as_str(),
                "activityId": approval_id,
                "childTurnId": child_turn_id,
                "agentName": agent_name,
                "tool": tool,
                "input": arguments,
            }),
        );
        Ok(())
    }

    pub(super) async fn execution_mode_task_id(&self, task_id: &TaskId) -> RuntimeResult<TaskId> {
        let parent_task_id = sqlx::query_scalar::<_, String>(
            "WITH RECURSIVE ancestors(id,depth) AS (SELECT parent_task_id,1 FROM subagent_runs WHERE child_task_id=? UNION ALL SELECT parent.parent_task_id,ancestors.depth+1 FROM subagent_runs parent JOIN ancestors ON parent.child_task_id=ancestors.id WHERE ancestors.depth<32) SELECT id FROM ancestors ORDER BY depth DESC LIMIT 1",
        )
        .bind(task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent approval scope lookup failed")
        })?;
        match parent_task_id {
            Some(parent_task_id) => {
                TaskId::new(parent_task_id).map_err(|error| invalid("taskId", error))
            }
            None => Ok(task_id.clone()),
        }
    }

    async fn resolve_approval_paths(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<Value> {
        if !tool.starts_with("fs_") && !tool.starts_with("workspace_index_") {
            return Ok(arguments.clone());
        }
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let base = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?
            .and_then(|task| task.project_folder)
            .map(PathBuf::from);
        super::filesystem_dispatch::resolve_relative_paths(arguments.clone(), base.as_deref())
    }

    async fn safe_read_grant_preview(
        &self,
        context: &OperationContext,
        tool: &str,
    ) -> RuntimeResult<Value> {
        let scopes = self.approval_path_scopes(context).await?;
        let allowed_tools = TOOL_NAMES
            .iter()
            .filter(|name| {
                let capabilities = tool_capabilities(name);
                capabilities.approval_required && capabilities.risk_class.is_safe_read()
            })
            .cloned()
            .collect::<Vec<_>>();
        debug_assert!(allowed_tools.iter().any(|name| name == tool));
        Ok(json!({
            "allowedTools": allowed_tools, "pathScopes": scopes, "maxCalls": SAFE_READ_MAX_CALLS,
            "maxFilesScanned": SAFE_READ_MAX_FILES, "maxBytesRead": SAFE_READ_MAX_BYTES,
            "expiresAtMs": now_ms().saturating_add(SAFE_READ_GRANT_TTL_MS),
            "optionConstraints": {"includeIgnored": false, "includeHidden": false}
        }))
    }

    async fn approval_path_scopes(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Vec<GrantPathScope>> {
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let mut paths = self.task_user_path_scopes(context).await?;
        if let Some(project) = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?
            .and_then(|task| task.project_folder)
            .map(PathBuf::from)
        {
            paths.push(project);
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| {
                let canonical = std::fs::canonicalize(&path).map_err(|_| {
                    RuntimeError::new(
                        "approval_scope_invalid",
                        "approval path scope no longer exists",
                    )
                })?;
                let kind = if canonical.is_dir() {
                    GrantPathScopeKind::Subtree
                } else {
                    GrantPathScopeKind::Exact
                };
                Ok(GrantPathScope {
                    path: normalized_path(&canonical),
                    kind,
                    identity: path_identity(&canonical),
                })
            })
            .collect()
    }

    async fn wait_for_approval(
        &self,
        context: &OperationContext,
        task_id: &TaskId,
        approval_id: &str,
    ) -> RuntimeResult<()> {
        let deadline = Instant::now() + APPROVAL_TIMEOUT;

        loop {
            let row = sqlx::query(
                "SELECT state,decision_json FROM approvals WHERE id=? AND task_id=? LIMIT 1",
            )
            .bind(approval_id)
            .bind(task_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state lookup failed"))?
            .ok_or_else(|| RuntimeError::new("approval_missing", "approval request disappeared"))?;
            let state = row.get::<String, _>("state");
            let decision_json = row.get::<Option<String>, _>("decision_json");
            match state.as_str() {
                "approved" => return Ok(()),
                "rejected" => {
                    let reason = rejection_reason(decision_json.as_deref());
                    return Err(RuntimeError::new(
                        "command_rejected_by_user",
                        reason.map_or_else(
                            || "the user rejected this operation".to_owned(),
                            |value| format!("the user rejected this operation: {value}"),
                        ),
                    ));
                }
                "cancelled" => {
                    return Err(RuntimeError::new(
                        "approval_cancelled",
                        "approval was cancelled",
                    ));
                }
                "expired" => {
                    return Err(RuntimeError::new(
                        "approval_timeout",
                        "command approval timed out before the user responded",
                    ));
                }
                "pending" => {}
                _ => {
                    return Err(RuntimeError::new(
                        "approval_state_invalid",
                        "approval has an invalid state",
                    ));
                }
            }
            if Instant::now() >= deadline {
                self.expire_approval(approval_id).await?;
                return Err(RuntimeError::new(
                    "approval_timeout",
                    "command approval timed out before the user responded",
                ));
            }
            tokio::select! {
                () = context.cancellation.cancelled() => {
                    self.cancel_approval(approval_id).await?;
                    return Err(RuntimeError::new("cancelled", "operation was cancelled while waiting for approval"));
                }
                () = tokio::time::sleep(APPROVAL_POLL_INTERVAL) => {}
            }
        }
    }

    async fn expire_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "expired").await
    }

    async fn cancel_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "cancelled")
            .await
    }

    async fn resolve_waiting_approval(&self, approval_id: &str, state: &str) -> RuntimeResult<()> {
        sqlx::query("UPDATE approvals SET state=?,resolved_at_ms=? WHERE id=? AND state='pending'")
            .bind(state)
            .bind(now_ms())
            .bind(approval_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state update failed"))?;
        Ok(())
    }
}
