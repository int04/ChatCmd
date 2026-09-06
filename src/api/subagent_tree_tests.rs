use super::*;
use crate::runtime_host::user_message_tests::test_host;

#[tokio::test]
async fn root_api_includes_nested_approvals_and_stop_cascades_to_grandchildren() {
    let (host, agent, dir) = test_host().await;
    let state = host.test_app_state(dir.path().join("tree.db").display().to_string());
    let now = now_ms();
    for task in ["root", "child", "grandchild"] {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'mcp','running',1,?,?)")
            .bind(task).bind(&agent).bind(state.device.id.as_str()).bind(task).bind(now).bind(now).execute(state.repository.pool()).await.unwrap();
    }
    for (id, parent, turn, child) in [
        ("subagent-child", "root", "root-turn", "child"),
        ("subagent-grandchild", "child", "child-turn", "grandchild"),
    ] {
        sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'Work','running',?,?)")
            .bind(id).bind(parent).bind(turn).bind(child).bind(id).bind(now).bind(now).execute(state.repository.pool()).await.unwrap();
    }
    sqlx::query("INSERT INTO approvals(id,task_id,state,request_json,created_at_ms) VALUES('approval-grandchild','grandchild','pending',?,?)")
        .bind(json!({"tool":"fs_read_text","turnId":"grandchild-turn","input":{"path":"file.rs"}}).to_string()).bind(now).execute(state.repository.pool()).await.unwrap();
    let (parent, runs) = task_subagent_data(&state, "root").await.unwrap();
    assert!(parent.is_none());
    assert_eq!(runs.len(), 2);
    let grandchild = runs.iter().find(|v| v["taskId"] == "grandchild").unwrap();
    assert_eq!(grandchild["rootTurnId"], "root-turn");
    assert_eq!(grandchild["parentTurnId"], "child-turn");
    assert_eq!(grandchild["parentTaskId"], "child");
    let approvals = pending_subagent_approvals(&state, "root").await.unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["childTaskId"], "grandchild");
    assert_eq!(approvals[0]["rootTurnId"], "root-turn");
    interrupt_active_child_subagents(&state, "root")
        .await
        .unwrap();
    let (_, runs) = task_subagent_data(&state, "root").await.unwrap();
    assert!(runs.iter().all(|r| r["status"] == "interrupted"));
    assert!(
        pending_subagent_approvals(&state, "root")
            .await
            .unwrap()
            .is_empty()
    );
}
