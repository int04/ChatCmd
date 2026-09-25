//! Regressions for ChatUI messages accidentally becoming new MCP conversations.
use std::time::Duration;

use chatcmd_storage::compact::{CompactCheckpoint, CompactPhase, openai_scope};
use serde_json::{Value, json};

use super::{RuntimeHost, now_ms};
use crate::runtime_host::user_message_tests::{test_host, turn_context};

const ORIGINAL: &str = "task-chat-0fda330a-4ed7-5af2-83b9-ba5da0990aee";
const PROMPT: &str = "Sử dụng plugin @rust_test\n\nThư mục dự án: D:\\DEV\\Caplog\\Client\n\nđể thực hiện yêu cầu sau: đọc docs/huongdan.md, điều chỉnh UI trang lading y hệt như ảnh t gửi, (chỉ lấy bố cục, còn màu vẫn theo chuẩn colorscheme)";

async fn seed_bridge(
    host: &RuntimeHost,
    agent: &str,
    task: &str,
    submitted: &str,
    age_ms: i64,
    active: bool,
) {
    let now = now_ms().saturating_sub(age_ms);
    let request = format!("request-{task}");
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'chatgpt_web',1,'running',1,?,?)")
        .bind(task).bind(agent).bind(host.device.id.as_str()).bind(format!("openai:browser-{task}"))
        .bind(now).bind(now).execute(host.repository.pool()).await.expect("seed task");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?, 'Auto',?,?,'running',?,?)")
        .bind(&request).bind(task).bind(format!("browser-turn-{task}")).bind(agent)
        .bind(submitted).bind(submitted).bind(now).bind(now)
        .execute(host.repository.pool()).await.expect("seed bridge request");
    if active {
        sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',?,?,?)")
            .bind(task).bind(format!("browser-{task}")).bind(format!("https://chatgpt.com/c/browser-{task}"))
            .bind(request).bind(now).bind(now).execute(host.repository.pool()).await.expect("seed active conversation");
    }
}

async fn task_count(host: &RuntimeHost) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(host.repository.pool())
        .await
        .expect("count tasks")
}

async fn enable_approval(host: &RuntimeHost) {
    sqlx::query("UPDATE settings SET value_json='true' WHERE key='ui_approveNewConversations'")
        .execute(host.repository.pool())
        .await
        .expect("enable approval");
}

#[tokio::test]
async fn uploaded_image_reuses_local_task_without_approval() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, PROMPT, 0, true).await;
    enable_approval(&host).await;
    let mut events = host.events.subscribe();
    let message = format!("<uploaded image>\n\n{PROMPT}");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        host.call_persisted(
            "agent_user_message",
            turn_context(
                "image-regression",
                &agent,
                "agent_user_message",
                "image-turn",
                "openai:mcp-transport",
            ),
            json!({"content": message}),
        ),
    )
    .await;
    assert_eq!(
        task_count(&host).await,
        1,
        "must not create an approval ghost task"
    );
    let accepted = result
        .expect("must not wait for a new conversation approval")
        .expect("reuse local task");
    assert_eq!(accepted["taskId"], ORIGINAL);
    let payload: String = sqlx::query_scalar("SELECT payload_json FROM timeline_events WHERE task_id=? AND turn_id='image-turn' AND actor='user' AND kind='message'")
        .bind(ORIGINAL).fetch_one(host.repository.pool()).await.expect("MCP user message");
    let payload: Value = serde_json::from_str(&payload).expect("payload JSON");
    assert_eq!(payload["bridgeRequestId"], format!("request-{ORIGINAL}"));
    assert_eq!(payload["browserTurnId"], format!("browser-turn-{ORIGINAL}"));
    assert_eq!(
        payload["content"], message,
        "normalization must not rewrite user data"
    );
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event_type, "conversation.approval_pending");
    }
}

#[tokio::test]
async fn active_image_request_still_matches_after_five_minutes() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, PROMPT, 10 * 60 * 1000, true).await;
    let mut context = turn_context(
        "slow-image",
        &agent,
        "agent_user_message",
        "slow-image-turn",
        "openai:new-transport",
    );
    host.ensure_call_identity(
        &mut context,
        Some(&format!("{PROMPT}\n\n<<ImageDisplayed>>")),
    )
    .await
    .expect("resolve active request regardless of elapsed time");
    assert_eq!(context.task_id.as_deref(), Some(ORIGINAL));
    assert_eq!(task_count(&host).await, 1);
}

#[tokio::test]
async fn known_scope_wins_over_another_tasks_matching_message() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, "previous message", 0, true).await;
    seed_bridge(&host, &agent, "task-other", "commit", 0, true).await;
    let scope = format!("openai:browser-{ORIGINAL}");
    let mut context = turn_context(
        "scope-first",
        &agent,
        "agent_user_message",
        "scope-turn",
        &scope,
    );
    host.ensure_call_identity(&mut context, Some("commit"))
        .await
        .expect("reuse known scope");
    assert_eq!(context.task_id.as_deref(), Some(ORIGINAL));
    assert_eq!(task_count(&host).await, 2);
}

#[tokio::test]
async fn known_turn_wins_over_another_tasks_matching_message() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, "previous message", 0, true).await;
    seed_bridge(&host, &agent, "task-other", "commit", 0, true).await;
    let mut bound = turn_context(
        "bind-turn",
        &agent,
        "workspace_roots",
        "known-turn",
        "openai:original",
    );
    bound.task_id = Some(ORIGINAL.to_owned());
    host.ensure_call_identity(&mut bound, None)
        .await
        .expect("bind turn");
    let mut context = turn_context(
        "reused-turn",
        &agent,
        "agent_user_message",
        "known-turn",
        "openai:rotated",
    );
    host.ensure_call_identity(&mut context, Some("commit"))
        .await
        .expect("reuse bound turn");
    assert_eq!(context.task_id.as_deref(), Some(ORIGINAL));
    assert_eq!(task_count(&host).await, 2);
}

#[tokio::test]
async fn ambiguous_image_message_does_not_create_or_choose_a_task() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, PROMPT, 0, true).await;
    seed_bridge(&host, &agent, "task-other", PROMPT, 0, true).await;
    let mut context = turn_context(
        "ambiguous-image",
        &agent,
        "agent_user_message",
        "ambiguous-turn",
        "openai:unknown",
    );
    let result = host
        .ensure_call_identity(&mut context, Some(&format!("<uploaded image>\n\n{PROMPT}")))
        .await;
    assert_eq!(
        task_count(&host).await,
        2,
        "ambiguity must never create a third task"
    );
    assert_eq!(
        result.expect_err("must not guess a conversation").code,
        "conversation_identity_ambiguous"
    );
}

async fn seed_pending(host: &RuntimeHost, agent: &str, request: &str) {
    let now = now_ms();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',?,?,'queued',?,?)")
        .bind(request).bind(format!("turn-{request}")).bind(agent).bind(PROMPT).bind(PROMPT)
        .bind(now).bind(now).execute(host.repository.pool()).await.expect("seed pending request");
}

#[tokio::test]
async fn uploaded_image_claims_pending_request_without_approval() {
    let (host, agent, _directory) = test_host().await;
    seed_pending(&host, &agent, "pending-image").await;
    enable_approval(&host).await;
    let mut context = turn_context(
        "pending-image-call",
        &agent,
        "agent_user_message",
        "pending-image-turn",
        "openai:pending-image",
    );
    let message = format!("<uploaded image>\n\n{PROMPT}");
    tokio::time::timeout(
        Duration::from_secs(2),
        host.ensure_call_identity(&mut context, Some(&message)),
    )
    .await
    .expect("no approval wait")
    .expect("claim queued request");
    let claimed: Option<String> =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_bridge_requests WHERE id='pending-image'")
            .fetch_one(host.repository.pool())
            .await
            .expect("claimed request");
    assert!(claimed.is_some());
    assert_eq!(context.task_id, claimed);
    assert_eq!(task_count(&host).await, 1);
    let source: String = sqlx::query_scalar("SELECT source FROM tasks WHERE id=?")
        .bind(context.task_id.as_deref())
        .fetch_one(host.repository.pool())
        .await
        .expect("task source");
    assert_eq!(source, "chatgpt_web");
}

#[tokio::test]
async fn ambiguous_pending_images_do_not_create_any_task() {
    let (host, agent, _directory) = test_host().await;
    seed_pending(&host, &agent, "pending-a").await;
    seed_pending(&host, &agent, "pending-b").await;
    let mut context = turn_context(
        "ambiguous-pending",
        &agent,
        "agent_user_message",
        "ambiguous-pending-turn",
        "openai:pending-unknown",
    );
    let result = host
        .ensure_call_identity(&mut context, Some(&format!("<uploaded image>\n\n{PROMPT}")))
        .await;
    assert_eq!(task_count(&host).await, 0);
    assert_eq!(
        result.expect_err("must not guess pending request").code,
        "conversation_identity_ambiguous"
    );
}

#[tokio::test]
async fn unrelated_new_message_is_not_assigned_to_the_only_active_bridge() {
    let (host, agent, _directory) = test_host().await;
    seed_bridge(&host, &agent, ORIGINAL, PROMPT, 0, true).await;
    let mut context = turn_context(
        "unrelated",
        &agent,
        "agent_user_message",
        "unrelated-turn",
        "openai:separate-chat",
    );
    host.ensure_call_identity(&mut context, Some("A genuinely different user request"))
        .await
        .expect("new direct chat");
    assert_ne!(context.task_id.as_deref(), Some(ORIGINAL));
    assert_eq!(task_count(&host).await, 2);
}

async fn seed_completed_manual_compact(host: &RuntimeHost, agent: &str) -> (String, String) {
    let old_id = "manual-compact-old";
    let new_id = "manual-compact-new";
    let old_url = format!("https://chatgpt.com/c/{old_id}");
    let new_url = format!("https://chatgpt.com/c/{new_id}");
    let now = now_ms();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'chatgpt_web',1,'running',1,?,?)")
        .bind(ORIGINAL).bind(agent).bind(host.device.id.as_str()).bind(openai_scope(old_id))
        .bind(now).bind(now).execute(host.repository.pool()).await.expect("seed compact task");
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',NULL,?,?)")
        .bind(ORIGINAL).bind(old_id).bind(&old_url).bind(now).bind(now)
        .execute(host.repository.pool()).await.expect("seed compact conversation");

    let mut job = host
        .repository
        .compact_start(ORIGINAL, "manual-compact-start", now + 1)
        .await
        .expect("start manual compact");
    job = host
        .repository
        .compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: job.revision,
                phase: Some(CompactPhase::SavingHandoff),
                handoff_text: Some("manual compact handoff".to_owned()),
                ..CompactCheckpoint::default()
            },
            "manual-compact-handoff",
            now + 2,
        )
        .await
        .expect("save manual handoff");
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
            "manual-compact-open",
            now + 3,
        )
        .await
        .expect("reserve manual destination");
    job = host
        .repository
        .compact_checkpoint(
            &job.id,
            &CompactCheckpoint {
                expected_revision: job.revision,
                phase: Some(CompactPhase::Completed),
                new_conversation_id: Some(new_id.to_owned()),
                new_conversation_url: Some(new_url.clone()),
                ..CompactCheckpoint::default()
            },
            "manual-compact-complete",
            now + 4,
        )
        .await
        .expect("complete manual compact");
    assert_eq!(job.phase, CompactPhase::Completed);
    (new_id.to_owned(), new_url)
}

#[tokio::test]
async fn manual_post_compact_waits_for_browser_binding_and_reuses_task_without_approval() {
    let (host, agent, _directory) = test_host().await;
    let (new_id, new_url) = seed_completed_manual_compact(&host, &agent).await;
    enable_approval(&host).await;
    let pool = host.repository.pool().clone();
    let browser_agent = agent.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = now_ms();
        sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES('manual-browser-turn',?,?,?,'Auto','tiếp tục','tiếp tục','running',?,?,?,?)")
            .bind(ORIGINAL).bind("manual-browser-turn-id").bind(browser_agent)
            .bind(&new_id).bind(&new_url).bind(now).bind(now)
            .execute(&pool).await.expect("record browser continuation");
        sqlx::query("UPDATE chatgpt_conversations SET active_request_id='manual-browser-turn',updated_at_ms=? WHERE task_id=?")
            .bind(now).bind(ORIGINAL).execute(&pool).await.expect("activate browser continuation");
    });

    let provider_scope = "openai:provider-session-after-compact";
    let mut context = turn_context(
        "manual-post-compact",
        &agent,
        "agent_user_message",
        "manual-post-compact-turn",
        provider_scope,
    );
    let mut events = host.events.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3),
        host.ensure_call_identity(&mut context, Some("tiếp tục")),
    )
    .await
    .expect("must not wait for approval")
    .expect("reuse compact destination task");
    writer.await.expect("browser writer task");

    assert_eq!(context.task_id.as_deref(), Some(ORIGINAL));
    assert_eq!(task_count(&host).await, 1);
    let binding: (String, i64) = sqlx::query_as(
        "SELECT task_id,generation FROM chatgpt_mcp_scope_bindings WHERE agent_id=? AND device_id=? AND scope_hash=?",
    ).bind(&agent).bind(host.device.id.as_str()).bind(provider_scope)
        .fetch_one(host.repository.pool()).await.expect("persist replacement provider scope");
    let generation: i64 = sqlx::query_scalar("SELECT generation FROM tasks WHERE id=?")
        .bind(ORIGINAL)
        .fetch_one(host.repository.pool())
        .await
        .expect("read compact generation");
    assert_eq!(binding.0, ORIGINAL);
    assert_eq!(binding.1, generation);
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event_type, "conversation.approval_pending");
    }
}

#[tokio::test]
async fn manual_post_compact_fails_closed_instead_of_requesting_new_conversation_approval() {
    let (host, agent, _directory) = test_host().await;
    seed_completed_manual_compact(&host, &agent).await;
    enable_approval(&host).await;
    let mut context = turn_context(
        "manual-post-compact-missing-browser",
        &agent,
        "agent_user_message",
        "manual-post-compact-missing-browser-turn",
        "openai:provider-session-before-browser",
    );
    let mut events = host.events.subscribe();
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        host.ensure_call_identity(&mut context, Some("tiếp tục")),
    )
    .await
    .expect("bounded compact binding wait")
    .expect_err("must not create a new approval conversation");

    assert_eq!(error.code, "conversation_identity_pending");
    assert_eq!(task_count(&host).await, 1);
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event_type, "conversation.approval_pending");
    }
}
