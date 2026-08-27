use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, TaskId, TaskStatus, TaskStore as _,
    TerminalEventChunk, TerminalEventStore as _, TerminalSession, TerminalSessionStatus,
    TimelineEvent, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error, user_message::compact_task_title};

impl RuntimeHost {
    pub(super) async fn call_persisted(
        &self,
        tool: &str,
        mut context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        self.authorize_tool(&context.agent_id, tool).await?;
        let first_user_message = (tool == "agent_user_message")
            .then(|| arguments.get("content").and_then(Value::as_str))
            .flatten();
        self.ensure_call_identity(&mut context, first_user_message)
            .await?;
        if tool != "agent_user_message" {
            self.ensure_user_message_synced(&context).await?;
        }
        if let Err(error) = self.authorize_execution(&context, tool, &arguments).await {
            self.append_call_event(&context, tool, "failed", None, None, Some(&error))
                .await?;
            return Err(error);
        }
        let _activity_guard = self.activities.register(&context, tool, &arguments);
        self.append_call_event(&context, tool, "started", Some(&arguments), None, None)
            .await?;

        let result = tokio::select! {
            result = self.dispatch(tool, context.clone(), arguments) => result,
            () = context.cancellation.cancelled() => {
                let reason = self.activities.stop_reason(&context.request_id);
                Err(RuntimeError::new(
                    "activity_stopped",
                    reason.map_or_else(
                        || "the user stopped this activity".to_owned(),
                        |value| format!("the user stopped this activity. Reason: {value}"),
                    ),
                ))
            }
        };
        match result {
            Ok(output) => {
                self.append_call_event(&context, tool, "succeeded", None, Some(&output), None)
                    .await?;
                Ok(enrich_tool_result(output, &context, tool))
            }
            Err(error) => {
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
        let mut payload = json!({
            "activityId": context.request_id,
            "tool": tool,
            "status": status
        });
        if let Some(value) = input {
            payload["input"] = value.clone();
        }
        if let Some(value) = output {
            payload["output"] = value.clone();
        }
        if let Some(value) = error {
            payload["errorCode"] = Value::String(value.code.clone());
            payload["errorMessage"] = Value::String(value.message.clone());
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

    pub(super) async fn save_agent_event(
        &self,
        context: &OperationContext,
        status: &str,
        content: &str,
        title: Option<&str>,
    ) -> RuntimeResult<Value> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        if status == "completed" && content.trim().is_empty() {
            return Err(RuntimeError::new(
                "final_response_required",
                "agent_turn_complete content must contain the exact final user-facing response",
            ));
        }
        let now = now_ms();
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
        let key = safe_id(
            "agent-event",
            &context.agent_id,
            &format!("{}\0{status}", context.request_id),
        );
        let session_id = required_session_id(context)?;
        let event_kind = if status == "completed" {
            EventKind::Status
        } else {
            EventKind::Progress
        };
        let payload = json!({"tool": context.tool_name, "status": status, "content": content, "title": applied_title.as_deref()});
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
        object.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
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
