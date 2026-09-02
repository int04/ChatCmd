use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct TaskListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
    #[serde(rename = "projectFolder")]
    project_folder: Option<String>,
}

pub(super) async fn tasks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<Value>, Problem> {
    let mut all_tasks = state
        .repository
        .list_tasks(500)
        .await
        .map_err(storage_problem)?;
    if let Some(project_folder) = query
        .project_folder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        all_tasks.retain(|task| {
            task.project_folder
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(project_folder))
        });
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 50) as usize;
    let start = query
        .cursor
        .as_deref()
        .and_then(|cursor| all_tasks.iter().position(|task| task.id.as_str() == cursor))
        .map_or(0, |index| index + 1);
    let has_more = start.saturating_add(limit) < all_tasks.len();
    let tasks = all_tasks
        .into_iter()
        .skip(start)
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| tasks.last().map(|task| task.id.as_str().to_owned()))
        .flatten();
    let summary_rows = sqlx::query("SELECT timeline_events.task_id,COUNT(DISTINCT timeline_events.turn_id) AS turn_count,(SELECT COALESCE(json_extract(latest.payload_json,'$.content'),json_extract(latest.payload_json,'$.message'),json_extract(latest.payload_json,'$.text'),json_extract(latest.payload_json,'$.response')) FROM timeline_events latest WHERE latest.task_id=timeline_events.task_id AND COALESCE(json_extract(latest.payload_json,'$.content'),json_extract(latest.payload_json,'$.message'),json_extract(latest.payload_json,'$.text'),json_extract(latest.payload_json,'$.response')) IS NOT NULL ORDER BY latest.created_at_ms DESC,latest.event_id DESC LIMIT 1) AS output_preview FROM timeline_events GROUP BY timeline_events.task_id")
        .fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let summaries = summary_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("task_id"),
                (
                    row.get::<i64, _>("turn_count"),
                    row.get::<Option<String>, _>("output_preview"),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let final_rows = sqlx::query("SELECT task_id,COUNT(*) AS final_response_count FROM timeline_events WHERE actor='assistant' AND kind='status' AND json_extract(payload_json,'$.status')='completed' GROUP BY task_id")
        .fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let final_counts = final_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("task_id"),
                row.get::<i64, _>("final_response_count"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let items = tasks
        .into_iter()
        .map(|task| {
            let id = task.id.as_str().to_owned();
            let mut value = task_value(task);
            if let Some(object) = value.as_object_mut() {
                let (turn_count, preview) = summaries.get(&id).cloned().unwrap_or((0, None));
                object.insert("turnCount".to_owned(), json!(turn_count));
                object.insert(
                    "finalResponseCount".to_owned(),
                    json!(final_counts.get(&id).copied().unwrap_or(0)),
                );
                if let Some(preview) = preview.filter(|value| !value.trim().is_empty()) {
                    object.insert(
                        "outputPreview".to_owned(),
                        Value::String(compact_preview(&preview)),
                    );
                }
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items, "nextCursor": next_cursor })))
}

pub(super) async fn pending_conversation_approvals(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query(
        "SELECT id FROM tasks WHERE allow_execute IS NULL ORDER BY created_at_ms ASC,id ASC",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.get::<String, _>("id");
        let task = state
            .repository
            .task(&TaskId::new(&id).map_err(|_| bad_id())?)
            .await
            .map_err(storage_problem)?
            .ok_or_else(not_found)?;
        let approval_deadline_ms = task.created_at_ms.saturating_add(60_000);
        let mut value = task_value(task);
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "approvalDeadlineUtc".to_owned(),
                Value::String(iso_ms(approval_deadline_ms)),
            );
        }
        items.push(value);
    }
    Ok(Json(Value::Array(items)))
}

pub(super) async fn pending_activity_approvals(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query(
        "SELECT id,task_id,request_json,created_at_ms FROM approvals WHERE state='pending' ORDER BY created_at_ms ASC,id ASC",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let items = rows
        .into_iter()
        .map(|row| {
            let activity_id = row.get::<String, _>("id");
            let task_id = row.get::<String, _>("task_id");
            let created_at_ms = row.get::<i64, _>("created_at_ms");
            let request = serde_json::from_str::<Value>(&row.get::<String, _>("request_json"))
                .unwrap_or(Value::Null);
            json!({
                "activityId": activity_id,
                "taskId": task_id,
                "turnId": request.get("turnId").and_then(Value::as_str),
                "tool": request.get("tool").and_then(Value::as_str).unwrap_or("tool"),
                "input": request.get("input").cloned().unwrap_or(Value::Null),
                "createdAtUtc": iso_ms(created_at_ms),
                "approvalDeadlineUtc": iso_ms(created_at_ms.saturating_add(120_000)),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(Value::Array(items)))
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskDetailQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

pub(super) async fn task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<TaskDetailQuery>,
) -> Result<Json<Value>, Problem> {
    task_detail_page(
        &state,
        &id,
        query.cursor.as_deref(),
        query.limit.unwrap_or(2).clamp(1, 20) as usize,
    )
    .await
}

pub(super) async fn task_activity(
    State(state): State<Arc<AppState>>,
    Path((id, activity_id)): Path<(String, String)>,
) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query("SELECT event_id,kind,payload_json FROM timeline_events WHERE task_id=? AND kind IN ('tool_call','tool_result') AND (event_id=? OR json_extract(payload_json,'$.activityId')=?) ORDER BY created_at_ms ASC,event_id ASC")
        .bind(&id)
        .bind(&activity_id)
        .bind(&activity_id)
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    if rows.is_empty() {
        return Err(not_found());
    }
    let mut detail = serde_json::Map::new();
    for row in rows {
        let kind = row.get::<String, _>("kind");
        let payload = serde_json::from_str::<Value>(&row.get::<String, _>("payload_json"))
            .unwrap_or(Value::Null);
        let Some(payload) = payload.as_object() else {
            continue;
        };
        if kind == "tool_call" {
            if let Some(value) = payload.get("input") {
                detail.insert("input".to_owned(), value.clone());
            }
        }
        if kind == "tool_result" {
            if let Some(value) = payload.get("output") {
                detail.insert("output".to_owned(), value.clone());
            }
        }
        for key in [
            "status",
            "errorCode",
            "errorMessage",
            "errorDetails",
            "details",
        ] {
            if let Some(value) = payload.get(key) {
                let target = if key == "details" {
                    "errorDetails"
                } else {
                    key
                };
                detail.insert(target.to_owned(), value.clone());
            }
        }
        if let Some(value) = payload.get("error") {
            if value.is_string() {
                detail.insert("error".to_owned(), value.clone());
            } else if !detail.contains_key("errorDetails") {
                detail.insert("errorDetails".to_owned(), value.clone());
            }
        }
    }
    Ok(Json(Value::Object(detail)))
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskTitleInput {
    title: String,
}

pub(super) async fn set_task_title(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<TaskTitleInput>,
) -> Result<Json<Value>, Problem> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 160 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid title",
            "title must contain between 1 and 160 characters",
        ));
    }
    let affected = sqlx::query("UPDATE tasks SET title=? WHERE id=?")
        .bind(title)
        .bind(&id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?
        .rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    let mut event = AppEvent::new("conversation.title_updated", json!({ "title": title }));
    event.task_id = Some(id.clone());
    state.publish(event);
    task_detail(&state, &id).await
}

pub(super) async fn task_action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<Value>, Problem> {
    if action == "stop" {
        return stop_conversation(&state, &id).await;
    }
    if matches!(action.as_str(), "approve-execution" | "reject-execution") {
        let allow_execute = action == "approve-execution";
        let status = if allow_execute { "pending" } else { "failed" };
        let affected = sqlx::query("UPDATE tasks SET allow_execute=?,status=?,updated_at_ms=? WHERE id=? AND allow_execute IS NULL")
            .bind(allow_execute)
            .bind(status)
            .bind(now_ms())
            .bind(&id)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?
            .rows_affected();
        if affected == 0 {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id=?")
                .bind(&id)
                .fetch_one(state.repository.pool())
                .await
                .map_err(db_problem)?;
            if exists == 0 {
                return Err(not_found());
            }
        } else {
            let mut event = AppEvent::new(
                "conversation.approval_resolved",
                json!({ "allowExecute": allow_execute }),
            );
            event.task_id = Some(id.clone());
            state.publish(event);
        }
        return task_detail(&state, &id).await;
    }
    let status = match action.as_str() {
        "retry" | "resume" => "pending",
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid action",
                "unsupported task action",
            ));
        }
    };
    let affected = sqlx::query("UPDATE tasks SET status=?,stopped_at_ms=CASE WHEN ?='stopped' THEN ? ELSE NULL END,updated_at_ms=? WHERE id=?")
        .bind(status).bind(status).bind(now_ms()).bind(now_ms()).bind(&id).execute(state.repository.pool()).await.map_err(db_problem)?.rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    task_detail(&state, &id).await
}

pub(super) async fn task_detail(state: &Arc<AppState>, id: &str) -> Result<Json<Value>, Problem> {
    task_detail_page(state, id, None, 2).await
}

async fn task_detail_page(
    state: &Arc<AppState>,
    id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Json<Value>, Problem> {
    let task = state
        .repository
        .task(&TaskId::new(id).map_err(|_| bad_id())?)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    let (subagent_parent, subagents) = task_subagent_data(state, id).await?;
    let execution_mode_task_id = subagent_parent
        .as_ref()
        .map_or(id, |parent| parent.task_id.as_str());
    let mut task_value = task_value(task);
    if let (Some(parent), Some(object)) = (subagent_parent.as_ref(), task_value.as_object_mut()) {
        object.insert("isSubagent".to_owned(), Value::Bool(true));
        object.insert(
            "parentTaskId".to_owned(),
            Value::String(parent.task_id.clone()),
        );
        object.insert(
            "parentTurnId".to_owned(),
            Value::String(parent.turn_id.clone()),
        );
        object.insert("agentName".to_owned(), Value::String(parent.name.clone()));
    }

    let cursor = cursor.map(parse_turn_cursor).transpose()?;
    let fetch_limit = limit.saturating_add(1) as i64;
    let turn_rows = if let Some((cursor_ms, cursor_turn)) = cursor.as_ref() {
        sqlx::query("SELECT turn_id,MAX(created_at_ms) AS sort_ms FROM timeline_events WHERE task_id=? AND turn_id IS NOT NULL GROUP BY turn_id HAVING MAX(created_at_ms) < ? OR (MAX(created_at_ms)=? AND turn_id < ?) ORDER BY sort_ms DESC,turn_id DESC LIMIT ?")
            .bind(id).bind(*cursor_ms).bind(*cursor_ms).bind(cursor_turn).bind(fetch_limit).fetch_all(state.repository.pool()).await.map_err(db_problem)?
    } else {
        sqlx::query("SELECT turn_id,MAX(created_at_ms) AS sort_ms FROM timeline_events WHERE task_id=? AND turn_id IS NOT NULL GROUP BY turn_id ORDER BY sort_ms DESC,turn_id DESC LIMIT ?")
            .bind(id).bind(fetch_limit).fetch_all(state.repository.pool()).await.map_err(db_problem)?
    };
    let has_more = turn_rows.len() > limit;
    let selected_turns = turn_rows
        .iter()
        .take(limit)
        .map(|row| row.get::<String, _>("turn_id"))
        .collect::<Vec<_>>();
    let next_cursor = if has_more {
        turn_rows.get(limit.saturating_sub(1)).map(|row| {
            format_turn_cursor(
                row.get::<i64, _>("sort_ms"),
                &row.get::<String, _>("turn_id"),
            )
        })
    } else {
        None
    };

    let rows = fetch_timeline_rows(state, id, &selected_turns).await?;
    let terminal_rows = fetch_terminal_rows(state, id, &selected_turns).await?;
    let mut ordered_events = rows
        .iter()
        .map(|row| {
            let timestamp = row.get::<i64, _>("created_at_ms");
            let event_id = row.get::<String, _>("event_id");
            (timestamp, event_id, timeline_row(row))
        })
        .collect::<Vec<_>>();
    ordered_events.extend(terminal_rows.iter().map(|row| {
        let timestamp = row.get::<i64, _>("created_at_ms");
        let event_id = row.get::<String, _>("event_id");
        let text = String::from_utf8_lossy(&row.get::<Vec<u8>, _>("payload")).into_owned();
        (timestamp, event_id.clone(), json!({
            "id": event_id,
            "type": row.get::<String, _>("kind"),
            "occurredAt": iso_ms(timestamp),
            "turnId": row.get::<Option<String>, _>("turn_id"),
            "sessionId": row.get::<Option<String>, _>("session_id"),
            "payload": { "text": text, "stream": row.get::<Option<String>, _>("stream"), "encoding": row.get::<String, _>("payload_encoding") }
        }))
    }));
    ordered_events.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let events = ordered_events
        .into_iter()
        .map(|(_, _, value)| value)
        .collect::<Vec<_>>();
    let subagent_approvals = pending_subagent_approvals(state, id).await?;
    let execution_mode_id = TaskId::new(execution_mode_task_id).map_err(|_| bad_id())?;
    Ok(Json(json!({
        "task": task_value,
        "turns": [],
        "events": events,
        "nextCursor": next_cursor,
        "subagents": subagents,
        "subagentApprovals": subagent_approvals,
        "executionMode": execution_mode_name(state.repository.execution_mode(Some(&execution_mode_id)).await.map_err(storage_problem)?),
        "executionModeSourceTaskId": execution_mode_task_id
    })))
}

async fn fetch_timeline_rows(
    state: &Arc<AppState>,
    task_id: &str,
    turn_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, Problem> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT event_id,turn_id,session_id,kind,payload_json,created_at_ms FROM timeline_events WHERE task_id=",
    );
    query.push_bind(task_id).push(" AND turn_id IN (");
    let mut separated = query.separated(",");
    for turn_id in turn_ids {
        separated.push_bind(turn_id);
    }
    separated.push_unseparated(") ORDER BY created_at_ms ASC,event_id ASC");
    query
        .build()
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)
}

async fn fetch_terminal_rows(
    state: &Arc<AppState>,
    task_id: &str,
    turn_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, Problem> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT event_id,turn_id,session_id,kind,stream,payload,payload_encoding,created_at_ms FROM terminal_event_chunks WHERE task_id=",
    );
    query.push_bind(task_id).push(" AND turn_id IN (");
    let mut separated = query.separated(",");
    for turn_id in turn_ids {
        separated.push_bind(turn_id);
    }
    separated.push_unseparated(") ORDER BY created_at_ms ASC,event_id ASC");
    query
        .build()
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)
}

fn format_turn_cursor(timestamp: i64, turn_id: &str) -> String {
    format!("{timestamp}:{turn_id}")
}
fn parse_turn_cursor(cursor: &str) -> Result<(i64, String), Problem> {
    let (timestamp, turn_id) = cursor.split_once(':').ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid cursor",
            "task history cursor is invalid",
        )
    })?;
    let timestamp = timestamp.parse::<i64>().map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid cursor",
            "task history cursor is invalid",
        )
    })?;
    if turn_id.trim().is_empty() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid cursor",
            "task history cursor is invalid",
        ));
    }
    Ok((timestamp, turn_id.to_owned()))
}

fn compact_preview(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let preview = chars.by_ref().take(180).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn task_value(task: chatcmd_core::Task) -> Value {
    let approval_deadline_utc = task
        .allow_execute
        .is_none()
        .then(|| iso_ms(task.created_at_ms.saturating_add(60_000)));
    json!({"id":task.id.as_str(),"title":task.title,"source":task.source,"projectFolder":task.project_folder,"allowExecute":task.allow_execute,"approvalDeadlineUtc":approval_deadline_utc,"status":task.status.as_str(),"updatedAtUtc":iso_ms(task.updated_at_ms),"createdAtUtc":iso_ms(task.created_at_ms),"generation":task.generation,"activeSessionId":task.active_session_id.map(|id|id.into_string())})
}
pub(super) fn timeline_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    let kind = row.get::<String, _>("kind");
    let payload =
        serde_json::from_str::<Value>(&row.get::<String, _>("payload_json")).unwrap_or(Value::Null);
    let payload = compact_tool_payload(&kind, payload);
    json!({"id":row.get::<String,_>("event_id"),"type":kind,"occurredAt":iso_ms(row.get("created_at_ms")),"turnId":row.get::<Option<String>,_>("turn_id"),"sessionId":row.get::<Option<String>,_>("session_id"),"payload":payload})
}

fn compact_tool_payload(kind: &str, payload: Value) -> Value {
    let Some(mut object) = payload.as_object().cloned() else {
        return payload;
    };
    if kind == "tool_call" {
        if let Some(input) = object.get("input").and_then(Value::as_object) {
            let mut summary = serde_json::Map::new();
            for key in [
                "path",
                "workingDirectory",
                "query",
                "command",
                "source",
                "destination",
                "name",
                "pattern",
            ] {
                if let Some(value) = input.get(key) {
                    summary.insert(key.to_owned(), value.clone());
                }
            }
            object.insert("input".to_owned(), Value::Object(summary));
        }
    } else if kind == "tool_result" {
        object.remove("output");
        object.remove("errorDetails");
        object.remove("details");
        if object.get("error").is_some_and(|value| !value.is_string()) {
            object.remove("error");
        }
    }
    Value::Object(object)
}
