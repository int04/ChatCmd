use super::*;

#[path = "report_edge_cases.rs"]
mod edge_cases;

fn begin_child<'a>(
    host: &'a RuntimeHost,
    parent: &'a OperationContext,
    registration: &'a Value,
) -> chatcmd_runtime::BoxFuture<'a, OperationContext> {
    Box::pin(async move {
        let id = registration["subagentId"].as_str().expect("run id");
        let child = registration["childTaskId"].as_str().expect("child id");
        let mut context = OperationContext::new(
            format!("report-user-{id}"),
            &parent.agent_id,
            "agent_user_message",
        );
        context.task_id = Some(child.to_owned());
        context.turn_id = Some(format!("turn-{id}"));
        let synced = Box::pin(host.call_persisted("agent_user_message", context.clone(),
            json!({"content": format!("Read exactly two unchanged files\nCMDGPT_SUBAGENT_ID={id}")})))
            .await.expect("child user message");
        assert_eq!(synced["taskId"], child);
        context.mcp_session_id = Some(synced["sessionId"].as_str().unwrap().to_owned());
        context.request_id = format!("report-final-{id}");
        context.tool_name = "agent_turn_complete".to_owned();
        context
    })
}

fn finish<'a>(
    host: &'a RuntimeHost,
    child: &'a OperationContext,
    text: &'a str,
    outcome: &'a str,
) -> chatcmd_runtime::BoxFuture<'a, ()> {
    Box::pin(async move {
        Box::pin(host.call_persisted(
            "agent_turn_complete",
            child.clone(),
            json!({
                "content": text, "workOutcome": outcome, "verificationIntent": "notApplicable",
                "verificationReason": "Read-only audit; no build or test was requested."
            }),
        ))
        .await
        .expect("real child finalization");
    })
}

async fn wait(host: &RuntimeHost, parent: &OperationContext, args: Value) -> Value {
    wait_result(host, parent.clone(), args)
        .await
        .expect("wait dispatch")
}

fn wait_result(
    host: &RuntimeHost,
    parent: OperationContext,
    args: Value,
) -> chatcmd_runtime::BoxFuture<'_, chatcmd_runtime::RuntimeResult<Value>> {
    // The full dispatcher has large debug futures; don't embed one per assertion in the test stack.
    Box::pin(host.dispatch("agent_subagent_wait", parent, args))
}

fn run<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["id"] == id)
        .expect("child row")
}

async fn event(
    host: &RuntimeHost,
    child: &OperationContext,
    id: &str,
    actor: &str,
    kind: &str,
    payload: Value,
    at: i64,
) {
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,?,?,?,?,?)")
        .bind(id).bind(&child.task_id).bind(&child.turn_id).bind(actor).bind(kind).bind(id).bind(payload.to_string()).bind(at)
        .execute(host.repository.pool()).await.unwrap();
}

#[tokio::test]
async fn wait_returns_exact_public_report_and_quality_after_real_child_completion() {
    let (host, parent, registration, id, dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let text = "src/a.rs: chịu trách nhiệm đọc dữ liệu; symbols: read_a.\n\nsrc/b.rs: kiểm tra đường dẫn; symbols: validate_b.\nKhông sửa file. 🦀";
    finish(&host, &child, text, "completed").await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let report = &run(&result, &id)["report"];
    assert_eq!(report["content"], text);
    assert_eq!(report["availability"], "available");
    assert_eq!(report["workOutcome"], "completed");
    assert_eq!(report["workOutcomeProvenance"], "agentDeclared");
    assert_eq!(report["verification"], "notApplicable");
    assert_eq!(report["verificationIsChildSnapshot"], true);
    assert_eq!(report["source"], "mcpFinal");
    assert_eq!(report["truncated"], false);
    assert_eq!(result["allWorkCompleted"], true);
    // Reopen SQLite independently: delivery must not depend on worker memory or rereading sources.
    let (reopened, _) = chatcmd_storage::SqliteRepository::open(&dir.path().join("chatcmd.db"), 1)
        .await
        .unwrap();
    let page = chatcmd_storage::subagent_report::report_page(reopened.pool(), &id, 0, 12000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page["content"], text);
}

#[tokio::test]
async fn lifecycle_completed_does_not_upgrade_partial_or_blocked_work() {
    for outcome in ["partial", "blocked"] {
        let (host, parent, registration, id, _dir) = fallback_fixture().await;
        let child = begin_child(&host, &parent, &registration).await;
        finish(&host, &child, "Only one file could be inspected.", outcome).await;
        let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
        assert_eq!(result["allFinished"], true);
        assert_eq!(result["allCompleted"], true);
        assert_eq!(result["allWorkCompleted"], false);
        assert_eq!(run(&result, &id)["report"]["workOutcome"], outcome);
        assert_eq!(result[format!("{outcome}Count")], 1);
        assert!(
            result["instruction"]
                .as_str()
                .unwrap()
                .contains("not proof")
        );
    }
}

#[tokio::test]
async fn nested_startup_failure_is_visible_with_reason_even_if_parent_completes() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let nested = host
        .register_subagent(&child, "nested audit", "Read the two files", None)
        .await
        .unwrap();
    let nested_id = nested["subagentId"].as_str().unwrap();
    let nested_task = nested["childTaskId"].as_str().unwrap();
    host.fail_subagent_worker(nested_task, "child startup failed: test failure")
        .await
        .unwrap();
    finish(
        &host,
        &child,
        "I completed both file audits locally after my child failed.",
        "completed",
    )
    .await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    assert_eq!(result["allFinished"], true);
    assert_eq!(result["allCompleted"], false);
    assert_eq!(result["allWorkCompleted"], false);
    assert_eq!(result["failedCount"], 1);
    assert_eq!(run(&result, &id)["status"], "completed");
    assert!(
        run(&result, &id)["report"]["content"]
            .as_str()
            .unwrap()
            .contains("locally")
    );
    assert_eq!(
        run(&result, nested_id)["report"]["availability"],
        "unavailable"
    );
    assert_eq!(
        run(&result, nested_id)["terminalReason"],
        "child startup failed: test failure"
    );
    assert_eq!(run(&result, nested_id)["rootTurnId"], PARENT_TURN_ID);
}

#[tokio::test]
async fn wait_returns_grandchild_report_to_root_without_duplicate_or_cross_turn_content() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let nested = host
        .register_subagent(&child, "nested report", "subproblem", None)
        .await
        .unwrap();
    let nested_id = nested["subagentId"].as_str().unwrap();
    let grandchild = begin_child(&host, &child, &nested).await;
    finish(
        &host,
        &grandchild,
        "Grandchild concrete findings",
        "completed",
    )
    .await;
    finish(&host, &child, "Integrated findings", "completed").await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    assert_eq!(result["subagents"].as_array().unwrap().len(), 2);
    assert_eq!(
        run(&result, nested_id)["report"]["content"],
        "Grandchild concrete findings"
    );
    assert_eq!(
        run(&result, &id)["report"]["content"],
        "Integrated findings"
    );
    let mut other_turn = parent.clone();
    other_turn.turn_id = Some("unrelated-parent-turn".into());
    let denied = wait_result(&host, other_turn, json!({"subagentId":nested_id}))
        .await
        .unwrap_err();
    assert_eq!(denied.code, "subagent_not_found");
    let mut unrelated = parent.clone();
    unrelated.task_id = Some("unrelated-parent-task".into());
    assert_eq!(
        wait_result(&host, unrelated, json!({"subagentId":id}))
            .await
            .unwrap_err()
            .code,
        "subagent_not_found"
    );
}

#[tokio::test]
async fn unicode_report_pages_round_trip_and_stale_or_invalid_cursors_are_rejected() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let text = "Dữ liệu 🦀 e\u{301}\r\n".repeat(2500);
    finish(&host, &child, &text, "completed").await;
    let mut result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    let mut collected = String::new();
    loop {
        let report = &run(&result, &id)["report"];
        collected.push_str(report["content"].as_str().unwrap());
        assert!(report["content"].as_str().unwrap().chars().count() <= 12000);
        let next = report["continuation"].clone();
        if next.is_null() {
            break;
        }
        result = wait(&host, &parent, next).await;
    }
    assert_eq!(collected, text);
    for (args, code) in [
        (
            json!({"subagentId":id,"reportOffset":1}),
            "invalid_arguments",
        ),
        (
            json!({"reportOffset":1,"reportVersion":"x"}),
            "invalid_arguments",
        ),
        (
            json!({"subagentId":id,"reportOffset":1,"reportVersion":"stale"}),
            "subagent_report_changed",
        ),
        (
            json!({"subagentId":id,"reportOffset":u64::MAX,"reportVersion":"x"}),
            "invalid_arguments",
        ),
    ] {
        assert_eq!(
            wait_result(&host, parent.clone(), args)
                .await
                .unwrap_err()
                .code,
            code
        );
    }
}

#[tokio::test]
async fn public_report_excludes_progress_tool_output_and_later_user_turns() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    for (index, actor, kind) in [
        (0, "assistant", "progress"),
        (1, "tool", "tool_result"),
        (2, "user", "status"),
    ] {
        event(&host, &child, &format!("fake-{index}"), actor, kind,
            json!({"status":"completed","tool":"agent_turn_complete","content":"PRIVATE TOOL OR THINKING"}), now_ms()).await;
    }
    finish(&host, &child, "Public findings only", "completed").await;
    let mut later = child.clone();
    later.turn_id = Some("later-unrelated-user-turn".into());
    event(
        &host,
        &later,
        "later-user",
        "user",
        "message",
        json!({"content":"Unrelated new request"}),
        now_ms() + 10,
    )
    .await;
    event(
        &host,
        &later,
        "later-answer",
        "assistant",
        "status",
        json!({"status":"completed","tool":"agent_turn_complete","content":"OTHER TURN SECRET"}),
        now_ms() + 20,
    )
    .await;
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    assert_eq!(
        run(&result, &id)["report"]["content"],
        "Public findings only"
    );
    assert!(!result.to_string().contains("SECRET"));
    assert!(!result.to_string().contains("PRIVATE TOOL"));
}

#[tokio::test]
async fn terminal_report_save_race_waits_for_the_real_report_not_just_status() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    host.finish_subagent_for_child(child.task_id.as_deref().unwrap(), "completed")
        .await
        .unwrap();
    let saving = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        finish(
            &host,
            &child,
            "Report persisted after lifecycle completion",
            "completed",
        )
        .await;
    };
    let (result, ()) = tokio::join!(wait(&host, &parent, json!({"timeoutMs":1500})), saving);
    assert_eq!(result["reportPendingCount"], 0);
    assert_eq!(
        run(&result, &id)["report"]["content"],
        "Report persisted after lifecycle completion"
    );
}

#[tokio::test]
async fn completed_without_report_is_explicitly_missing_and_never_verified() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    host.finish_subagent_for_child(child.task_id.as_deref().unwrap(), "completed")
        .await
        .unwrap();
    sqlx::query("UPDATE subagent_runs SET completed_at_ms=? WHERE id=?")
        .bind(now_ms() - 10_000)
        .bind(&id)
        .execute(host.repository.pool())
        .await
        .unwrap();
    let result = wait(&host, &parent, json!({"timeoutMs":250})).await;
    assert_eq!(result["allCompleted"], true);
    assert_eq!(result["allWorkCompleted"], false);
    assert_eq!(result["reportMissingCount"], 1);
    assert_eq!(run(&result, &id)["report"]["content"], Value::Null);
    assert_eq!(run(&result, &id)["report"]["verification"], "unknown");
}

#[tokio::test]
async fn empty_completion_does_not_consume_the_child_terminal_transition() {
    let (host, parent, registration, _id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let err = host
        .call_persisted(
            "agent_turn_complete",
            child.clone(),
            json!({"content":"  "}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "final_response_required");
    let status: String =
        sqlx::query_scalar("SELECT status FROM subagent_runs WHERE child_task_id=?")
            .bind(child.task_id)
            .fetch_one(host.repository.pool())
            .await
            .unwrap();
    assert_eq!(status, "running");
}
