use super::*;

#[tokio::test]
async fn aggregate_report_text_is_bounded_and_omitted_pages_are_retrievable() {
    let (host, parent, _directory) = parent_fixture().await;
    let text = "Báo cáo 🦀\n".repeat(1600);
    let mut ids = Vec::new();
    for index in 0..7 {
        let registration = host
            .register_subagent(&parent, &format!("audit-{index}"), "independent work", None)
            .await
            .unwrap();
        let child = begin_child(&host, &parent, &registration).await;
        finish(&host, &child, &text, "completed").await;
        ids.push(registration["subagentId"].as_str().unwrap().to_owned());
    }
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let rows = result["subagents"].as_array().unwrap();
    let chars: usize = rows
        .iter()
        .map(|r| r["report"]["content"].as_str().unwrap().chars().count())
        .sum();
    assert!(chars <= 60_000);
    let omitted = rows
        .iter()
        .find(|r| r["report"]["content"] == "")
        .expect("aggregate budget omits later text");
    assert_eq!(omitted["report"]["availability"], "available");
    assert_eq!(omitted["report"]["continuation"]["reportOffset"], 0);
    let page = wait(&host, &parent, omitted["report"]["continuation"].clone()).await;
    let fetched = &run(&page, omitted["id"].as_str().unwrap())["report"];
    assert_eq!(
        fetched["content"],
        text.chars().take(12_000).collect::<String>()
    );
    assert_eq!(result["reportAvailableCount"], 7);
}

#[tokio::test]
async fn outcome_metadata_is_bound_to_the_accepted_final_event_not_an_earlier_retry() {
    let (host, parent, registration, id, _directory) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    event(&host, &child, "orphan-quality", "assistant", "status", json!({
        "status":"quality", "finalEventId":"rejected-final-event", "qualityReport":{
            "workOutcome":"completed", "workOutcomeProvenance":"agentDeclared", "verification":"passed"
        }
    }), now_ms()).await;
    finish(&host, &child, "Actual partial report", "partial").await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    assert_eq!(run(&result, &id)["report"]["workOutcome"], "partial");
    assert_eq!(run(&result, &id)["report"]["verification"], "notApplicable");
    assert_eq!(result["allWorkCompleted"], false);
}

#[tokio::test]
async fn structured_report_is_returned_verbatim_instead_of_reinterpreting_its_claims() {
    let (host, parent, registration, id, _directory) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let text = json!({"files":["src/a.rs","src/b.rs"],"symbols":["ReadA","ReadB"],"changes":[],"evidenceRefs":["unverified-text-claim"],"blockers":[],"workOutcome":"completed"}).to_string();
    finish(&host, &child, &text, "partial").await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let report = &run(&result, &id)["report"];
    assert_eq!(report["content"], text);
    assert_eq!(
        report["workOutcome"], "partial",
        "normalized metadata, not arbitrary text claims"
    );
    assert_eq!(report["evidenceRefs"], json!([]));
    assert_eq!(result["allWorkCompleted"], false);
}

#[tokio::test]
async fn legacy_default_outcome_and_terminal_conflicts_never_imply_verified_work() {
    let (host, parent, registration, id, _directory) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    Box::pin(host.call_persisted(
        "agent_turn_complete",
        child,
        json!({"content":"Legacy final report"}),
    ))
    .await
    .unwrap();
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let report = &run(&result, &id)["report"];
    assert_eq!(report["content"], "Legacy final report");
    assert_eq!(report["workOutcomeProvenance"], "legacyDefault");
    assert_eq!(result["allWorkCompleted"], false);
    let broken = host
        .register_subagent(&parent, "failed child", "work", None)
        .await
        .unwrap();
    let broken_task = broken["childTaskId"].as_str().unwrap();
    host.fail_subagent_worker(broken_task, "first failure")
        .await
        .unwrap();
    host.fail_subagent_worker(broken_task, "duplicate callback")
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='completed' WHERE id=?")
        .bind(broken_task)
        .execute(host.repository.pool())
        .await
        .unwrap();
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let failed = run(&result, broken["subagentId"].as_str().unwrap());
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["terminalReason"], "first failure");
    assert_eq!(result["failedCount"], 1);
}
