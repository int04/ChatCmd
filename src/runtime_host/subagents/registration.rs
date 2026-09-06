use super::*;

impl RuntimeHost {
    pub(in crate::runtime_host) async fn register_subagent(
        &self,
        context: &OperationContext,
        name: &str,
        request: &str,
        approval_grant: Option<&SubagentApprovalGrantInput>,
    ) -> RuntimeResult<Value> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        let name = validate_text("name", name, MAX_SUBAGENT_NAME_CHARS)?;
        let request = validate_text("request", request, MAX_SUBAGENT_REQUEST_CHARS)?;
        validate_subagent_approval_request(approval_grant)?;
        let approval_grant_json = approval_grant
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| RuntimeError::new("invalid_arguments", "approvalGrant is invalid"))?;
        let deterministic_id = subagent_id_for_registration(
            parent_task_id,
            parent_turn_id,
            name,
            request,
            approval_grant_json.as_deref(),
        );
        let deterministic_task_id = child_task_id_for_subagent(&deterministic_id);
        // A disabled setting is a policy decision, not a queue with zero capacity.
        // Check retries before counting slots: the existing child may own the last one.
        let _slot_guard = loop {
            if context.cancellation.is_cancelled() {
                return Err(RuntimeError::new(
                    "operationCancelled",
                    "sub-agent registration cancelled",
                ));
            }
            let guard = self.subagent_registration_gate.lock().await;
            let limit = self.subagent_concurrency_limit().await?;
            if limit == 0 {
                return Err(RuntimeError::new(
                    "subagents_disabled",
                    "Sub-agents are disabled (Settings > Execution > Sub-agent count = 0). Continue in the main conversation.",
                ));
            }
            if let Some(row) =
                sqlx::query("SELECT id,child_task_id,status FROM subagent_runs WHERE id=?")
                    .bind(&deterministic_id)
                    .fetch_optional(self.repository.pool())
                    .await
                    .map_err(|_| {
                        RuntimeError::new("storage_error", "sub-agent retry lookup failed")
                    })?
            {
                return Ok(subagent_registration_value(
                    &deterministic_id,
                    &row.get::<Option<String>, _>("child_task_id")
                        .unwrap_or_else(|| deterministic_task_id.clone()),
                    name,
                    &row.get::<String, _>("status"),
                    true,
                ));
            }
            let parent_status =
                sqlx::query_scalar::<_, String>("SELECT status FROM tasks WHERE id=?")
                    .bind(parent_task_id)
                    .fetch_optional(self.repository.pool())
                    .await
                    .map_err(|_| {
                        RuntimeError::new("storage_error", "sub-agent parent lookup failed")
                    })?;
            if !matches!(parent_status.as_deref(), Some("running" | "pending")) {
                return Err(RuntimeError::new(
                    "subagent_parent_inactive",
                    "cannot delegate from a finished or stopped parent task",
                ));
            }
            let nested: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM subagent_runs WHERE child_task_id=?)",
            )
            .bind(parent_task_id)
            .fetch_one(self.repository.pool())
            .await
            .map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent parent relation unavailable")
            })?;
            let active = self.active_subagent_count().await?;
            if active < limit {
                break guard;
            }
            if nested {
                return Err(RuntimeError::new(
                    "subagent_capacity_exhausted",
                    "All global child slots are occupied. Do this delegated work locally; waiting for another child could deadlock the parent tree.",
                ));
            }
            drop(guard);
            self.wait_before_retrying_subagent_slot().await;
        };
        let now = now_ms();
        let mut transaction = self
            .repository
            .pool()
            .begin()
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent transaction failed"))?;

        if let Some(row) = sqlx::query(
            "SELECT id,child_task_id,status FROM subagent_runs WHERE parent_task_id=? AND parent_turn_id=? AND name=? AND request=? AND requested_approval_grant_json IS ? ORDER BY created_at_ms,id LIMIT 1",
        )
        .bind(parent_task_id)
        .bind(parent_turn_id)
        .bind(name)
        .bind(request)
        .bind(approval_grant_json.as_deref())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent idempotency lookup failed"))?
        {
            let subagent_id = row.get::<String, _>("id");
            let child_task_id = row
                .get::<Option<String>, _>("child_task_id")
                .unwrap_or_else(|| child_task_id_for_subagent(&subagent_id));
            let status = row.get::<String, _>("status");
            transaction.commit().await.map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent transaction commit failed")
            })?;
            return Ok(subagent_registration_value(
                &subagent_id,
                &child_task_id,
                name,
                &status,
                true,
            ));
        }

        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) SELECT ?,agent_id,device_id,NULL,?,'mcp',project_folder,'pending',NULL,1,NULL,?,? FROM tasks WHERE id=? ON CONFLICT(id) DO NOTHING")
            .bind(&deterministic_task_id)
            .bind(name)
            .bind(now)
            .bind(now)
            .bind(parent_task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent task reservation failed"))?;
        let inserted = sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,requested_approval_grant_json,status,created_at_ms,updated_at_ms,max_runtime_ms) VALUES(?,?,?,?,?,?,?,'pending',?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&deterministic_id)
            .bind(parent_task_id)
            .bind(parent_turn_id)
            .bind(&deterministic_task_id)
            .bind(name)
            .bind(request)
            .bind(approval_grant_json.as_deref())
            .bind(now)
            .bind(now)
            .bind(subagent_max_runtime_ms())
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent registration failed"))?
            .rows_affected();
        let status = if inserted == 0 {
            sqlx::query_scalar::<_, String>("SELECT status FROM subagent_runs WHERE id=?")
                .bind(&deterministic_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| {
                    RuntimeError::new("storage_error", "sub-agent duplicate lookup failed")
                })?
        } else {
            "pending".to_owned()
        };
        transaction.commit().await.map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent transaction commit failed")
        })?;

        if inserted != 0 {
            self.publish_subagent_status(
                parent_task_id,
                parent_turn_id,
                &deterministic_id,
                Some(&deterministic_task_id),
                name,
                "pending",
            );
        }
        Ok(subagent_registration_value(
            &deterministic_id,
            &deterministic_task_id,
            name,
            &status,
            inserted == 0,
        ))
    }

    pub(in crate::runtime_host) async fn delegated_subagent_task_id(
        &self,
        context: &OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<Option<String>> {
        let Some(subagent_id) = first_user_message.and_then(extract_subagent_id) else {
            return Ok(None);
        };
        let row = sqlx::query("SELECT r.child_task_id FROM subagent_runs r JOIN tasks parent ON parent.id=r.parent_task_id WHERE r.id=? AND parent.agent_id=? LIMIT 1")
            .bind(&subagent_id)
            .bind(&context.agent_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent task lookup failed"))?;
        let Some(row) = row else {
            return Err(RuntimeError::new(
                "subagent_not_found",
                "delegated sub-agent marker is invalid or belongs to another agent",
            ));
        };
        Ok(Some(
            row.get::<Option<String>, _>("child_task_id")
                .unwrap_or_else(|| child_task_id_for_subagent(&subagent_id)),
        ))
    }

    pub(in crate::runtime_host) async fn claim_subagent_from_message(
        &self,
        context: &OperationContext,
        child_task_id: &str,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let Some(subagent_id) = first_user_message.and_then(extract_subagent_id) else {
            return Ok(());
        };
        let row = sqlx::query("SELECT r.parent_task_id,r.parent_turn_id,r.name,r.child_task_id,r.status AS registered_status,r.fallback_state,r.max_runtime_ms,r.requested_approval_grant_json FROM subagent_runs r JOIN tasks parent ON parent.id=r.parent_task_id WHERE r.id=? AND parent.agent_id=? LIMIT 1")
            .bind(&subagent_id)
            .bind(&context.agent_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent claim lookup failed"))?;
        let Some(row) = row else {
            return Err(RuntimeError::new(
                "subagent_not_found",
                "delegated sub-agent marker is invalid or belongs to another agent",
            ));
        };
        let requested_approval_grant = row
            .get::<Option<String>, _>("requested_approval_grant_json")
            .map(|raw| {
                serde_json::from_str::<SubagentApprovalGrantInput>(&raw).map_err(|_| {
                    RuntimeError::new(
                        "storage_error",
                        "stored sub-agent approval grant request is invalid",
                    )
                })
            })
            .transpose()?;
        let parent_task_id = row.get::<String, _>("parent_task_id");
        if parent_task_id == child_task_id {
            return Err(RuntimeError::new(
                "invalid_subagent",
                "a task cannot be its own sub-agent",
            ));
        }
        let existing_child = row.get::<Option<String>, _>("child_task_id");
        if existing_child
            .as_deref()
            .is_some_and(|value| value != child_task_id)
        {
            return Err(RuntimeError::new(
                "subagent_already_claimed",
                "delegated sub-agent marker was already claimed by another task",
            ));
        }
        let registered_status = row.get::<String, _>("registered_status");
        if !matches!(registered_status.as_str(), "pending" | "running") {
            return Err(RuntimeError::new(
                "subagent_not_active",
                format!("delegated sub-agent is already {registered_status}"),
            ));
        }
        if registered_status == "running" {
            return self
                .heartbeat_subagent(child_task_id)
                .await?
                .then_some(())
                .ok_or_else(|| {
                    RuntimeError::new(
                        "subagent_lease_lost",
                        "delegated sub-agent is owned by another worker attempt",
                    )
                });
        }
        if self.subagent_concurrency_limit().await? == 0 {
            return Err(RuntimeError::new(
                "subagents_disabled",
                "New child agents are disabled by the user.",
            ));
        }
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let name = row.get::<String, _>("name");
        let fallback_state = row.get::<String, _>("fallback_state");
        let fallback_claim = matches!(fallback_state.as_str(), "requested" | "started");
        let now = now_ms();
        let lease_expires_at = now.saturating_add(if fallback_claim {
            chatcmd_storage::subagent_tree::BROWSER_HEARTBEAT_LEASE_MS
        } else {
            subagent_lease_ms()
        });
        let lease_expires_at =
            lease_expires_at.min(now.saturating_add(row.get::<i64, _>("max_runtime_ms")));
        let mut transaction = self.repository.pool().begin().await.map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent claim transaction failed")
        })?;
        let claimed = sqlx::query("UPDATE subagent_runs SET child_task_id=?,status='running',fallback_state=CASE WHEN fallback_state IN ('requested','started') THEN 'claimed' ELSE fallback_state END,worker_id=?,attempt=attempt+1,lease_acquired_at_ms=?,lease_expires_at_ms=?,last_heartbeat_at_ms=?,started_at_ms=?,terminal_reason=NULL,updated_at_ms=?,completed_at_ms=NULL WHERE id=? AND status='pending' AND (child_task_id IS NULL OR child_task_id=?)")
            .bind(child_task_id)
            .bind(self.subagent_worker_id.as_ref())
            .bind(now)
            .bind(lease_expires_at)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(&subagent_id)
            .bind(child_task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent claim failed"))?;
        if claimed.rows_affected() != 1 {
            transaction.rollback().await.map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent claim rollback failed")
            })?;
            let current =
                sqlx::query("SELECT child_task_id,status FROM subagent_runs WHERE id=? LIMIT 1")
                    .bind(&subagent_id)
                    .fetch_optional(self.repository.pool())
                    .await
                    .map_err(|_| {
                        RuntimeError::new("storage_error", "sub-agent claim refresh failed")
                    })?;
            if current.as_ref().is_some_and(|current| {
                current.get::<String, _>("status") == "running"
                    && current.get::<Option<String>, _>("child_task_id").as_deref()
                        == Some(child_task_id)
            }) {
                return Ok(());
            }
            let current_status = current
                .as_ref()
                .map(|current| current.get::<String, _>("status"))
                .unwrap_or_else(|| "missing".to_owned());
            return Err(RuntimeError::new(
                "subagent_not_active",
                format!("delegated sub-agent is already {current_status}"),
            ));
        }
        let attempt = sqlx::query_scalar::<_, i64>("SELECT attempt FROM subagent_runs WHERE id=?")
            .bind(&subagent_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent attempt lookup failed"))?;
        self.initialize_subagent_grant(
            &mut transaction,
            &subagent_id,
            SubagentGrantInheritance {
                owner_agent_id: &context.agent_id,
                parent_task_id: &parent_task_id,
                parent_turn_id: &parent_turn_id,
                child_task_id,
                child_turn_id: context.turn_id.as_deref(),
                child_attempt: attempt,
                lease_expires_at_ms: lease_expires_at,
            },
            requested_approval_grant.as_ref(),
        )
        .await?;
        transaction.commit().await.map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent claim transaction commit failed")
        })?;
        self.publish_subagent_status(
            &parent_task_id,
            &parent_turn_id,
            &subagent_id,
            Some(child_task_id),
            &name,
            "running",
        );
        if fallback_claim {
            self.publish_event(
                format!("subagent-fallback-claimed-{subagent_id}-{now}"),
                "subagent.fallback_claimed",
                Some(parent_task_id),
                None,
                Some(parent_turn_id),
                json!({
                    "subagentId": subagent_id,
                    "childTaskId": child_task_id,
                    "name": name
                }),
            );
        }
        Ok(())
    }
}
