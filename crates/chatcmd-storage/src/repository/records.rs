use super::*;

impl TaskStore for SqliteRepository {
    async fn upsert_task(&self, task: &Task) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,device_id=excluded.device_id,conversation_scope_hash=excluded.conversation_scope_hash,title=excluded.title,source=excluded.source,status=excluded.status,active_session_id=excluded.active_session_id,generation=excluded.generation,stopped_at_ms=excluded.stopped_at_ms,updated_at_ms=excluded.updated_at_ms")
            .bind(task.id.as_str()).bind(task.agent_id.as_ref().map(AgentId::as_str)).bind(task.device_id.as_str())
            .bind(&task.conversation_scope_hash).bind(&task.title).bind(&task.source).bind(task.status.as_str())
            .bind(task.active_session_id.as_ref().map(SessionId::as_str)).bind(task.generation).bind(task.stopped_at_ms)
            .bind(task.created_at_ms).bind(task.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert task", error))?;
        Ok(())
    }

    async fn task(&self, id: &TaskId) -> Result<Option<Task>, StorageError> {
        let row = sqlx::query("SELECT id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms FROM tasks WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read task", error))?;
        row.as_ref().map(map_task).transpose()
    }

    async fn list_tasks(&self, limit: u32) -> Result<Vec<Task>, StorageError> {
        let rows = sqlx::query("SELECT id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms FROM tasks ORDER BY updated_at_ms DESC,id LIMIT ?")
            .bind(i64::from(limit.clamp(1, 1000)))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list tasks", error))?;
        rows.iter().map(map_task).collect()
    }

    async fn upsert_task_session(&self, session: &TaskSession) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO task_sessions(task_id,session_id,generation,replaced_session_id,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(task_id,session_id) DO UPDATE SET generation=excluded.generation,replaced_session_id=excluded.replaced_session_id,status=excluded.status,updated_at_ms=excluded.updated_at_ms")
            .bind(session.task_id.as_str()).bind(session.session_id.as_str()).bind(session.generation)
            .bind(session.replaced_session_id.as_ref().map(SessionId::as_str)).bind(session.status.as_str())
            .bind(session.created_at_ms).bind(session.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert task session", error))?;
        Ok(())
    }

    async fn bind_turn(&self, binding: &TurnBinding) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO turn_bindings(agent_id,device_id,turn_id,task_id,last_used_at_ms) VALUES(?,?,?,?,?) ON CONFLICT(agent_id,device_id,turn_id) DO UPDATE SET task_id=excluded.task_id,last_used_at_ms=excluded.last_used_at_ms")
            .bind(binding.agent_id.as_str()).bind(binding.device_id.as_str()).bind(binding.turn_id.as_str())
            .bind(binding.task_id.as_str()).bind(binding.last_used_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("bind turn", error))?;
        Ok(())
    }

    async fn save_approval(&self, approval: &Approval) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO approvals(id,task_id,session_id,state,request_json,decision_json,created_at_ms,resolved_at_ms) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state,decision_json=excluded.decision_json,resolved_at_ms=excluded.resolved_at_ms")
            .bind(&approval.id).bind(approval.task_id.as_str()).bind(approval.session_id.as_ref().map(SessionId::as_str))
            .bind(approval.state.as_str()).bind(&approval.request_json).bind(&approval.decision_json)
            .bind(approval.created_at_ms).bind(approval.resolved_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("save approval", error))?;
        Ok(())
    }

    async fn set_execution_mode(&self, mode: &TaskExecutionMode) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,?,?) ON CONFLICT(task_id) DO UPDATE SET mode=excluded.mode,updated_at_ms=excluded.updated_at_ms")
            .bind(mode.task_id.as_str()).bind(mode.mode.as_str()).bind(mode.updated_at_ms)
            .execute(&self.pool).await.map_err(|error| backend("set task execution mode", error))?;
        Ok(())
    }
}

impl TerminalEventStore for SqliteRepository {
    async fn upsert_terminal_session(&self, session: &TerminalSession) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO terminal_sessions(id,task_id,turn_id,executable,working_directory,columns,rows,process_id,status,exit_code,created_at_ms,updated_at_ms,closed_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET task_id=excluded.task_id,turn_id=excluded.turn_id,executable=excluded.executable,working_directory=excluded.working_directory,columns=excluded.columns,rows=excluded.rows,process_id=excluded.process_id,status=excluded.status,exit_code=excluded.exit_code,updated_at_ms=excluded.updated_at_ms,closed_at_ms=excluded.closed_at_ms")
            .bind(session.id.as_str()).bind(session.task_id.as_ref().map(TaskId::as_str)).bind(session.turn_id.as_ref().map(chatcmd_core::TurnId::as_str))
            .bind(&session.executable).bind(&session.working_directory).bind(session.columns).bind(session.rows)
            .bind(session.process_id).bind(session.status.as_str()).bind(session.exit_code).bind(session.created_at_ms)
            .bind(session.updated_at_ms).bind(session.closed_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("upsert terminal session", error))?;
        Ok(())
    }

    async fn append_terminal_chunks(
        &self,
        chunks: &[TerminalEventChunk],
    ) -> Result<usize, StorageError> {
        if chunks.len() > 250 {
            return Err(StorageError::InvalidData(
                "event batch exceeds 250 chunks".to_owned(),
            ));
        }
        self.append_chunk_batch(chunks).await
    }

    async fn terminal_chunks(
        &self,
        session_id: &SessionId,
        after_sequence: Option<i64>,
        limit: u32,
    ) -> Result<Vec<TerminalEventChunk>, StorageError> {
        let rows = sqlx::query("SELECT session_id,sequence,event_id,task_id,turn_id,kind,stream,payload,payload_encoding,created_at_ms FROM terminal_event_chunks WHERE session_id=? AND sequence>? ORDER BY sequence LIMIT ?")
            .bind(session_id.as_str()).bind(after_sequence.unwrap_or(-1)).bind(i64::from(limit.clamp(1, 1000)))
            .fetch_all(&self.pool).await.map_err(|error| backend("read terminal chunks", error))?;
        rows.iter().map(map_chunk).collect()
    }

    async fn append_timeline_events(
        &self,
        events: &[TimelineEvent],
    ) -> Result<usize, StorageError> {
        if events.len() > 250 {
            return Err(StorageError::InvalidData(
                "timeline batch exceeds 250 events".to_owned(),
            ));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin timeline batch", error))?;
        let mut inserted = 0_usize;
        for event in events {
            inserted += usize::try_from(sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?)")
                .bind(event.id.as_str()).bind(event.task_id.as_str()).bind(event.turn_id.as_ref().map(chatcmd_core::TurnId::as_str))
                .bind(event.session_id.as_ref().map(SessionId::as_str)).bind(event.actor.as_str()).bind(event.kind.as_str())
                .bind(&event.idempotency_key).bind(&event.payload_json).bind(&event.metadata_json).bind(event.created_at_ms)
                .execute(&mut *transaction).await.map_err(|error| map_sqlx_conflict("append timeline event", error))?.rows_affected())
                .map_err(|error| backend("convert affected row count", error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit timeline batch", error))?;
        Ok(inserted)
    }
}
