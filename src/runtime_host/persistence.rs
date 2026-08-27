use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, Task, TaskId, TaskSession, TaskStatus,
    TaskStore as _, TerminalEventChunk, TerminalEventStore as _, TerminalSession,
    TerminalSessionStatus, TimelineEvent, TurnBinding, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

impl RuntimeHost {
    pub(super) async fn call_persisted(
        &self,
        tool: &str,
        mut context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        self.authorize_tool(&context.agent_id, tool).await?;
        self.ensure_call_identity(&mut context).await?;
        self.append_call_event(&context, tool, "started", Some(&arguments), None, None)
            .await?;

        let result = self.dispatch(tool, context.clone(), arguments).await;
        match &result {
            Ok(output) => {
                self.append_call_event(&context, tool, "succeeded", None, Some(output), None)
                    .await?;
            }
            Err(error) => {
                self.append_call_event(&context, tool, "failed", None, None, Some(error))
                    .await?;
            }
        }
        result
    }

    async fn ensure_call_identity(&self, context: &mut OperationContext) -> RuntimeResult<()> {
        let scope = context
            .mcp_session_id
            .as_deref()
            .unwrap_or(&context.request_id);
        let logical_session = safe_id("mcp-session", &context.agent_id, scope);
        let task = context
            .task_id
            .clone()
            .unwrap_or_else(|| safe_id("task", &context.agent_id, scope));
        let turn = context.turn_id.clone().unwrap_or_else(|| {
            safe_id(
                "turn",
                &context.agent_id,
                &format!("{scope}\0{}", context.request_id),
            )
        });
        context.task_id = Some(task.clone());
        context.turn_id = Some(turn);
        context.mcp_session_id = Some(logical_session.clone());

        let task_id = TaskId::new(task).map_err(|error| invalid("taskId", error))?;
        let session_id =
            SessionId::new(logical_session).map_err(|error| invalid("sessionId", error))?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let now = now_ms();
        let generation = current.as_ref().map_or(1, |task| task.generation);
        self.repository
            .upsert_task(&Task {
                id: task_id.clone(),
                agent_id: chatcmd_core::AgentId::new(&context.agent_id).ok(),
                device_id: self.device.id.clone(),
                conversation_scope_hash: current
                    .as_ref()
                    .and_then(|task| task.conversation_scope_hash.clone()),
                title: current.as_ref().and_then(|task| task.title.clone()),
                source: Some("mcp".to_owned()),
                status: TaskStatus::Running,
                active_session_id: Some(session_id.clone()),
                generation,
                stopped_at_ms: None,
                created_at_ms: current.as_ref().map_or(now, |task| task.created_at_ms),
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        self.repository
            .upsert_task_session(&TaskSession {
                task_id: task_id.clone(),
                session_id,
                generation,
                replaced_session_id: None,
                status: TerminalSessionStatus::Running,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        let agent_id = chatcmd_core::AgentId::new(&context.agent_id)
            .map_err(|error| invalid("agentId", error))?;
        let turn_id = TurnId::new(context.turn_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("turnId", error))?;
        self.repository
            .bind_turn(&TurnBinding {
                agent_id,
                device_id: self.device.id.clone(),
                turn_id,
                task_id,
                last_used_at_ms: now,
            })
            .await
            .map_err(storage_error)
    }

    async fn append_call_event(
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
        let event_kind = if status == "started" {
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
        if let Some(value) = title.filter(|value| !value.trim().is_empty()) {
            task.title = Some(value.trim().to_owned());
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
        let turn_id = required_turn_id(context)?;
        let session_id = required_session_id(context)?;
        let event_kind = if status == "completed" {
            EventKind::Status
        } else {
            EventKind::Progress
        };
        let payload = json!({
            "tool": context.tool_name,
            "status": status,
            "content": content,
            "title": title
        });
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
        Ok(json!({ "accepted": true, "taskId": task_id.as_str(), "status": status }))
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
