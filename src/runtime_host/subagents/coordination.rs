use super::*;

impl RuntimeHost {
    #[cfg(test)]
    pub(in crate::runtime_host) async fn wait_for_subagents(
        &self,
        context: &OperationContext,
        timeout_ms: u64,
    ) -> RuntimeResult<Value> {
        self.wait_for_subagent_reports(
            context,
            &crate::runtime_host::inputs::SubagentWaitInput {
                timeout_ms,
                subagent_id: None,
                report_offset: 0,
                report_version: None,
            },
        )
        .await
    }

    pub(in crate::runtime_host) async fn wait_for_subagent_reports(
        &self,
        context: &OperationContext,
        input: &crate::runtime_host::inputs::SubagentWaitInput,
    ) -> RuntimeResult<Value> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        let deadline = Instant::now() + Duration::from_millis(input.timeout_ms.clamp(250, 40_000));
        loop {
            self.expire_stale_subagents(None).await?;
            let mut runs = self
                .subagent_values(parent_task_id, Some(parent_turn_id))
                .await?;
            let pending_count = runs.iter().filter(|r| r["status"] == "pending").count();
            let running_count = runs.iter().filter(|r| r["status"] == "running").count();
            let active_count = pending_count + running_count;
            if active_count == 0 || input.subagent_id.is_some() || Instant::now() >= deadline {
                self.attach_subagent_reports(&mut runs, input).await?;
                let counts = reports::report_counts(&runs);
                let reports_pending = counts["pending"].as_u64().unwrap_or(0) > 0;
                if active_count == 0
                    && reports_pending
                    && input.subagent_id.is_none()
                    && Instant::now() < deadline
                {
                    sleep(Duration::from_millis(100)).await;
                    continue;
                }
                let all_completed =
                    !runs.is_empty() && runs.iter().all(|r| r["status"] == "completed");
                let failed_count = runs
                    .iter()
                    .filter(|r| {
                        matches!(
                            r["status"].as_str(),
                            Some("failed" | "stopped" | "timedOut" | "interrupted")
                        )
                    })
                    .count();
                let all_work_completed = all_completed
                    && runs.iter().all(|r| {
                        r["report"]["workOutcome"] == "completed"
                            && r["report"]["workOutcomeProvenance"] == "agentDeclared"
                            && r["report"]["blockers"]
                                .as_array()
                                .is_some_and(Vec::is_empty)
                    });
                return Ok(json!({
                    "allFinished": active_count == 0, "allCompleted": all_completed,
                    "allWorkCompleted": all_work_completed,
                    "pendingCount": pending_count, "runningCount": running_count,
                    "activeCount": active_count, "failedCount": failed_count,
                    "reportPendingCount": counts["pending"], "reportMissingCount": counts["missing"],
                    "reportAvailableCount": counts["available"],
                    "partialCount": counts["partial"], "blockedCount": counts["blocked"],
                    "unknownOutcomeCount": counts["unknown"],
                    "nextPollAfterMs": if active_count == 0 && !reports_pending { 0 } else { 1000 },
                    "earliestLeaseExpiryMs": runs.iter().filter_map(|r| r["leaseExpiresAtMs"].as_i64()).min(),
                    "subagents": runs,
                    "instruction": reports::wait_instruction(active_count, reports_pending, !all_work_completed && !runs.is_empty())
                }));
            }
            if context.cancellation.is_cancelled() {
                return Err(RuntimeError::new(
                    "cancelled",
                    "sub-agent wait was cancelled",
                ));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    pub(in crate::runtime_host) async fn ensure_subagents_finished(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        self.expire_stale_subagents(None).await?;
        let runs = self
            .subagent_values(parent_task_id, Some(parent_turn_id))
            .await?;
        let active = runs
            .iter()
            .filter_map(|run| {
                let status = run.get("status").and_then(Value::as_str)?;
                is_pending_status(Some(status)).then(|| {
                    format!(
                        "{} ({status})",
                        run.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unnamed child")
                    )
                })
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Ok(());
        }
        Err(RuntimeError::new(
            "subagents_still_running",
            format!(
                "{} child agent(s) are still pending/running: {}. Call agent_subagent_wait until allFinished=true before agent_turn_complete",
                active.len(),
                active.join(", ")
            ),
        ))
    }

    pub(super) async fn subagent_values(
        &self,
        parent_task_id: &str,
        parent_turn_id: Option<&str>,
    ) -> RuntimeResult<Vec<Value>> {
        let rows = chatcmd_storage::subagent_tree::descendant_runs(
            self.repository.pool(),
            parent_task_id,
            parent_turn_id,
        )
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent tree lookup failed"))?;

        Ok(rows.iter().map(subagent_row_value).collect())
    }

    pub(super) fn publish_subagent_status(
        &self,
        parent_task_id: &str,
        parent_turn_id: &str,
        subagent_id: &str,
        child_task_id: Option<&str>,
        name: &str,
        status: &str,
    ) {
        self.publish_subagent_status_with_reason(
            parent_task_id,
            parent_turn_id,
            subagent_id,
            child_task_id,
            name,
            status,
            None,
            0,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn publish_subagent_status_with_reason(
        &self,
        parent_task_id: &str,
        parent_turn_id: &str,
        subagent_id: &str,
        child_task_id: Option<&str>,
        name: &str,
        status: &str,
        reason: Option<&str>,
        attempt: i64,
    ) {
        self.publish_event(
            format!("subagent:{subagent_id}:{status}:{}", now_ms()),
            "subagent.status",
            Some(parent_task_id.to_owned()),
            child_task_id.map(str::to_owned),
            Some(parent_turn_id.to_owned()),
            json!({
                "subagentId": subagent_id,
                "childTaskId": child_task_id,
                "name": name,
                "status": status,
                "reason": reason,
                "attempt": attempt
            }),
        );
    }
}
