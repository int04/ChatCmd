use super::*;

pub(super) fn map_device(row: &sqlx::sqlite::SqliteRow) -> Result<LocalDevice, StorageError> {
    Ok(LocalDevice {
        id: DeviceId::new(
            row.try_get::<String, _>("device_id")
                .map_err(|error| backend("map device id", error))?,
        )
        .map_err(invalid_data)?,
        installation_id: row
            .try_get("installation_id")
            .map_err(|error| backend("map installation id", error))?,
        name: row
            .try_get("name")
            .map_err(|error| backend("map device name", error))?,
        platform: row
            .try_get("platform")
            .map_err(|error| backend("map device platform", error))?,
        os_version: row
            .try_get("os_version")
            .map_err(|error| backend("map OS version", error))?,
        architecture: row
            .try_get("architecture")
            .map_err(|error| backend("map architecture", error))?,
        app_version: row
            .try_get("app_version")
            .map_err(|error| backend("map app version", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map device timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map device timestamp", error))?,
    })
}

pub(super) fn map_agent(row: &sqlx::sqlite::SqliteRow) -> Result<McpAgent, StorageError> {
    Ok(McpAgent {
        id: AgentId::new(
            row.try_get::<String, _>("id")
                .map_err(|error| backend("map agent id", error))?,
        )
        .map_err(invalid_data)?,
        name: row
            .try_get("name")
            .map_err(|error| backend("map agent name", error))?,
        enabled: row
            .try_get("enabled")
            .map_err(|error| backend("map agent state", error))?,
        project_folder: row
            .try_get("project_folder")
            .map_err(|error| backend("map project folder", error))?,
        secret_last4: row
            .try_get("secret_last4")
            .map_err(|error| backend("map secret suffix", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map agent timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map agent timestamp", error))?,
        last_used_at_ms: row
            .try_get("last_used_at_ms")
            .map_err(|error| backend("map last use", error))?,
    })
}

pub(super) fn map_task(row: &sqlx::sqlite::SqliteRow) -> Result<Task, StorageError> {
    Ok(Task {
        id: TaskId::new(
            row.try_get::<String, _>("id")
                .map_err(|error| backend("map task id", error))?,
        )
        .map_err(invalid_data)?,
        agent_id: row
            .try_get::<Option<String>, _>("agent_id")
            .map_err(|error| backend("map task agent", error))?
            .map(AgentId::new)
            .transpose()
            .map_err(invalid_data)?,
        device_id: DeviceId::new(
            row.try_get::<String, _>("device_id")
                .map_err(|error| backend("map task device", error))?,
        )
        .map_err(invalid_data)?,
        conversation_scope_hash: row
            .try_get("conversation_scope_hash")
            .map_err(|error| backend("map conversation scope", error))?,
        title: row
            .try_get("title")
            .map_err(|error| backend("map task title", error))?,
        source: row
            .try_get("source")
            .map_err(|error| backend("map task source", error))?,
        status: TaskStatus::from_str(
            &row.try_get::<String, _>("status")
                .map_err(|error| backend("map task status", error))?,
        )
        .map_err(invalid_data)?,
        active_session_id: row
            .try_get::<Option<String>, _>("active_session_id")
            .map_err(|error| backend("map active session", error))?
            .map(SessionId::new)
            .transpose()
            .map_err(invalid_data)?,
        generation: row
            .try_get("generation")
            .map_err(|error| backend("map task generation", error))?,
        stopped_at_ms: row
            .try_get("stopped_at_ms")
            .map_err(|error| backend("map task stop time", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map task timestamp", error))?,
        updated_at_ms: row
            .try_get("updated_at_ms")
            .map_err(|error| backend("map task timestamp", error))?,
    })
}

pub(super) fn map_chunk(row: &sqlx::sqlite::SqliteRow) -> Result<TerminalEventChunk, StorageError> {
    Ok(TerminalEventChunk {
        session_id: SessionId::new(
            row.try_get::<String, _>("session_id")
                .map_err(|error| backend("map chunk session", error))?,
        )
        .map_err(invalid_data)?,
        sequence: row
            .try_get("sequence")
            .map_err(|error| backend("map chunk sequence", error))?,
        event_id: EventId::new(
            row.try_get::<String, _>("event_id")
                .map_err(|error| backend("map chunk event", error))?,
        )
        .map_err(invalid_data)?,
        task_id: row
            .try_get::<Option<String>, _>("task_id")
            .map_err(|error| backend("map chunk task", error))?
            .map(TaskId::new)
            .transpose()
            .map_err(invalid_data)?,
        turn_id: row
            .try_get::<Option<String>, _>("turn_id")
            .map_err(|error| backend("map chunk turn", error))?
            .map(chatcmd_core::TurnId::new)
            .transpose()
            .map_err(invalid_data)?,
        kind: EventKind::from_str(
            &row.try_get::<String, _>("kind")
                .map_err(|error| backend("map chunk kind", error))?,
        )
        .map_err(invalid_data)?,
        stream: row
            .try_get("stream")
            .map_err(|error| backend("map chunk stream", error))?,
        payload: row
            .try_get("payload")
            .map_err(|error| backend("map chunk payload", error))?,
        payload_encoding: row
            .try_get("payload_encoding")
            .map_err(|error| backend("map chunk encoding", error))?,
        created_at_ms: row
            .try_get("created_at_ms")
            .map_err(|error| backend("map chunk timestamp", error))?,
    })
}
