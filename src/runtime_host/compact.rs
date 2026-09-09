//! Compact's task identity fence precedes normal MCP identity creation/adoption.
use super::{RuntimeHost, now_ms, storage_error};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use chatcmd_storage::compact::{CompactCheckpoint, CompactPhase};
use serde_json::Value;

impl RuntimeHost {
    async fn settle_replacement_compact_if_ready(
        &self,
        tool: &str,
        context: &OperationContext,
        arguments: &Value,
    ) -> RuntimeResult<()> {
        if tool != "agent_user_message" || is_resume_bootstrap(arguments) {
            return Ok(());
        }
        let Some(scope) = context.conversation_scope_id.as_deref() else {
            return Ok(());
        };

        // The browser publishes a canonical destination identity while the acknowledgement
        // is still generating. A real user turn from that exact authenticated scope is
        // therefore sufficient to finish a delayed opening_new_chat checkpoint before
        // normal MCP identity selection. This closes the race where the acknowledgement
        // is visible before the extension's next 400 ms recovery tick commits `completed`.
        for _ in 0..2 {
            let candidate = sqlx::query_as::<_, (String, String, i64, String, String)>(
                r#"SELECT j.id,j.task_id,j.revision,j.new_conversation_id,j.new_conversation_url
                   FROM chatgpt_compact_jobs j
                   JOIN tasks t ON t.id=j.task_id
                   WHERE j.phase='opening_new_chat'
                     AND j.new_scope_hash=?
                     AND j.new_conversation_id IS NOT NULL
                     AND j.new_conversation_url IS NOT NULL
                     AND t.agent_id=? AND t.source='chatgpt_web'
                   ORDER BY j.updated_at_ms DESC,j.id DESC
                   LIMIT 1"#,
            )
            .bind(scope)
            .bind(&context.agent_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|error| {
                storage_error(chatcmd_core::StorageError::Backend(error.to_string()))
            })?;
            let Some((id, task_id, revision, conversation_id, conversation_url)) = candidate else {
                return Ok(());
            };
            if context
                .task_id
                .as_deref()
                .is_some_and(|expected| expected != task_id)
            {
                return Ok(());
            }

            let checkpoint = CompactCheckpoint {
                expected_revision: revision,
                phase: Some(CompactPhase::Completed),
                new_conversation_id: Some(conversation_id),
                new_conversation_url: Some(conversation_url),
                detail: Some(None),
                ..CompactCheckpoint::default()
            };
            let operation_id = format!("compact-mcp-resume-{id}-{revision}");
            match self
                .repository
                .compact_checkpoint(&id, &checkpoint, &operation_id, now_ms())
                .await
            {
                Ok(completed) => {
                    self.publish_event(
                        format!("chatgpt-compact-{}-{}", completed.id, completed.revision),
                        "chatgpt_compact_updated",
                        Some(completed.task_id.clone()),
                        None,
                        None,
                        serde_json::json!({
                            "jobId": completed.id,
                            "taskId": completed.task_id,
                            "phase": completed.phase,
                            "revision": completed.revision,
                            "updatedAtMs": completed.updated_at_ms,
                        }),
                    );
                    return Ok(());
                }
                Err(error) => {
                    let latest = self
                        .repository
                        .compact_job(&id)
                        .await
                        .map_err(storage_error)?;
                    match latest.phase {
                        CompactPhase::Completed => return Ok(()),
                        CompactPhase::OpeningNewChat => {
                            // The extension may have advanced only the CAS revision while this
                            // user call arrived. Reload once and retry the same safe transition.
                            continue;
                        }
                        CompactPhase::Cancelled => {
                            return Err(RuntimeError::new(
                                "compact_resume_cancelled",
                                "The context handoff was cancelled before this replacement conversation could resume the task.",
                            ));
                        }
                        _ => return Err(storage_error(error)),
                    }
                }
            }
        }
        Err(RuntimeError::new(
            "compact_resume_race",
            "The replacement conversation changed while ChatCMD was attaching it. No local tool was run.",
        ))
    }

    pub(super) async fn call_compact_checked(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let _call = self.activities.track_compact_call(&context)?;
        let bootstrap = tool == "agent_user_message" && is_resume_bootstrap(&arguments);
        if !bootstrap {
            self.settle_replacement_compact_if_ready(tool, &context, &arguments)
                .await?;
        }
        let scope = context.conversation_scope_id.as_deref();
        let task = context.task_id.as_deref();
        // An authenticated conversation scope wins over a transport session that
        // the provider may reuse for another chat. Session is only a fallback.
        let session = scope
            .is_none()
            .then_some(context.mcp_session_id.as_deref())
            .flatten();
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE scope_hash=? OR previous_scope_hash=? OR old_session_id=?) OR EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE phase NOT IN ('completed','cancelled') AND (task_id=? OR old_scope_hash=? OR new_scope_hash=?))"
        ).bind(scope).bind(scope).bind(session).bind(task).bind(scope).bind(scope)
            .fetch_one(self.repository.pool()).await
            .map_err(|error| storage_error(chatcmd_core::StorageError::Backend(error.to_string())))?;
        if blocked || bootstrap {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "This conversation is being compacted or is archived. No local tool was run. Write the handoff without tools; resume only in the confirmed replacement ChatGPT conversation.",
            ));
        }
        self.call_persisted(tool, context, arguments).await
    }
}

fn is_resume_bootstrap(arguments: &Value) -> bool {
    arguments
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|text| text.trim_start().starts_with("[[CHATCMD-RESUME:"))
}

#[cfg(test)]
mod tests {
    use crate::runtime_host::{ActivityRegistry, user_message_tests};
    use chatcmd_runtime::OperationContext;
    use chatcmd_storage::compact::{CompactCheckpoint, CompactPhase, openai_scope};
    use serde_json::json;

    #[test]
    fn compact_barrier_tracks_lifecycle_calls_and_duplicate_request_ids_until_both_finish() {
        let activities = ActivityRegistry::default();
        let mut context = OperationContext::new("same-request", "agent", "agent_user_message");
        context.conversation_scope_id = Some("openai:old".to_owned());
        let first = activities.track_compact_call(&context).unwrap();
        let second = activities.track_compact_call(&context).unwrap();
        assert!(!activities.compact_settled("task", Some("openai:old")));
        assert!(activities.compact_settled("other-task", Some("openai:other")));
        drop(first);
        assert!(!activities.compact_settled("task", Some("openai:old")));
        drop(second);
        assert!(activities.compact_settled("task", Some("openai:old")));
    }

    #[tokio::test]
    async fn first_real_turn_in_replacement_scope_finishes_delayed_compact_before_resume() {
        let (host, agent_id, _directory) = user_message_tests::test_host().await;
        let old_id = "11111111-1111-4111-8111-111111111111";
        let new_id = "22222222-2222-4222-8222-222222222222";
        let old_url = format!("https://chatgpt.com/c/{old_id}");
        let new_url = format!("https://chatgpt.com/c/{new_id}");
        let old_scope = openai_scope(old_id);
        let new_scope = openai_scope(new_id);

        let mut initial = OperationContext::new(
            "compact-regression-initial",
            &agent_id,
            "agent_user_message",
        );
        initial.turn_id = Some("compact-regression-turn-initial".to_owned());
        initial.conversation_scope_id = Some(old_scope.clone());
        let accepted = host
            .call_persisted(
                "agent_user_message",
                initial,
                json!({"content":"Start the compact regression"}),
            )
            .await
            .expect("create original task");
        let task_id = accepted["taskId"].as_str().expect("task id").to_owned();
        let now = crate::runtime_host::now_ms();

        sqlx::query("UPDATE tasks SET source='chatgpt_web',conversation_scope_hash=? WHERE id=?")
            .bind(&old_scope)
            .bind(&task_id)
            .execute(host.repository.pool())
            .await
            .expect("mark task as ChatGPT web");
        sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,?,NULL,?,?)")
            .bind(&task_id)
            .bind(old_id)
            .bind(&old_url)
            .bind("gpt-test")
            .bind(now)
            .bind(now)
            .execute(host.repository.pool())
            .await
            .expect("bind original ChatGPT conversation");

        let mut job = host
            .repository
            .compact_start(&task_id, "compact-regression-start", now + 1)
            .await
            .expect("start compact");
        job = host
            .repository
            .compact_checkpoint(
                &job.id,
                &CompactCheckpoint {
                    expected_revision: job.revision,
                    phase: Some(CompactPhase::SavingHandoff),
                    handoff_text: Some("FACTUAL HANDOFF".to_owned()),
                    ..CompactCheckpoint::default()
                },
                "compact-regression-save-handoff",
                now + 2,
            )
            .await
            .expect("save handoff");
        job = host
            .repository
            .compact_checkpoint(
                &job.id,
                &CompactCheckpoint {
                    expected_revision: job.revision,
                    phase: Some(CompactPhase::OpeningNewChat),
                    new_conversation_id: Some(new_id.to_owned()),
                    new_conversation_url: Some(new_url.clone()),
                    ..CompactCheckpoint::default()
                },
                "compact-regression-open-destination",
                now + 3,
            )
            .await
            .expect("reserve replacement conversation");
        assert_eq!(job.phase, CompactPhase::OpeningNewChat);

        let mut bootstrap = OperationContext::new(
            "compact-regression-bootstrap",
            &agent_id,
            "agent_user_message",
        );
        bootstrap.turn_id = Some("compact-regression-turn-bootstrap".to_owned());
        bootstrap.conversation_scope_id = Some(new_scope.clone());
        let bootstrap_error = host
            .call_compact_checked(
                "agent_user_message",
                bootstrap,
                json!({"content":format!("[[CHATCMD-RESUME:{}]]\nHandoff", job.id)}),
            )
            .await
            .expect_err("bootstrap must remain tool-free");
        assert_eq!(bootstrap_error.code, "conversation_compacting_or_archived");
        assert_eq!(
            host.repository
                .compact_job(&job.id)
                .await
                .expect("read compact job")
                .phase,
            CompactPhase::OpeningNewChat
        );

        let mut old_scope_turn = OperationContext::new(
            "compact-regression-old-scope",
            &agent_id,
            "agent_user_message",
        );
        old_scope_turn.task_id = Some(task_id.clone());
        old_scope_turn.turn_id = Some("compact-regression-turn-old-scope".to_owned());
        old_scope_turn.conversation_scope_id = Some(old_scope.clone());
        let old_scope_error = host
            .call_compact_checked(
                "agent_user_message",
                old_scope_turn,
                json!({"content":"continue from the archived source"}),
            )
            .await
            .expect_err("old source must stay fenced");
        assert_eq!(old_scope_error.code, "conversation_compacting_or_archived");

        let mut continued = OperationContext::new(
            "compact-regression-continued",
            &agent_id,
            "agent_user_message",
        );
        continued.turn_id = Some("compact-regression-turn-continued".to_owned());
        continued.conversation_scope_id = Some(new_scope.clone());
        let mut events = host.events.subscribe();
        let resumed = host
            .call_compact_checked(
                "agent_user_message",
                continued,
                json!({"content":"tiếp tục công việc"}),
            )
            .await
            .expect("replacement conversation should resume the same task");

        let compact_event = events
            .try_recv()
            .expect("compact completion realtime event");
        assert_eq!(compact_event.event_type, "chatgpt_compact_updated");
        assert_eq!(compact_event.task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(compact_event.payload["phase"], "completed");
        assert_eq!(resumed["taskId"], task_id);
        assert_eq!(
            host.repository
                .compact_job(&job.id)
                .await
                .expect("read completed compact")
                .phase,
            CompactPhase::Completed
        );
        let task_state: (Option<String>, i64, String) = sqlx::query_as(
            "SELECT conversation_scope_hash,generation,status FROM tasks WHERE id=?",
        )
        .bind(&task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read resumed task");
        assert_eq!(task_state.0.as_deref(), Some(new_scope.as_str()));
        assert_eq!(task_state.1, 2);
        assert_eq!(task_state.2, "running");
        let archived: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chatgpt_compact_archives WHERE task_id=? AND scope_hash=?",
        )
        .bind(&task_id)
        .bind(&old_scope)
        .fetch_one(host.repository.pool())
        .await
        .expect("read archive fence");
        assert_eq!(archived, 1);
    }
}
