#![allow(dead_code)]
use chatcmd_core::StorageError;
use chatcmd_storage::{
    SqliteRepository,
    compact::{CompactCheckpoint, CompactJob, CompactPhase, openai_scope},
};
use tempfile::TempDir;

pub async fn fixture() -> (SqliteRepository, TempDir) {
    let directory = TempDir::new().expect("temporary directory");
    let (repo, _) = SqliteRepository::open(&directory.path().join("compact.db"), 4)
        .await
        .expect("open temp DB");
    sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,created_at_ms,updated_at_ms) VALUES('agent','Compact Agent',randomblob(32),'test',1,1,1)")
        .execute(repo.pool()).await.expect("agent");
    for (task, conversation) in [("task-a", "old-chat"), ("task-b", "owned-chat")] {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,allow_execute,status,active_session_id,generation,created_at_ms,updated_at_ms) SELECT ?,'agent',device_id,?,'Preserve my title','chatgpt_web','D:\\project',0,'running','old-session',7,1,1 FROM local_device")
            .bind(task).bind(openai_scope(conversation)).execute(repo.pool()).await.expect("task");
        sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,project_folder,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES(?,?,?,'agent','Pro','question','wrapped question','D:\\project','running',?,?,1,1)")
            .bind(format!("request-{task}")).bind(task).bind(format!("turn-{task}"))
            .bind(conversation).bind(format!("https://chatgpt.com/g/g-p-project/c/{conversation}"))
            .execute(repo.pool()).await.expect("request");
        sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Pro',?,1,1)")
            .bind(task).bind(conversation).bind(format!("https://chatgpt.com/g/g-p-project/c/{conversation}"))
            .bind(format!("request-{task}")).execute(repo.pool()).await.expect("binding");
    }
    sqlx::query("INSERT INTO agent_allowed_tools(agent_id,tool_id) VALUES('agent','tool-fs-read')")
        .execute(repo.pool())
        .await
        .expect("permission");
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('keep-history','task-a','turn-task-a','user','message','keep-history','{\"content\":\"keep me\"}',1)")
        .execute(repo.pool()).await.expect("history");
    sqlx::query("INSERT INTO chatgpt_message_queue(id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms) VALUES('queue-a','task-a','do not lose me','immediate',100,1,1)")
        .execute(repo.pool()).await.expect("queue");
    (repo, directory)
}

pub async fn checkpoint(
    repo: &SqliteRepository,
    job: &CompactJob,
    phase: CompactPhase,
    text: Option<&str>,
) -> CompactJob {
    repo.compact_checkpoint(
        &job.id,
        &CompactCheckpoint {
            expected_revision: job.revision,
            phase: Some(phase),
            handoff_text: text.map(str::to_owned),
            ..Default::default()
        },
        &format!("step-{}-{}", job.id, job.revision),
        10 + job.revision,
    )
    .await
    .expect("checkpoint")
}

pub async fn ready(repo: &SqliteRepository) -> CompactJob {
    let job = repo
        .compact_start("task-a", "start-a", 2)
        .await
        .expect("start");
    let job = checkpoint(repo, &job, CompactPhase::WritingHandoff, None).await;
    let job = checkpoint(
        repo,
        &job,
        CompactPhase::SavingHandoff,
        Some("durable handoff"),
    )
    .await;
    checkpoint(repo, &job, CompactPhase::OpeningNewChat, None).await
}

pub async fn finish(repo: &SqliteRepository, job: &CompactJob) -> CompactJob {
    repo.compact_checkpoint(
        &job.id,
        &CompactCheckpoint {
            expected_revision: job.revision,
            phase: Some(CompactPhase::Completed),
            new_conversation_id: Some("new-chat".to_owned()),
            new_conversation_url: Some("https://chatgpt.com/g/g-p-project/c/new-chat".to_owned()),
            ..Default::default()
        },
        "complete-a",
        20,
    )
    .await
    .expect("complete")
}

pub fn assert_conflict<T: std::fmt::Debug>(result: Result<T, StorageError>) {
    assert!(
        matches!(result, Err(StorageError::Conflict(_))),
        "{result:?}"
    );
}
