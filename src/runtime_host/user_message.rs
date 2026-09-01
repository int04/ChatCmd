use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, TaskId, TaskStore as _, TerminalEventStore as _,
    TimelineEvent, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

impl RuntimeHost {
    pub(super) async fn ensure_user_message_synced(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let found = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='user' AND kind='message' LIMIT 1)",
        )
        .bind(task_id.as_str())
        .bind(turn_id.as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "user message sync state unavailable"))?;
        if found == 1 {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "user_message_sync_required",
                "call agent_user_message first with the exact current user message and the same turnId before using any other ChatCMD tool",
            ))
        }
    }

    pub(super) async fn task_user_path_scopes(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Vec<PathBuf>> {
        let Some(task_id) = context
            .task_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(Vec::new());
        };
        let task_id = TaskId::new(task_id).map_err(|error| invalid("taskId", error))?;
        let payloads = sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' ORDER BY created_at_ms,event_id",
        )
        .bind(task_id.as_str())
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "user path grants unavailable"))?;
        let mut scopes = BTreeSet::new();
        for payload in payloads {
            let content = serde_json::from_str::<Value>(&payload)
                .ok()
                .and_then(|value| {
                    value
                        .get("content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            for path in extract_explicit_absolute_paths(&content) {
                scopes.insert(path);
                if scopes.len() >= 32 {
                    break;
                }
            }
            if scopes.len() >= 32 {
                break;
            }
        }
        Ok(scopes.into_iter().collect())
    }

    async fn bind_project_folder_from_user_message(
        &self,
        task_id: &TaskId,
        content: &str,
    ) -> RuntimeResult<()> {
        let Some(mut task) = self.repository.task(task_id).await.map_err(storage_error)? else {
            return Err(RuntimeError::new("not_found", "task missing"));
        };
        if task
            .project_folder
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(());
        }
        let paths = extract_explicit_absolute_paths(content);
        if paths.len() != 1 {
            return Ok(());
        }
        let path = &paths[0];
        let folder = if path.is_dir() {
            Some(path.clone())
        } else if path.is_file() {
            path.parent().map(std::path::Path::to_path_buf)
        } else {
            None
        };
        let Some(folder) = folder else {
            return Ok(());
        };
        task.project_folder = Some(folder.to_string_lossy().into_owned());
        task.updated_at_ms = now_ms();
        self.repository
            .upsert_task(&task)
            .await
            .map_err(storage_error)
    }

    pub(super) async fn save_user_message(
        &self,
        context: &OperationContext,
        content: &str,
    ) -> RuntimeResult<Value> {
        if content.trim().is_empty() {
            return Err(RuntimeError::new(
                "user_message_required",
                "agent_user_message content must contain the exact current user message",
            ));
        }
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let session_id = required_session_id(context)?;
        let first_turn_before = self.first_user_turn(&task_id).await?;
        let provisional_title = compact_task_title(content);
        let is_first_candidate = first_turn_before.is_none();
        let key = safe_id(
            "user-message",
            &context.agent_id,
            &format!("{}\0{}", task_id.as_str(), turn_id.as_str()),
        );
        let payload = json!({
            "tool": context.tool_name,
            "role": "user",
            "content": content,
            "title": is_first_candidate.then_some(provisional_title.as_str())
        });
        let inserted = self
            .repository
            .append_timeline_events(&[TimelineEvent {
                id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                task_id: task_id.clone(),
                turn_id: Some(turn_id.clone()),
                session_id: Some(session_id.clone()),
                actor: ActorKind::User,
                kind: EventKind::Message,
                idempotency_key: key.clone(),
                payload_json: payload.to_string(),
                metadata_json: None,
                created_at_ms: now_ms(),
            }])
            .await
            .map_err(storage_error)?;
        if inserted == 0 {
            let existing = sqlx::query_scalar::<_, String>(
                "SELECT payload_json FROM timeline_events WHERE event_id=? LIMIT 1",
            )
            .bind(&key)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "user message payload unavailable"))?;
            if existing
                .as_deref()
                .is_none_or(|value| !same_user_message(value, content))
            {
                return Err(RuntimeError::new(
                    "turn_user_message_conflict",
                    "the current turnId is already bound to a different user message; use a new turnId for each user message",
                ));
            }
        }
        self.bind_project_folder_from_user_message(&task_id, content)
            .await?;
        self.begin_turn_file_tracking(context).await;

        let first_turn = first_turn_before.or_else(|| Some(turn_id.as_str().to_owned()));
        let is_first_message = first_turn.as_deref() == Some(turn_id.as_str());
        if is_first_message {
            self.apply_initial_task_title(&task_id, &provisional_title)
                .await?;
        }
        if inserted > 0 {
            self.publish_event(
                key,
                EventKind::Message.as_str(),
                Some(task_id.as_str().to_owned()),
                Some(session_id.as_str().to_owned()),
                Some(turn_id.as_str().to_owned()),
                payload,
            );
        }
        let project_folder = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?
            .and_then(|task| task.project_folder)
            .filter(|folder| !folder.trim().is_empty());
        Ok(json!({
            "accepted": true,
            "duplicate": inserted == 0,
            "userMessageSynced": true,
            "planMode": is_plan_mode_request(content),
            "isFirstMessage": is_first_message,
            "suggestedTitleRequired": is_first_message,
            "provisionalTitle": is_first_message.then_some(provisional_title),
            "projectFolder": project_folder,
            "taskId": task_id.as_str(),
            "turnId": turn_id.as_str(),
            "toolRecovery": {
                "catalogIsStable": true,
                "hostMayLazyLoadSchemas": true,
                "missingSchemaDoesNotMeanMissingTool": true,
                "mustDiscoverBeforeUnavailableReply": true,
                "mustContinueInSameTurn": true,
                "chatGptDiscoveryHint": "If a needed ChatCMD tool schema is not visible in this turn, use the host connector/resource discovery mechanism (for example api_tool.list_resources) on the current connector with a focused query such as fs_, shell_, git_, skill, task, or agent, then continue the work without asking the user to resend the request.",
                "recommendedQueries": ["fs_", "shell_", "git_", "skill", "task", "agent"]
            }
        }))
    }

    pub(super) async fn is_first_user_turn(
        &self,
        task_id: &TaskId,
        turn_id: &TurnId,
    ) -> RuntimeResult<bool> {
        Ok(self.first_user_turn(task_id).await?.as_deref() == Some(turn_id.as_str()))
    }

    async fn first_user_turn(&self, task_id: &TaskId) -> RuntimeResult<Option<String>> {
        sqlx::query_scalar::<_, String>(
            "SELECT turn_id FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' AND turn_id IS NOT NULL ORDER BY created_at_ms,event_id LIMIT 1",
        )
        .bind(task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "first user turn lookup failed"))
    }

    async fn apply_initial_task_title(&self, task_id: &TaskId, title: &str) -> RuntimeResult<()> {
        let Some(mut task) = self.repository.task(task_id).await.map_err(storage_error)? else {
            return Err(RuntimeError::new("not_found", "task missing"));
        };
        if task.title.as_deref().is_none_or(str::is_empty) {
            task.title = Some(title.to_owned());
            task.updated_at_ms = now_ms();
            self.repository
                .upsert_task(&task)
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }
}

pub(super) fn compact_task_title(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let title = chars.by_ref().take(77).collect::<String>();
    if chars.next().is_some() {
        format!("{title}…")
    } else {
        title
    }
}

fn is_plan_mode_request(content: &str) -> bool {
    let normalized = content
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.contains("lên kế hoạch")
        || normalized.contains("lập kế hoạch")
        || normalized.contains("#plan")
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

fn extract_explicit_absolute_paths(content: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut quoted = None::<(char, usize)>;
    for (index, ch) in content.char_indices() {
        if matches!(ch, '`' | '"' | '\'') {
            if let Some((delimiter, start)) = quoted {
                if delimiter == ch {
                    candidates.push(&content[start..index]);
                    quoted = None;
                }
            } else {
                quoted = Some((ch, index + ch.len_utf8()));
            }
        }
    }
    candidates.extend(content.split_whitespace());

    let mut unique = BTreeSet::new();
    for candidate in candidates {
        let cleaned = candidate.trim_matches(|ch: char| {
            matches!(
                ch,
                '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        });
        if cleaned.is_empty() {
            continue;
        }
        let path = PathBuf::from(cleaned);
        if !path.is_absolute() || !path.exists() {
            continue;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if canonical.parent().is_none() {
            continue;
        }
        unique.insert(canonical);
        if unique.len() >= 16 {
            break;
        }
    }
    unique.into_iter().collect()
}

fn same_user_message(payload_json: &str, content: &str) -> bool {
    serde_json::from_str::<Value>(payload_json)
        .ok()
        .and_then(|payload| {
            payload
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|existing| existing == content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_user_message_must_match_exact_content() {
        let payload = json!({"role":"user","content":"xin chào"}).to_string();
        assert!(same_user_message(&payload, "xin chào"));
        assert!(!same_user_message(&payload, "xin chào!"));
    }

    #[test]
    fn user_message_key_is_stable_per_turn_and_changes_between_turns() {
        let first = safe_id("user-message", "agent", "task-a\0turn-a");
        assert_eq!(first, safe_id("user-message", "agent", "task-a\0turn-a"));
        assert_ne!(first, safe_id("user-message", "agent", "task-a\0turn-b"));
    }

    #[test]
    fn first_message_title_is_compact_and_bounded() {
        assert_eq!(
            compact_task_title("  sửa   lỗi git diff  "),
            "sửa lỗi git diff"
        );
        assert!(compact_task_title(&"x".repeat(100)).chars().count() <= 78);
    }

    #[test]
    fn plan_mode_detects_explicit_planning_triggers_only() {
        assert!(is_plan_mode_request("Lên kế hoạch cho tôi mua quà"));
        assert!(is_plan_mode_request("LẬP   KẾ HOẠCH\nwebsite bán hàng"));
        assert!(is_plan_mode_request("Xây website giúp tôi #PLAN"));
        assert!(!is_plan_mode_request("Cho tôi xem kế hoạch hiện tại"));
        assert!(!is_plan_mode_request("Dùng planner để theo dõi công việc"));
    }
}
