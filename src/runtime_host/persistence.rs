use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, TaskId, TaskStatus, TaskStore as _,
    TerminalEventChunk, TerminalEventStore as _, TerminalSession, TerminalSessionStatus,
    TimelineEvent, TurnId,
};
use chatcmd_runtime::{
    OperationContext, RuntimeError, RuntimeResult, ToolPhase, ToolStatus, ToolUsage,
};
use serde_json::{Value, json};
use sqlx::Row as _;
use tracing::Instrument as _;
use uuid::Uuid;

use super::{
    RuntimeHost, invalid, now_ms, storage_error,
    tool_event_projection::{EventLimits, bounded_error_message, project},
    user_message::compact_task_title,
};

impl RuntimeHost {
    pub(super) async fn call_persisted(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let telemetry = self.telemetry.start(&context, tool);
        telemetry.set_phase(ToolPhase::Authorizing);
        let span = telemetry.span();
        let result = self
            .call_persisted_inner(tool, context, arguments, &telemetry)
            .instrument(span)
            .await;
        if tool.starts_with("blob_") {
            self.telemetry.set_blob_bytes(self.blob_store.usage_bytes());
        }
        let (status, error_code) = match &result {
            Ok(_) => (ToolStatus::Success, None),
            Err(error) if is_timeout_code(&error.code) => {
                (ToolStatus::Timeout, Some(error.code.as_str()))
            }
            Err(error) if is_cancel_code(&error.code) => {
                (ToolStatus::Cancelled, Some(error.code.as_str()))
            }
            Err(error) => (ToolStatus::Failure, Some(error.code.as_str())),
        };
        let usage = result
            .as_ref()
            .ok()
            .map(tool_usage_from_value)
            .unwrap_or_default();
        if result.as_ref().ok().is_some_and(tool_result_has_artifact) {
            telemetry.mark_artifact_created();
        }
        let truncated = result.as_ref().ok().is_some_and(tool_result_is_truncated);
        telemetry.finish(status, usage, error_code, truncated);
        result
    }

    async fn call_persisted_inner(
        &self,
        tool: &str,
        mut context: OperationContext,
        arguments: Value,
        telemetry: &chatcmd_runtime::ToolCallTelemetry,
    ) -> RuntimeResult<Value> {
        self.authorize_tool(&context.agent_id, tool).await?;
        let first_user_message = (tool == "agent_user_message")
            .then(|| arguments.get("content").and_then(Value::as_str))
            .flatten();
        self.ensure_call_identity(&mut context, first_user_message)
            .await?;
        telemetry.update_context(&context);
        if let Some(task_id) = context.task_id.as_deref()
            && let Err(error) = self.heartbeat_subagent(task_id).await
        {
            tracing::warn!(code = %error.code, "sub-agent activity heartbeat failed");
        }
        if tool != "agent_user_message" {
            self.ensure_user_message_synced(&context).await?;
        }
        if let Err(error) = self.authorize_execution(&context, tool, &arguments).await {
            self.append_call_event(&context, tool, "failed", None, None, Some(&error))
                .await?;
            return Err(error);
        }
        telemetry.set_phase(phase_for_tool(tool));
        let _activity_guard = self.activities.register(&context, tool, &arguments);
        self.append_call_event(&context, tool, "started", Some(&arguments), None, None)
            .await?;

        let dispatch = self.dispatch(tool, context.clone(), arguments);
        tokio::pin!(dispatch);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(10),
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
            result = &mut dispatch => break result,
            () = context.cancellation.cancelled() => break {
                let reason = self.activities.stop_reason(&context.request_id);
                // Dropping a dispatch future does not stop an active blocking worker. Continue
                // polling until it reaches a cooperative checkpoint and completes cleanup. If
                // it committed before observing cancellation, preserve the committed result.
                match dispatch.await {
                    Ok(output) => Ok(output),
                    Err(error) if error.code != "operationCancelled" && error.code != "cancelled" => Err(error),
                    Err(_) => Err(RuntimeError::new(
                        "activity_stopped",
                        reason.map_or_else(
                            || "the user stopped this activity after worker cleanup".to_owned(),
                            |value| format!("the user stopped this activity after worker cleanup. Reason: {value}"),
                        ),
                    )),
                }
            },
            _ = heartbeat.tick() => {
                if let Some(task_id) = context.task_id.as_deref()
                    && let Err(error) = self.heartbeat_subagent(task_id).await
                {
                    tracing::warn!(code = %error.code, "sub-agent timer heartbeat failed");
                }
            },
            }
        };
        match result {
            Ok(output) => {
                telemetry.update_usage(tool_usage_from_value(&output));
                telemetry.set_phase(ToolPhase::CleaningUp);
                if let Err(error) = self
                    .append_call_event(&context, tool, "succeeded", None, Some(&output), None)
                    .await
                {
                    tracing::warn!(
                        tool,
                        error_code = error.code,
                        "tool succeeded but its bounded timeline event could not be persisted"
                    );
                }
                let output = self
                    .attach_immediate_messages(&context, tool, output)
                    .await?;
                Ok(enrich_tool_result(output, &context, tool))
            }
            Err(error) => {
                telemetry.set_phase(if is_cancel_code(&error.code) {
                    ToolPhase::RollingBack
                } else {
                    ToolPhase::CleaningUp
                });
                let status = if error.code == "activity_stopped" {
                    "stopped"
                } else {
                    "failed"
                };
                self.append_call_event(&context, tool, status, None, None, Some(&error))
                    .await?;
                Err(error)
            }
        }
    }

    pub(super) async fn append_call_event(
        &self,
        context: &OperationContext,
        tool: &str,
        status: &str,
        input: Option<&Value>,
        output: Option<&Value>,
        error: Option<&RuntimeError>,
    ) -> RuntimeResult<()> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let session_id = required_session_id(context)?;
        let key = safe_id(
            "mcp-event",
            &context.agent_id,
            &format!("{}\0{status}", context.request_id),
        );
        let limits = EventLimits::default();
        let mut payload = json!({
            "activityId": context.request_id,
            "tool": tool,
            "status": status,
            "schemaVersion": 2
        });
        if let Some(value) = input {
            let projection = project(tool, value, limits);
            add_projection_metadata(&mut payload, "input", &projection);
            payload["input"] = projection.public_summary;
        }
        if let Some(value) = output {
            let projection = project(tool, value, limits);
            add_projection_metadata(&mut payload, "output", &projection);
            payload["output"] = projection.public_summary;
        }
        if let Some(value) = error {
            payload["errorCode"] = Value::String(value.code.clone());
            let (message, truncated) = bounded_error_message(&value.message, limits);
            payload["errorMessage"] = Value::String(message);
            if truncated {
                payload["errorTruncated"] = Value::Bool(true);
            }
        }
        let event_kind = if status == "started" || status == "pending_approval" {
            EventKind::ToolCall
        } else {
            EventKind::ToolResult
        };
        let task_value = task_id.as_str().to_owned();
        let turn_value = turn_id.as_str().to_owned();
        let session_value = session_id.as_str().to_owned();
        let event = TimelineEvent {
            id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
            task_id,
            turn_id: Some(turn_id),
            session_id: Some(session_id),
            actor: ActorKind::Tool,
            kind: event_kind,
            idempotency_key: key.clone(),
            payload_json: payload.to_string(),
            metadata_json: None,
            created_at_ms: now_ms(),
        };
        self.repository
            .append_timeline_events(&[event])
            .await
            .map_err(storage_error)?;
        self.publish_event(
            key,
            event_kind.as_str(),
            Some(task_value),
            Some(session_value),
            Some(turn_value),
            payload,
        );
        Ok(())
    }

    pub(super) async fn persist_shell_session(
        &self,
        context: &OperationContext,
        info: &chatcmd_runtime::ShellSessionInfo,
    ) -> RuntimeResult<()> {
        self.repository
            .upsert_terminal_session(&TerminalSession {
                id: SessionId::new(&info.session_id)
                    .map_err(|error| invalid("sessionId", error))?,
                task_id: Some(required_task_id(context)?),
                turn_id: Some(required_turn_id(context)?),
                executable: info.executable.clone(),
                working_directory: info.initial_working_directory.display().to_string(),
                columns: i32::from(info.columns),
                rows: i32::from(info.rows),
                process_id: info.process_id.map(i64::from),
                status: TerminalSessionStatus::Running,
                exit_code: info.exit_code,
                created_at_ms: i64::try_from(info.created_at_unix_ms).unwrap_or(i64::MAX),
                updated_at_ms: now_ms(),
                closed_at_ms: None,
            })
            .await
            .map_err(storage_error)
    }

    pub(super) async fn persist_shell_events(
        &self,
        context: &OperationContext,
        result: &chatcmd_runtime::ShellReadResult,
    ) -> RuntimeResult<()> {
        let session_id =
            SessionId::new(&result.session_id).map_err(|error| invalid("sessionId", error))?;
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let chunks = result
            .events
            .iter()
            .map(|event| {
                Ok(TerminalEventChunk {
                    session_id: session_id.clone(),
                    sequence: i64::try_from(event.sequence)
                        .map_err(|error| invalid("sequence", error))?,
                    event_id: EventId::new(format!("{}:{}", result.session_id, event.sequence))
                        .map_err(|error| invalid("eventId", error))?,
                    task_id: Some(task_id.clone()),
                    turn_id: Some(turn_id.clone()),
                    kind: EventKind::TerminalOutput,
                    stream: Some(event.stream.clone()),
                    payload: event.data.as_bytes().to_vec(),
                    payload_encoding: "utf-8".to_owned(),
                    created_at_ms: i64::try_from(event.timestamp_unix_ms).unwrap_or(i64::MAX),
                })
            })
            .collect::<RuntimeResult<Vec<_>>>()?;
        self.repository
            .append_terminal_chunks(&chunks)
            .await
            .map_err(storage_error)?;
        for event in &result.events {
            self.publish_event(
                format!("{}:{}", result.session_id, event.sequence),
                EventKind::TerminalOutput.as_str(),
                Some(task_id.as_str().to_owned()),
                Some(session_id.as_str().to_owned()),
                Some(turn_id.as_str().to_owned()),
                json!({ "text": event.data, "stream": event.stream, "encoding": "utf-8" }),
            );
        }
        Ok(())
    }

    pub(super) async fn update_session_status(
        &self,
        session_id: &str,
        status: &str,
        exit_code: Option<i32>,
    ) -> RuntimeResult<()> {
        let now = now_ms();
        sqlx::query("UPDATE terminal_sessions SET status=?,exit_code=?,updated_at_ms=?,closed_at_ms=CASE WHEN ? IN ('closed','exited') THEN ? ELSE closed_at_ms END WHERE id=?")
            .bind(status).bind(exit_code).bind(now).bind(status).bind(now).bind(session_id)
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "session state could not be persisted"))?;
        Ok(())
    }

    pub(super) async fn reconcile_orphaned_tool_calls(
        &self,
        task_id: &str,
        turn_id: &str,
        fallback_session_id: Option<&str>,
        reason: &str,
        created_at_ms: i64,
    ) -> RuntimeResult<usize> {
        let rows = sqlx::query(
            "SELECT json_extract(start.payload_json,'$.activityId') AS activity_id, COALESCE(MAX(json_extract(start.payload_json,'$.tool')),'tool') AS tool, MAX(start.session_id) AS session_id FROM timeline_events start WHERE start.task_id=? AND start.turn_id=? AND start.kind='tool_call' AND COALESCE(json_extract(start.payload_json,'$.activityId'),'')<>'' AND COALESCE(json_extract(start.payload_json,'$.status'),'') IN ('started','pending_approval','stop_requested') AND COALESCE(json_extract(start.payload_json,'$.tool'),'') NOT IN ('agent_user_message','agent_progress','agent_subagent_start','agent_subagent_wait','agent_turn_complete') AND NOT EXISTS (SELECT 1 FROM timeline_events terminal WHERE terminal.task_id=start.task_id AND terminal.turn_id=start.turn_id AND terminal.kind='tool_result' AND json_extract(terminal.payload_json,'$.activityId')=json_extract(start.payload_json,'$.activityId')) GROUP BY json_extract(start.payload_json,'$.activityId')",
        )
        .bind(task_id)
        .bind(turn_id)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "orphaned tool activity lookup failed"))?;
        let task = TaskId::new(task_id).map_err(|error| invalid("taskId", error))?;
        let turn = TurnId::new(turn_id).map_err(|error| invalid("turnId", error))?;
        let mut reconciled = 0_usize;
        for row in rows {
            let activity_id = row.get::<String, _>("activity_id");
            let tool = row.get::<String, _>("tool");
            let session_value = row
                .get::<Option<String>, _>("session_id")
                .or_else(|| fallback_session_id.map(str::to_owned));
            let session = session_value
                .as_deref()
                .map(SessionId::new)
                .transpose()
                .map_err(|error| invalid("sessionId", error))?;
            let key = safe_id(
                "tool-reconcile",
                "runtime",
                &format!("{task_id}\0{turn_id}\0{activity_id}"),
            );
            let payload = json!({
                "activityId": activity_id,
                "tool": tool,
                "status": "interrupted",
                "errorCode": "tool_result_missing",
                "errorMessage": "tool execution ended without a terminal result before the turn completed",
                "reconciled": true,
                "reconcileReason": reason
            });
            let inserted = self
                .repository
                .append_timeline_events(&[TimelineEvent {
                    id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                    task_id: task.clone(),
                    turn_id: Some(turn.clone()),
                    session_id: session,
                    actor: ActorKind::Tool,
                    kind: EventKind::ToolResult,
                    idempotency_key: key.clone(),
                    payload_json: payload.to_string(),
                    metadata_json: None,
                    created_at_ms,
                }])
                .await
                .map_err(storage_error)?;
            if inserted == 0 {
                continue;
            }
            reconciled += inserted;
            self.publish_event(
                key,
                EventKind::ToolResult.as_str(),
                Some(task_id.to_owned()),
                session_value,
                Some(turn_id.to_owned()),
                payload,
            );
        }
        Ok(reconciled)
    }

    pub(super) async fn save_agent_event(
        &self,
        context: &OperationContext,
        status: &str,
        content: &str,
        title: Option<&str>,
    ) -> RuntimeResult<Value> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        if status == "completed"
            && !self
                .finish_subagent_for_child(task_id.as_str(), "completed")
                .await?
        {
            return Err(RuntimeError::new(
                "subagent_lease_lost",
                "child completion rejected because another terminal transition won",
            ));
        }
        if status == "completed" && content.trim().is_empty() {
            return Err(RuntimeError::new(
                "final_response_required",
                "agent_turn_complete content must contain the exact final user-facing response",
            ));
        }
        let now = now_ms();
        let session_id = required_session_id(context)?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let mut task = current.ok_or_else(|| RuntimeError::new("not_found", "task missing"))?;
        task.status = if status == "completed" {
            TaskStatus::Completed
        } else {
            TaskStatus::Running
        };
        let title_allowed =
            status != "completed" || self.is_first_user_turn(&task_id, &turn_id).await?;
        let applied_title = title
            .filter(|value| !value.trim().is_empty())
            .filter(|_| title_allowed)
            .map(compact_task_title);
        if let Some(value) = applied_title.as_deref() {
            task.title = Some(value.to_owned());
        }
        task.updated_at_ms = now;
        self.repository
            .upsert_task(&task)
            .await
            .map_err(storage_error)?;
        if status == "completed" {
            if let Err(error) = self
                .reconcile_orphaned_tool_calls(
                    task_id.as_str(),
                    turn_id.as_str(),
                    Some(session_id.as_str()),
                    "turn_completed",
                    now,
                )
                .await
            {
                tracing::warn!(
                    task_id = %task_id,
                    turn_id = %turn_id,
                    error = ?error,
                    "failed to reconcile orphaned tool calls while completing turn"
                );
            }
        }
        let key = safe_id(
            "agent-event",
            &context.agent_id,
            &format!("{}\0{status}", context.request_id),
        );
        let event_kind = if status == "completed" {
            EventKind::Status
        } else {
            EventKind::Progress
        };
        let (file_changes, file_change_tracking_incomplete, file_change_events_dropped) =
            if status == "completed" {
                self.finish_turn_file_tracking(context).await
            } else {
                (Vec::new(), false, 0)
            };
        let payload = json!({"tool": context.tool_name, "status": status, "content": content, "title": applied_title.as_deref(),
            "fileChanges": file_changes, "fileChangeTrackingIncomplete": file_change_tracking_incomplete,
            "fileChangeEventsDropped": file_change_events_dropped, "fileChangeSchemaVersion": 2});
        self.repository
            .append_timeline_events(&[TimelineEvent {
                id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                task_id: task_id.clone(),
                turn_id: Some(turn_id.clone()),
                session_id: Some(session_id.clone()),
                actor: ActorKind::Assistant,
                kind: event_kind,
                idempotency_key: key.clone(),
                payload_json: payload.to_string(),
                metadata_json: None,
                created_at_ms: now,
            }])
            .await
            .map_err(storage_error)?;
        self.publish_event(
            key,
            event_kind.as_str(),
            Some(task_id.as_str().to_owned()),
            Some(session_id.as_str().to_owned()),
            Some(turn_id.as_str().to_owned()),
            payload,
        );
        Ok(
            json!({"accepted": true, "taskId": task_id.as_str(), "status": status, "titleUpdated": applied_title.is_some(), "title": applied_title}),
        )
    }
}

fn phase_for_tool(tool: &str) -> ToolPhase {
    if matches!(tool, "fs_search" | "fs_find" | "fs_list" | "fs_list_v2") {
        ToolPhase::Scanning
    } else if matches!(tool, "fs_read_text" | "fs_read_text_v2" | "fs_stat") {
        ToolPhase::Reading
    } else if tool.starts_with("fs_") {
        ToolPhase::Staging
    } else if tool.starts_with("git_") {
        ToolPhase::Committing
    } else if tool.starts_with("shell_") || tool.starts_with("process_") {
        ToolPhase::ProcessRunning
    } else if tool.starts_with("blob_") || tool.starts_with("task_artifact_") {
        ToolPhase::ArtifactWriting
    } else if tool == "agent_plan_question" {
        ToolPhase::WaitingApproval
    } else if tool.starts_with("agent_subagent_") {
        ToolPhase::WaitingSubagent
    } else {
        ToolPhase::Syncing
    }
}

fn tool_usage_from_value(value: &Value) -> ToolUsage {
    let mut usage: ToolUsage = value
        .get("usage")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if usage.output_bytes == 0 {
        usage.output_bytes = serde_json::to_vec(value)
            .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .unwrap_or(0);
    }
    usage
}

fn tool_result_is_truncated(value: &Value) -> bool {
    value
        .get("truncation")
        .and_then(|value| value.get("truncated"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            value
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
}

fn tool_result_has_artifact(value: &Value) -> bool {
    ["contentRef", "content_ref", "artifactRef", "artifact_ref"]
        .into_iter()
        .any(|key| value.get(key).is_some_and(|value| !value.is_null()))
}

fn is_cancel_code(code: &str) -> bool {
    matches!(
        code,
        "operationCancelled" | "cancelled" | "activity_stopped"
    )
}

fn is_timeout_code(code: &str) -> bool {
    matches!(code, "timeBudgetExceeded" | "timeout" | "timed_out")
}

fn add_projection_metadata(
    payload: &mut Value,
    direction: &str,
    projection: &super::tool_event_projection::ToolEventProjection,
) {
    payload[format!("{direction}BytesReceived")] = json!(projection.received_bytes);
    payload[format!("{direction}BytesProjected")] = json!(projection.projected_bytes);
    if projection.truncated {
        payload["payloadTruncated"] = Value::Bool(true);
    }
    if !projection.redactions.is_empty() {
        payload["redactions"] = json!(projection.redactions);
    }
}

fn enrich_tool_result(value: Value, context: &OperationContext, tool: &str) -> Value {
    let mut object = match value {
        Value::Object(object) => object,
        other => {
            let mut object = serde_json::Map::new();
            object.insert("result".to_owned(), other);
            object
        }
    };
    if let Some(task_id) = context.task_id.as_deref() {
        object.insert("taskId".to_owned(), Value::String(task_id.to_owned()));
    }
    if let Some(turn_id) = context.turn_id.as_deref() {
        object.insert("turnId".to_owned(), Value::String(turn_id.to_owned()));
    }
    if let Some(session_id) = context.mcp_session_id.as_deref() {
        object
            .entry("sessionId".to_owned())
            .or_insert_with(|| Value::String(session_id.to_owned()));
    }
    let completed = tool == "agent_turn_complete";
    object.insert("requiresFinalization".to_owned(), Value::Bool(!completed));
    if completed {
        object.insert("completed".to_owned(), Value::Bool(true));
        object.insert(
            "continuationInstruction".to_owned(),
            Value::String(
                "Reply to the user with the exact content passed to agent_turn_complete. Reuse this taskId on later turns in the same chat."
                    .to_owned(),
            ),
        );
    } else {
        object.insert(
            "finalizer".to_owned(),
            Value::String("agent_turn_complete".to_owned()),
        );
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::enrich_tool_result;
    use chatcmd_runtime::OperationContext;
    use serde_json::json;

    #[test]
    fn enrichment_preserves_tool_session_but_keeps_parent_task_correlation() {
        let mut context = OperationContext::new("request", "agent", "test_tool");
        context.task_id = Some("parent-task".to_owned());
        context.turn_id = Some("parent-turn".to_owned());
        context.mcp_session_id = Some("logical-mcp-session".to_owned());
        let result = enrich_tool_result(
            json!({
                "taskId": "child-task",
                "turnId": "child-turn",
                "sessionId": "physical-shell-session"
            }),
            &context,
            "test_tool",
        );
        assert_eq!(result["taskId"], "parent-task");
        assert_eq!(result["turnId"], "parent-turn");
        assert_eq!(result["sessionId"], "physical-shell-session");
    }

    #[test]
    fn enrichment_adds_missing_context_correlation_ids() {
        let mut context = OperationContext::new("request", "agent", "test_tool");
        context.task_id = Some("task".to_owned());
        context.turn_id = Some("turn".to_owned());
        context.mcp_session_id = Some("session".to_owned());
        let result = enrich_tool_result(json!({"accepted":true}), &context, "test_tool");
        assert_eq!(result["taskId"], "task");
        assert_eq!(result["turnId"], "turn");
        assert_eq!(result["sessionId"], "session");
    }
}

fn required_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}

fn required_turn_id(context: &OperationContext) -> RuntimeResult<TurnId> {
    TurnId::new(context.turn_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("turnId", error))
}

fn required_session_id(context: &OperationContext) -> RuntimeResult<SessionId> {
    SessionId::new(context.mcp_session_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("sessionId", error))
}

fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}
