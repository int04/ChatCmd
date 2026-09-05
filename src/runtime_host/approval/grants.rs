impl RuntimeHost {
    async fn consume_safe_read_grant(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<bool> {
        if arguments
            .get("includeIgnored")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || arguments
                .get("includeHidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Ok(false);
        }
        let task_id = context.task_id.as_deref().unwrap_or_default();
        let now = now_ms();
        sqlx::query("UPDATE approval_grants SET state='expired',updated_at_ms=? WHERE task_id=? AND state='active' AND expires_at_ms<=?")
            .bind(now).bind(task_id).bind(now).execute(self.repository.pool()).await.map_err(|_| RuntimeError::new("storage_error", "approval grant expiry failed"))?;
        let rows = sqlx::query("SELECT id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read FROM approval_grants WHERE task_id=? AND owner_agent_id=? AND state='active' AND expires_at_ms>? AND catalog_hash=? AND child_attempt IS (SELECT attempt FROM subagent_runs WHERE child_task_id=? LIMIT 1) ORDER BY created_at_ms DESC")
            .bind(task_id).bind(&context.agent_id).bind(now).bind(catalog_hash()).bind(task_id).fetch_all(self.repository.pool()).await.map_err(|_| RuntimeError::new("storage_error", "approval grant lookup failed"))?;
        let paths = extract_paths(tool, arguments)?;
        let charge = requested_charge(tool, arguments);
        for row in rows {
            let tools: Vec<String> =
                serde_json::from_str(&row.get::<String, _>("allowed_tools_json"))
                    .unwrap_or_default();
            if !tools.iter().any(|value| value == tool) {
                continue;
            }
            let option_constraints = row.get::<String, _>("option_constraints_json");
            if !option_constraints_match(arguments, &option_constraints) {
                record_grant_denial(
                    self.repository.pool(),
                    &row.get::<String, _>("id"),
                    task_id,
                    tool,
                    paths.len(),
                    "option constraints mismatch",
                )
                .await?;
                continue;
            }
            let scopes: Vec<GrantPathScope> =
                serde_json::from_str(&row.get::<String, _>("path_scopes_json")).unwrap_or_default();
            if paths.iter().any(|path| !path_allowed(path, &scopes)) {
                record_grant_denial(
                    self.repository.pool(),
                    &row.get::<String, _>("id"),
                    task_id,
                    tool,
                    paths.len(),
                    "path scope mismatch",
                )
                .await?;
                continue;
            }
            let id = row.get::<String, _>("id");
            let mut transaction = self.repository.pool().begin().await.map_err(|_| {
                RuntimeError::new("storage_error", "approval grant transaction failed")
            })?;
            let affected = sqlx::query("UPDATE approval_grants SET used_calls=used_calls+1,used_files_scanned=used_files_scanned+?,used_bytes_read=used_bytes_read+?,updated_at_ms=?,state=CASE WHEN used_calls+1>=max_calls THEN 'exhausted' ELSE state END WHERE id=? AND state='active' AND used_calls+1<=max_calls AND (max_files_scanned IS NULL OR used_files_scanned+?<=max_files_scanned) AND (max_bytes_read IS NULL OR used_bytes_read+?<=max_bytes_read)")
                .bind(charge.files).bind(charge.bytes_read).bind(now).bind(&id).bind(charge.files).bind(charge.bytes_read).execute(&mut *transaction).await.map_err(|_| RuntimeError::new("storage_error", "approval grant budget consumption failed"))?.rows_affected();
            if affected == 1 {
                sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,tool,path_count,calls,files_scanned,bytes_read,bytes_written,created_at_ms) VALUES(?,?,?,'used',?,?,1,?,?,0,?)")
                    .bind(Uuid::new_v4().to_string()).bind(&id).bind(task_id).bind(tool)
                    .bind(i64::try_from(paths.len()).unwrap_or(i64::MAX)).bind(charge.files).bind(charge.bytes_read).bind(now)
                    .execute(&mut *transaction).await.map_err(|_| RuntimeError::new("storage_error", "approval grant audit failed"))?;
                transaction.commit().await.map_err(|_| {
                    RuntimeError::new("storage_error", "approval grant transaction failed")
                })?;
                return Ok(true);
            }
            transaction.rollback().await.map_err(|_| {
                RuntimeError::new("storage_error", "approval grant transaction failed")
            })?;
            record_grant_denial(
                self.repository.pool(),
                &id,
                task_id,
                tool,
                paths.len(),
                "resource budget exhausted",
            )
            .await?;
        }
        Ok(false)
    }

    pub(super) async fn inherit_subagent_approval_grant(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        inheritance: SubagentGrantInheritance<'_>,
        request: &SubagentApprovalGrantInput,
    ) -> RuntimeResult<String> {
        let SubagentGrantInheritance {
            owner_agent_id,
            parent_task_id,
            parent_turn_id,
            child_task_id,
            child_turn_id,
            child_attempt,
            lease_expires_at_ms,
        } = inheritance;
        let max_calls = i64::try_from(request.max_calls).map_err(|_| {
            RuntimeError::new("invalid_arguments", "approvalGrant maxCalls is too large")
        })?;
        let max_files_scanned = i64::try_from(request.max_files_scanned).map_err(|_| {
            RuntimeError::new(
                "invalid_arguments",
                "approvalGrant maxFilesScanned is too large",
            )
        })?;
        let max_bytes_read = i64::try_from(request.max_bytes_read).map_err(|_| {
            RuntimeError::new(
                "invalid_arguments",
                "approvalGrant maxBytesRead is too large",
            )
        })?;
        let mut requested_tools = request.allowed_tools.clone();
        requested_tools.sort();
        requested_tools.dedup();
        if requested_tools.len() != request.allowed_tools.len()
            || requested_tools.iter().any(|tool| {
                let capabilities = tool_capabilities(tool);
                !capabilities.approval_required || !capabilities.risk_class.is_safe_read()
            })
        {
            return Err(RuntimeError::new(
                "approval_grant_inheritance_denied",
                "child approval grants may contain only distinct approval-required safe-read tools",
            ));
        }
        let mut requested_scopes = Vec::with_capacity(request.path_scopes.len());
        for path in &request.path_scopes {
            let canonical = std::fs::canonicalize(path).map_err(|_| {
                RuntimeError::new(
                    "approval_grant_inheritance_denied",
                    "child approval grant path does not exist",
                )
            })?;
            let kind = if canonical.is_dir() {
                GrantPathScopeKind::Subtree
            } else {
                GrantPathScopeKind::Exact
            };
            requested_scopes.push(GrantPathScope {
                path: normalized_path(&canonical),
                kind,
                identity: path_identity(&canonical),
            });
        }
        requested_scopes.sort_by(|left, right| left.path.cmp(&right.path));
        requested_scopes.dedup_by(|left, right| left.path == right.path);
        if requested_scopes.len() != request.path_scopes.len() {
            return Err(RuntimeError::new(
                "approval_grant_inheritance_denied",
                "child approval grant path scopes must be distinct after canonicalization",
            ));
        }

        let now = now_ms();
        let rows = sqlx::query("SELECT id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,used_calls,max_files_scanned,used_files_scanned,max_bytes_read,used_bytes_read,expires_at_ms FROM approval_grants WHERE owner_agent_id=? AND task_id=? AND state='active' AND expires_at_ms>? AND catalog_hash=? AND (turn_id IS NULL OR turn_id=?) ORDER BY created_at_ms DESC,id")
        .bind(owner_agent_id)
        .bind(parent_task_id)
        .bind(now)
        .bind(catalog_hash())
        .bind(parent_turn_id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| RuntimeError::new("storage_error", "parent approval grant lookup failed"))?;
        for row in rows {
            let parent_tools: Vec<String> =
                serde_json::from_str(&row.get::<String, _>("allowed_tools_json"))
                    .unwrap_or_default();
            if requested_tools
                .iter()
                .any(|tool| !parent_tools.iter().any(|parent| parent == tool))
            {
                continue;
            }
            let parent_scopes: Vec<GrantPathScope> =
                serde_json::from_str(&row.get::<String, _>("path_scopes_json")).unwrap_or_default();
            if requested_scopes.iter().any(|scope| {
                let requested = Path::new(&scope.path);
                !path_allowed(requested, &parent_scopes)
            }) {
                continue;
            }
            let parent_id = row.get::<String, _>("id");
            let parent_expires_at = row.get::<i64, _>("expires_at_ms");
            let child_expires_at = parent_expires_at.min(lease_expires_at_ms);
            if child_expires_at <= now {
                continue;
            }
            let affected = sqlx::query("UPDATE approval_grants SET used_calls=used_calls+?,used_files_scanned=used_files_scanned+?,used_bytes_read=used_bytes_read+?,updated_at_ms=?,state=CASE WHEN used_calls+?>=max_calls THEN 'exhausted' ELSE state END WHERE id=? AND state='active' AND expires_at_ms>? AND catalog_hash=? AND used_calls+?<=max_calls AND (max_files_scanned IS NULL OR used_files_scanned+?<=max_files_scanned) AND (max_bytes_read IS NULL OR used_bytes_read+?<=max_bytes_read)")
            .bind(max_calls)
            .bind(max_files_scanned)
            .bind(max_bytes_read)
            .bind(now)
            .bind(max_calls)
            .bind(&parent_id)
            .bind(now)
            .bind(catalog_hash())
            .bind(max_calls)
            .bind(max_files_scanned)
            .bind(max_bytes_read)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "parent approval grant reservation failed"))?
            .rows_affected();
            if affected != 1 {
                continue;
            }
            let child_grant_id = Uuid::new_v4().to_string();
            let allowed_tools_json = serde_json::to_string(&requested_tools).map_err(|_| {
                RuntimeError::new("storage_error", "child approval tool serialization failed")
            })?;
            let path_scopes_json = serde_json::to_string(&requested_scopes).map_err(|_| {
                RuntimeError::new("storage_error", "child approval path serialization failed")
            })?;
            let option_constraints_json = row.get::<String, _>("option_constraints_json");
            sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE task_id=? AND state='active' AND child_attempt IS NOT ?")
            .bind(now)
            .bind(child_task_id)
            .bind(child_attempt)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "stale child approval grant revoke failed"))?;
            sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,turn_id,child_attempt,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read,max_bytes_written,expires_at_ms,inherited_from,catalog_hash,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,NULL,?,?,?,'active',?,?)")
            .bind(&child_grant_id)
            .bind(owner_agent_id)
            .bind(child_task_id)
            .bind(child_turn_id)
            .bind(child_attempt)
            .bind(&allowed_tools_json)
            .bind(&path_scopes_json)
            .bind(&option_constraints_json)
            .bind(max_calls)
            .bind(max_files_scanned)
            .bind(max_bytes_read)
            .bind(child_expires_at)
            .bind(&parent_id)
            .bind(catalog_hash())
            .bind(now)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child approval grant creation failed"))?;
            sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,calls,files_scanned,bytes_read,reason,created_at_ms) VALUES(?,?,?,'used',?,?,?, ?,?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&parent_id)
            .bind(parent_task_id)
            .bind(max_calls)
            .bind(max_files_scanned)
            .bind(max_bytes_read)
            .bind(format!("reserved for inherited child grant {child_grant_id}"))
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "parent approval grant reservation audit failed"))?;
            sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,path_count,reason,created_at_ms) VALUES(?,?,?,'created',?,?,?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&child_grant_id)
            .bind(child_task_id)
            .bind(i64::try_from(requested_scopes.len()).unwrap_or(i64::MAX))
            .bind(format!("inherited from parent grant {parent_id}"))
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child approval grant audit failed"))?;
            return Ok(child_grant_id);
        }
        Err(RuntimeError::new(
            "approval_grant_inheritance_denied",
            "requested child approval grant is not a bounded intersection of an active parent grant",
        ))
    }
}
