use super::routing_tests::{REQUEST, SCOPE, TASK, TURN, count, fixture, seed};
use crate::{
    chatgpt_routing::with_route,
    runtime_host::user_message_tests::{test_host, turn_context},
};

#[tokio::test]
async fn request_marker_routes_a_rewritten_message_with_a_model_generated_turn() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &with_route(&submitted, REQUEST, TURN)).await;
    let mut context = turn_context("marker", &agent, "agent_user_message", "model-turn", SCOPE);
    host.ensure_call_identity(&mut context, Some(&with_route(&echo, REQUEST, TURN)))
        .await
        .unwrap();
    assert_eq!(context.task_id.as_deref(), Some(TASK));
    assert_eq!(
        context.turn_id.as_deref(),
        Some("model-turn"),
        "do not change an already supplied turn ID"
    );
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn request_route_is_scoped_to_authenticated_agent_and_conflicts_do_not_create_tasks() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    let message = with_route(&echo, REQUEST, TURN);
    let mut foreign = turn_context(
        "foreign",
        "another-agent",
        "agent_user_message",
        "foreign-turn",
        SCOPE,
    );
    assert_eq!(
        host.ensure_call_identity(&mut foreign, Some(&message))
            .await
            .unwrap_err()
            .code,
        "conversation_identity_unbound"
    );
    let mut conflict = turn_context(
        "conflict",
        &agent,
        "agent_user_message",
        "conflict-turn",
        SCOPE,
    );
    conflict.task_id = Some("task-invented".to_owned());
    assert_eq!(
        host.ensure_call_identity(&mut conflict, Some(&message))
            .await
            .unwrap_err()
            .code,
        "conversation_identity_conflict"
    );
    let mut invented = turn_context(
        "invented",
        &agent,
        "agent_user_message",
        "invented-turn",
        SCOPE,
    );
    invented.task_id = Some("task-invented".to_owned());
    assert_eq!(
        host.ensure_call_identity(&mut invented, Some("new message"))
            .await
            .unwrap_err()
            .code,
        "conversation_identity_unbound"
    );
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn stopped_request_route_does_not_revive_a_task_or_ask_for_approval() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='stopped' WHERE id=?")
        .bind(REQUEST)
        .execute(host.repository.pool())
        .await
        .unwrap();
    let mut context = turn_context("stopped", &agent, "agent_user_message", TURN, SCOPE);
    assert_eq!(
        host.ensure_call_identity(&mut context, Some(&echo))
            .await
            .unwrap_err()
            .code,
        "conversation_identity_stale"
    );
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn request_selector_never_changes_denied_conversation_permissions() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    sqlx::query("UPDATE tasks SET allow_execute=0 WHERE id=?")
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    let mut context = turn_context("denied", &agent, "agent_user_message", TURN, SCOPE);
    assert_eq!(
        host.ensure_call_identity(&mut context, Some(&echo))
            .await
            .unwrap_err()
            .code,
        "conversation_approval_denied"
    );
    let allowed: i64 = sqlx::query_scalar("SELECT allow_execute FROM tasks WHERE id=?")
        .bind(TASK)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(allowed, 0);
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn old_generation_scope_cannot_rebind_after_compaction() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, _) = fixture();
    seed(&host, &agent, &submitted).await;
    let mut first = turn_context("old", &agent, "agent_user_message", "old-turn", SCOPE);
    host.ensure_call_identity(&mut first, Some(&submitted))
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET generation=generation+1,conversation_scope_hash='openai:replacement' WHERE id=?")
        .bind(TASK).execute(host.repository.pool()).await.unwrap();
    let mut stale = turn_context("stale", &agent, "agent_user_message", "stale-turn", SCOPE);
    stale.task_id = Some(TASK.to_owned());
    assert_eq!(
        host.ensure_call_identity(&mut stale, Some("continue"))
            .await
            .unwrap_err()
            .code,
        "conversation_compacting_or_archived"
    );
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn completed_scope_binding_survives_database_close_and_reopen() {
    let (mut host, agent, dir) = test_host().await;
    let (submitted, _) = fixture();
    seed(&host, &agent, &submitted).await;
    let mut first = turn_context(
        "before-restart",
        &agent,
        "agent_user_message",
        "before-turn",
        SCOPE,
    );
    host.ensure_call_identity(&mut first, Some(&submitted))
        .await
        .unwrap();
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='completed' WHERE id=?")
        .bind(REQUEST)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='completed' WHERE id=?")
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL WHERE task_id=?")
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    host.repository.pool().close().await;
    host.repository = chatcmd_storage::SqliteRepository::open(&dir.path().join("chatcmd.db"), 2)
        .await
        .unwrap()
        .0;
    let mut next = turn_context(
        "after-restart",
        &agent,
        "agent_user_message",
        "after-turn",
        SCOPE,
    );
    host.ensure_call_identity(&mut next, Some("commit"))
        .await
        .unwrap();
    assert_eq!(next.task_id.as_deref(), Some(TASK));
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn identical_prompts_are_disambiguated_by_request_id() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,source,allow_execute,status,generation,created_at_ms,updated_at_ms) SELECT 'task-b',agent_id,device_id,source,allow_execute,status,generation,created_at_ms,updated_at_ms FROM tasks WHERE id=?")
        .bind(TASK).execute(host.repository.pool()).await.unwrap();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) SELECT 'request-b','task-b','turn-b',agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms FROM chatgpt_bridge_requests WHERE id=?")
        .bind(REQUEST).execute(host.repository.pool()).await.unwrap();
    let mut context = turn_context(
        "pick-b",
        &agent,
        "agent_user_message",
        "model-turn-b",
        "openai:transport-b",
    );
    host.ensure_call_identity(
        &mut context,
        Some(&with_route(&echo, "request-b", "turn-b")),
    )
    .await
    .unwrap();
    assert_eq!(context.task_id.as_deref(), Some("task-b"));
    assert_eq!(count(&host).await, 2);
}

#[tokio::test]
async fn provisional_tool_match_does_not_persist_a_durable_scope_alias() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, _) = fixture();
    seed(&host, &agent, &submitted).await;
    let mut context = turn_context("early-tool", &agent, "workspace_roots", "early-turn", SCOPE);
    host.ensure_call_identity(&mut context, None).await.unwrap();
    assert_eq!(context.task_id.as_deref(), Some(TASK));
    let aliases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_mcp_scope_bindings")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(
        aliases, 0,
        "heuristic tool-only routing must not pin future user turns"
    );
}

#[tokio::test]
async fn older_request_route_cannot_claim_the_newer_user_turn() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) SELECT 'new-request',task_id,'new-browser-turn',agent_id,model,user_content,submitted_content,'running',created_at_ms+1,updated_at_ms+1 FROM chatgpt_bridge_requests WHERE id=?")
        .bind(REQUEST).execute(host.repository.pool()).await.unwrap();
    let mut context = turn_context(
        "late-old-request",
        &agent,
        "agent_user_message",
        TURN,
        SCOPE,
    );
    assert_eq!(
        host.ensure_call_identity(&mut context, Some(&echo))
            .await
            .unwrap_err()
            .code,
        "conversation_identity_stale"
    );
    assert_eq!(count(&host).await, 1);
}
