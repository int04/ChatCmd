use super::*;

async fn submit(state: &Arc<AppState>, input: SubagentFallbackResult) -> Value {
    subagent_fallback_result(State(state.clone()), Path(SUBAGENT_ID.into()), Json(input))
        .await
        .unwrap()
        .0
}

async fn run_status(state: &Arc<AppState>) -> String {
    sqlx::query_scalar("SELECT status FROM subagent_runs WHERE id=?")
        .bind(SUBAGENT_ID)
        .fetch_one(state.repository.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn subagent_browser_final_rejects_missing_unstable_or_wrong_identity_evidence() {
    for fault in [
        "missing",
        "generating",
        "unstable",
        "empty-id",
        "wrong-chat",
        "wrong-url",
        "stale",
        "empty-answer",
        "large-answer",
    ] {
        let (state, _dir) = claimed_fixture().await;
        let mut input = result_input();
        let mut proof = json!({"protocol":1,"userMessageId":"u","assistantMessageId":"a","stableForMs":12000,"generating":false});
        match fault {
            "missing" => input.completion_evidence = None,
            "generating" => {
                proof["generating"] = json!(true);
                input.completion_evidence = Some(serde_json::from_value(proof).unwrap());
            }
            "unstable" => {
                proof["stableForMs"] = json!(100);
                input.completion_evidence = Some(serde_json::from_value(proof).unwrap());
            }
            "empty-id" => {
                proof["userMessageId"] = json!("");
                input.completion_evidence = Some(serde_json::from_value(proof).unwrap());
            }
            "wrong-chat" => input.conversation_id = Some("other-chat".into()),
            "wrong-url" => input.conversation_url = Some("https://example.com/c/child-chat".into()),
            "stale" => input.attempt = 2,
            "empty-answer" => input.assistant_content = Some("   ".into()),
            _ => input.assistant_content = Some("x".repeat(100_001)),
        }
        assert_eq!(submit(&state, input).await["accepted"], false, "{fault}");
        assert_eq!(run_status(&state).await, "running", "{fault}");
    }
}

#[tokio::test]
async fn subagent_browser_final_active_tool_invalidates_candidate_until_quiet() {
    let (state, _dir) = claimed_fixture().await;
    complete(&state).await;
    age_candidate(&state).await;
    let mut context =
        chatcmd_runtime::OperationContext::new("still-reading", "agent", "fs_read_text");
    context.task_id = Some(CHILD_TASK_ID.into());
    context.turn_id = Some("child-turn".into());
    let guard = state
        .activities
        .register(&context, "fs_read_text", &json!({"path":"a.rs"}))
        .unwrap();
    assert_eq!(complete(&state).await["reason"], "child_work_still_active");
    drop(guard);
    assert_eq!(complete(&state).await["reason"], "finalization_grace");
    assert_eq!(run_status(&state).await, "running");
}

#[tokio::test]
async fn subagent_browser_final_activity_and_changed_answer_restart_grace() {
    let (state, _dir) = claimed_fixture().await;
    complete(&state).await;
    age_candidate(&state).await;
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('progress',?,'child-turn','assistant','progress','progress','{}',?)")
        .bind(CHILD_TASK_ID).bind(now_ms()).execute(state.repository.pool()).await.unwrap();
    assert_eq!(complete(&state).await["reason"], "finalization_grace");
    age_candidate(&state).await;
    let mut input = result_input();
    input.assistant_content = Some("A different final report".into());
    assert_eq!(submit(&state, input).await["reason"], "finalization_grace");
    assert_eq!(run_status(&state).await, "running");
}

#[tokio::test]
async fn subagent_browser_final_does_not_overwrite_terminal_or_ambiguous_turn() {
    for fault in [
        "completed",
        "stopped",
        "failed",
        "deadline",
        "later-turn",
        "sampling",
    ] {
        let (state, _dir) = claimed_fixture().await;
        complete(&state).await;
        age_candidate(&state).await;
        match fault {
            "deadline" => {
                sqlx::query("UPDATE subagent_runs SET max_runtime_ms=1 WHERE id=?")
                    .bind(SUBAGENT_ID)
                    .execute(state.repository.pool())
                    .await
                    .unwrap();
            }
            "later-turn" => {
                sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('later-user',?,'later-turn','user','message','later-user','{}',?)").bind(CHILD_TASK_ID).bind(now_ms()).execute(state.repository.pool()).await.unwrap();
            }
            "sampling" => {
                sqlx::query("UPDATE subagent_runs SET fallback_state='none' WHERE id=?")
                    .bind(SUBAGENT_ID)
                    .execute(state.repository.pool())
                    .await
                    .unwrap();
            }
            other => {
                sqlx::query("UPDATE subagent_runs SET status=? WHERE id=?")
                    .bind(other)
                    .bind(SUBAGENT_ID)
                    .execute(state.repository.pool())
                    .await
                    .unwrap();
            }
        }
        assert_eq!(complete(&state).await["accepted"], false, "{fault}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND actor='assistant' AND json_extract(payload_json,'$.status')='completed'")
            .bind(CHILD_TASK_ID).fetch_one(state.repository.pool()).await.unwrap();
        assert_eq!(count, 0, "{fault}");
    }
}

#[tokio::test]
async fn subagent_browser_final_report_failure_rolls_back_terminal_transition() {
    let (state, _dir) = claimed_fixture().await;
    complete(&state).await;
    age_candidate(&state).await;
    sqlx::query("CREATE TRIGGER fail_browser_report BEFORE INSERT ON timeline_events WHEN NEW.actor='assistant' AND json_extract(NEW.payload_json,'$.status')='completed' BEGIN SELECT RAISE(ABORT,'injected write failure'); END")
        .execute(state.repository.pool()).await.unwrap();
    assert!(
        subagent_fallback_result(
            State(state.clone()),
            Path(SUBAGENT_ID.into()),
            Json(result_input())
        )
        .await
        .is_err()
    );
    assert_eq!(run_status(&state).await, "running");
    let task: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(task, "running");
    sqlx::query("DROP TRIGGER fail_browser_report")
        .execute(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(complete(&state).await["accepted"], true);
}

#[tokio::test]
async fn subagent_browser_final_concurrent_callbacks_create_one_report() {
    let (state, _dir) = claimed_fixture().await;
    complete(&state).await;
    age_candidate(&state).await;
    let (a, b) = tokio::join!(complete(&state), complete(&state));
    assert_ne!(a["accepted"], b["accepted"]);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND actor='assistant' AND json_extract(payload_json,'$.status')='completed'")
        .bind(CHILD_TASK_ID).fetch_one(state.repository.pool()).await.unwrap();
    assert_eq!(count, 1);
}
