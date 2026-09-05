use super::chatgpt_router_tests::{expect_json, extension_request, fixture, gui_get};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::Row;

fn native(user_id: &str) -> Value {
    json!({"conversationId":"native-test","conversationUrl":"https://chatgpt.com/c/native-test",
        "userMessageId":user_id,"content":"Hello directly from ChatGPT"})
}

#[tokio::test]
async fn native_turns_are_idempotent_and_never_grant_execution() {
    let (state, app, _directory) = fixture("completed").await;
    let path = "/api/local/chatgpt/capture/turns";
    let first = expect_json(
        extension_request(&app, "POST", path, native("user-1")).await,
        StatusCode::OK,
    )
    .await;
    let duplicate = expect_json(
        extension_request(&app, "POST", path, native("user-1")).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["id"], duplicate["id"]);
    let second = expect_json(
        extension_request(&app, "POST", path, native("user-2")).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["taskId"], second["taskId"]);
    assert_ne!(
        first["id"], second["id"],
        "identical text in a different user turn remains distinct"
    );
    let task_id = first["taskId"].as_str().unwrap();
    let allowed: Option<i64> = sqlx::query_scalar("SELECT allow_execute FROM tasks WHERE id=?")
        .bind(task_id)
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(allowed, Some(0));
    let tools: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_allowed_tools WHERE agent_id='chatgpt-browser-recorder'",
    )
    .fetch_one(state.repository.pool())
    .await
    .unwrap();
    assert_eq!(tools, 0);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND kind='message'",
    )
    .bind(task_id)
    .fetch_one(state.repository.pool())
    .await
    .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn native_followup_reuses_existing_conversation_without_changing_its_permission() {
    let (state, app, _directory) = fixture("completed").await;
    let pool = state.repository.pool();
    sqlx::query("UPDATE tasks SET allow_execute=0 WHERE id='task-a'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,created_at_ms,updated_at_ms) VALUES('task-a','native-test','https://chatgpt.com/c/native-test','Auto',1,1)")
        .execute(pool).await.unwrap();
    let request = expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/capture/turns",
            native("followup"),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(request["taskId"], "task-a");
    let allowed: Option<i64> =
        sqlx::query_scalar("SELECT allow_execute FROM tasks WHERE id='task-a'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(allowed, Some(0));
}

#[tokio::test]
async fn native_capture_rejects_spoofed_identity_and_authority_fields() {
    let (_state, app, _directory) = fixture("completed").await;
    for url in [
        "https://chatgpt.com/c/other",
        "https://chatgpt.com/share/c/native-test",
        "https://example.com/c/native-test",
    ] {
        let mut value = native("u");
        value["conversationUrl"] = json!(url);
        let response =
            extension_request(&app, "POST", "/api/local/chatgpt/capture/turns", value).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let mut value = native("u");
    value["taskId"] = json!("task-a");
    assert_eq!(
        extension_request(&app, "POST", "/api/local/chatgpt/capture/turns", value)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn full_stack_capture_from_native_and_chatcmd_without_mcp() {
    let (state, app, _directory) = fixture("completed").await;
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='queued',user_content='First line'||char(10)||'Second line',submitted_content='First line'||char(10)||'Second line' WHERE id='request-b'")
        .execute(state.repository.pool()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let gui_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut command = tokio::process::Command::new("node");
    command
        .arg("chatgpt-extension/capture-integration.cjs")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CHATCMD_CAPTURE_TEST_URL", url)
        .kill_on_drop(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(45), command.output()).await;
    server.abort();
    let output = result
        .expect("capture integration timeout")
        .expect("Node is required for extension integration");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    let snapshots = sqlx::query("SELECT r.id,r.task_id,r.status,e.payload_json FROM chatgpt_bridge_requests r JOIN timeline_events e ON e.event_id='chatgpt-think-'||r.id WHERE r.conversation_id IN ('native-e2e','bridge-e2e')")
        .fetch_all(state.repository.pool()).await.unwrap();
    assert_eq!(
        snapshots.len(),
        2,
        "no duplicate request after reinjection/revisit"
    );
    let token = state
        .gui_auth
        .setup_password("capture-integration-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    for row in snapshots {
        let (http_status, detail) = gui_get(
            &gui_app,
            &format!("/api/local/tasks/{}", row.get::<String, _>("task_id")),
            Some(&cookie),
        )
        .await;
        assert_eq!(http_status, StatusCode::OK);
        assert!(
            detail["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "chatgpt_think"
                    && event["payload"]["completed"] == true)
        );
        assert_eq!(row.get::<String, _>("status"), "completed");
        let payload: Value = serde_json::from_str(&row.get::<String, _>("payload_json")).unwrap();
        assert_eq!(payload["completed"], true);
        let text = payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["content"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Preparing the answer"));
        assert!(text.contains("The final answer"));
        assert!(!text.contains("Show more"));
        let tool_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND kind IN ('tool_call','tool_result')")
            .bind(row.get::<String,_>("task_id")).fetch_one(state.repository.pool()).await.unwrap();
        assert_eq!(tool_count, 0, "capture must work with zero MCP activity");
    }
}

#[tokio::test]
async fn identical_native_questions_in_same_millisecond_do_not_share_a_final() {
    let (state, app, _directory) = fixture("completed").await;
    let first = expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/capture/turns",
            native("same-ms-1"),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let first_id = first["id"].as_str().unwrap();
    expect_json(
        extension_request(
            &app,
            "POST",
            &format!("/api/local/chatgpt/bridge/{first_id}/browser-completed"),
            json!({"assistantContent":"Old answer"}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let second = expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/capture/turns",
            native("same-ms-2"),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let second_id = second["id"].as_str().unwrap();
    sqlx::query("UPDATE chatgpt_bridge_requests SET created_at_ms=(SELECT created_at_ms FROM chatgpt_bridge_requests WHERE id=?) WHERE id=?")
        .bind(first_id).bind(second_id).execute(state.repository.pool()).await.unwrap();
    let status = expect_json(
        extension_request(
            &app,
            "GET",
            &format!("/api/local/chatgpt/requests/{second_id}"),
            Value::Null,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["hasFinalResponse"], false);
    assert_eq!(status["status"], "running");
}

#[tokio::test]
async fn three_concurrent_native_conversations_stay_isolated_in_sqlite() {
    use std::collections::HashSet;
    let (state, app, _directory) = fixture("completed").await;
    async fn enroll(app: &axum::Router, conversation: &str) -> Value {
        expect_json(extension_request(app, "POST", "/api/local/chatgpt/capture/turns",
            json!({"conversationId":conversation,"conversationUrl":format!("https://chatgpt.com/c/{conversation}"),
                "userMessageId":"same-user-id","content":"Exactly the same question"})).await, StatusCode::OK).await
    }
    let (a, b, c) = tokio::join!(
        enroll(&app, "multi-a"),
        enroll(&app, "multi-b"),
        enroll(&app, "multi-c")
    );
    let requests = [a, b, c];
    assert_eq!(
        requests
            .iter()
            .map(|r| r["taskId"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        requests
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    async fn snapshot(app: &axum::Router, request: &Value, revision: u64) {
        let id = request["id"].as_str().unwrap();
        let conversation = request["conversationId"].as_str().unwrap();
        expect_json(extension_request(app, "POST", &format!("/api/local/chatgpt/bridge/{id}/observation"),
            json!({"conversationId":conversation,"conversationUrl":request["conversationUrl"],
                "userMessageId":"same-user-id","revision":revision,"completed":false,
                "messages":[{"id":"same-assistant-id","kind":"answer","content":format!("Only {conversation} revision {revision}")}]})).await, StatusCode::OK).await;
    }
    // Writes arrive together, repeatedly, sharing both user and assistant IDs across chats.
    for revision in 2..=8 {
        tokio::join!(
            snapshot(&app, &requests[0], revision),
            snapshot(&app, &requests[1], revision),
            snapshot(&app, &requests[2], revision)
        );
    }
    let a_id = requests[0]["id"].as_str().unwrap();
    expect_json(
        extension_request(
            &app,
            "POST",
            &format!("/api/local/chatgpt/bridge/{a_id}/browser-completed"),
            json!({"assistantContent":"Only multi-a FINAL"}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    for request in &requests[1..] {
        let id = request["id"].as_str().unwrap();
        let status = expect_json(
            extension_request(
                &app,
                "GET",
                &format!("/api/local/chatgpt/requests/{id}"),
                Value::Null,
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            status["status"], "running",
            "one completion must not finish another chat"
        );
        assert_eq!(status["hasFinalResponse"], false);
    }
    async fn complete(app: &axum::Router, request: &Value) {
        let id = request["id"].as_str().unwrap();
        let conversation = request["conversationId"].as_str().unwrap();
        expect_json(
            extension_request(
                app,
                "POST",
                &format!("/api/local/chatgpt/bridge/{id}/browser-completed"),
                json!({"assistantContent":format!("Only {conversation} FINAL")}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
    }
    tokio::join!(complete(&app, &requests[1]), complete(&app, &requests[2]));
    let token = state
        .gui_auth
        .setup_password("multi-chat-test-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    for request in &requests {
        let id = request["id"].as_str().unwrap();
        let task_id = request["taskId"].as_str().unwrap();
        let conversation = request["conversationId"].as_str().unwrap();
        let (status, detail) =
            gui_get(&app, &format!("/api/local/tasks/{task_id}"), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK);
        let events = detail["events"].as_array().unwrap();
        let snapshots: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "chatgpt_think")
            .collect();
        assert_eq!(
            snapshots.len(),
            1,
            "updates replace one durable snapshot, not append a token log"
        );
        assert_eq!(snapshots[0]["payload"]["bridgeRequestId"], id);
        assert_eq!(snapshots[0]["payload"]["completed"], true);
        for message in snapshots[0]["payload"]["messages"].as_array().unwrap() {
            assert!(
                message["content"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("Only {conversation}"))
            );
        }
        assert!(
            !events
                .iter()
                .any(|event| event["type"] == "tool_call" || event["type"] == "tool_result")
        );
    }
}
