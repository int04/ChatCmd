use super::*;

#[path = "subagent_browser_completion_guard_tests.rs"]
mod guards;

async fn claimed_fixture() -> (Arc<AppState>, TempDir) {
    let (state, directory) = fixture().await;
    let _ = subagent_fallback_started(
        State(state.clone()),
        Path(SUBAGENT_ID.into()),
        Json(SubagentFallbackStarted {
            attempt: 1,
            conversation_id: Some("child-chat".into()),
            conversation_url: Some("https://chatgpt.com/c/child-chat".into()),
        }),
    )
    .await
    .unwrap();
    let now = now_ms();
    sqlx::query("UPDATE subagent_runs SET status='running',fallback_state='claimed',worker_id='worker',attempt=1,started_at_ms=?,lease_expires_at_ms=?,max_runtime_ms=1800000 WHERE id=?")
        .bind(now - 60_000).bind(now + 180_000).bind(SUBAGENT_ID)
        .execute(state.repository.pool()).await.unwrap();
    sqlx::query("UPDATE tasks SET status='running' WHERE id=?")
        .bind(CHILD_TASK_ID)
        .execute(state.repository.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('child-user',?,'child-turn','user','message','child-user',?,?)")
        .bind(CHILD_TASK_ID).bind(json!({"tool":"agent_user_message","content":"Read files"}).to_string())
        .bind(now - 50_000).execute(state.repository.pool()).await.unwrap();
    (state, directory)
}

fn result_input() -> SubagentFallbackResult {
    serde_json::from_value(json!({"attempt":1,"status":"completed",
        "conversationId":"child-chat","conversationUrl":"https://chatgpt.com/c/child-chat",
        "assistantContent":"Read both files. Final report.",
        "completionEvidence":{"protocol":1,"userMessageId":"user-dom-1",
            "assistantMessageId":"answer-dom-1","stableForMs":12000,"generating":false}
    }))
    .unwrap()
}

async fn complete(state: &Arc<AppState>) -> Value {
    subagent_fallback_result(
        State(state.clone()),
        Path(SUBAGENT_ID.into()),
        Json(result_input()),
    )
    .await
    .unwrap()
    .0
}

#[tokio::test]
async fn subagent_browser_started_after_mcp_claim_only_binds_identity() {
    let (state, _dir) = claimed_fixture().await;
    sqlx::query("DELETE FROM chatgpt_conversations WHERE task_id=?")
        .bind(CHILD_TASK_ID)
        .execute(state.repository.pool())
        .await
        .unwrap();
    let input = || SubagentFallbackStarted {
        attempt: 1,
        conversation_id: Some("child-chat".into()),
        conversation_url: Some("https://chatgpt.com/c/child-chat".into()),
    };
    let result = subagent_fallback_started(
        State(state.clone()),
        Path(SUBAGENT_ID.into()),
        Json(input()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result["identityOnly"], true);
    let run = sqlx::query(
        "SELECT status,fallback_state,attempt,lease_expires_at_ms FROM subagent_runs WHERE id=?",
    )
    .bind(SUBAGENT_ID)
    .fetch_one(state.repository.pool())
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("status"), "running");
    assert_eq!(run.get::<String, _>("fallback_state"), "claimed");
    assert_eq!(run.get::<i64, _>("attempt"), 1);
    assert!(run.get::<i64, _>("lease_expires_at_ms") > now_ms());
    let mut wrong = input();
    wrong.conversation_id = Some("another".into());
    assert!(
        subagent_fallback_started(State(state.clone()), Path(SUBAGENT_ID.into()), Json(wrong))
            .await
            .is_err()
    );
    complete(&state).await;
    age_candidate(&state).await;
    assert_eq!(complete(&state).await["accepted"], true);
}

#[tokio::test]
async fn subagent_browser_final_waits_for_descendant_and_revokes_grants() {
    let (state, _dir) = claimed_fixture().await;
    sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms) VALUES('nested-run',?,'child-turn',NULL,'nested','read','pending',?,?)")
        .bind(CHILD_TASK_ID).bind(now_ms()).bind(now_ms()).execute(state.repository.pool()).await.unwrap();
    assert_eq!(complete(&state).await["reason"], "child_work_still_active");
    sqlx::query("UPDATE subagent_runs SET status='completed' WHERE id='nested-run'")
        .execute(state.repository.pool())
        .await
        .unwrap();
    let agent: String = sqlx::query_scalar("SELECT agent_id FROM tasks WHERE id=?")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,turn_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read,expires_at_ms,catalog_hash,state,created_at_ms,updated_at_ms) VALUES('child-grant',?,?,'child-turn','[]','[]','{}',1,1,100,?,'test','active',?,?)")
        .bind(agent).bind(CHILD_TASK_ID).bind(now_ms()+60000).bind(now_ms()).bind(now_ms()).execute(state.repository.pool()).await.unwrap();
    complete(&state).await;
    age_candidate(&state).await;
    assert_eq!(complete(&state).await["completed"], true);
    let grant: String =
        sqlx::query_scalar("SELECT state FROM approval_grants WHERE id='child-grant'")
            .fetch_one(state.repository.pool())
            .await
            .unwrap();
    assert_eq!(grant, "revoked");
}

async fn age_candidate(state: &Arc<AppState>) {
    sqlx::query("UPDATE timeline_events SET created_at_ms=? WHERE event_id=?")
        .bind(now_ms() - 20_000)
        .bind(format!("subagent-browser-final:{SUBAGENT_ID}:1"))
        .execute(state.repository.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_mcp_finish_recovers_only_after_stable_browser_candidate() {
    let (state, _dir) = claimed_fixture().await;
    let first = complete(&state).await;
    assert_eq!(first["accepted"], false);
    assert_eq!(first["reason"], "finalization_grace");
    assert_eq!(complete(&state).await["accepted"], false);
    age_candidate(&state).await;
    let result = complete(&state).await;
    assert_eq!(result["accepted"], true);
    assert_eq!(result["completed"], true);
    assert_eq!(result["completionSource"], "browserFinal");
    assert_eq!(result["mcpFinalizerReceived"], false);
    let report = chatcmd_storage::subagent_report::report_page(
        state.repository.pool(),
        SUBAGENT_ID,
        0,
        12000,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report["source"], "browserFinal");
    assert_eq!(report["turnId"], "child-turn");
    assert_eq!(report["verification"], "unknown");
    assert_eq!(report["content"], "Read both files. Final report.");
    complete(&state).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND actor='assistant' AND json_extract(payload_json,'$.status')='completed'")
        .bind(CHILD_TASK_ID).fetch_one(state.repository.pool()).await.unwrap();
    assert_eq!(count, 1);
}
