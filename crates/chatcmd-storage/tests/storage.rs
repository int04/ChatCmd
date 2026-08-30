use std::path::Path;

use chatcmd_core::{
    Bootstrap, EventId, EventKind, LegacyImport, LocalDeviceStore, McpAgentStore, NewMcpAgent,
    PolicyLookup, SessionId, StorageError, Task, TaskId, TaskStatus, TaskStore, TerminalEventChunk,
    TerminalEventStore, TerminalSession, TerminalSessionStatus, ToolCatalogStore,
};
use chatcmd_storage::{CURRENT_SCHEMA_VERSION, LegacyImporter, SqliteRepository};
use tempfile::TempDir;

async fn repository(directory: &TempDir) -> SqliteRepository {
    SqliteRepository::open(&directory.path().join("chatcmd.db"), 8)
        .await
        .expect("open repository")
        .0
}

fn terminal_session(id: &str, task_id: Option<TaskId>) -> TerminalSession {
    TerminalSession {
        id: SessionId::new(id).expect("session ID"),
        task_id,
        turn_id: None,
        executable: "shell".to_owned(),
        working_directory: ".".to_owned(),
        columns: 80,
        rows: 24,
        process_id: Some(42),
        status: TerminalSessionStatus::Running,
        exit_code: None,
        created_at_ms: 1,
        updated_at_ms: 1,
        closed_at_ms: None,
    }
}

fn chunk(session_id: &SessionId, sequence: i64) -> TerminalEventChunk {
    TerminalEventChunk {
        session_id: session_id.clone(),
        sequence,
        event_id: EventId::new(format!("event-{sequence}")).expect("event ID"),
        task_id: None,
        turn_id: None,
        kind: EventKind::TerminalOutput,
        stream: Some("stdout".to_owned()),
        payload: format!("chunk-{sequence}").into_bytes(),
        payload_encoding: "utf-8".to_owned(),
        created_at_ms: sequence,
    }
}

#[tokio::test]
async fn fresh_bootstrap_twice_keeps_one_stable_device() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let first = repository.local_device().await.expect("first device");
    let report = repository.bootstrap().await.expect("second bootstrap");
    let second = repository.local_device().await.expect("second device");

    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(first, second);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_device")
        .fetch_one(repository.pool())
        .await
        .expect("count devices");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn secret_rotation_invalidates_old_token_and_database_stores_only_hash() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let created = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Agent".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent");
    let agent_id = created.agent.id.clone();
    let old = created.secret.expose_once();
    assert!(
        repository
            .lookup_policy_by_token(&old)
            .await
            .expect("old lookup")
            .is_some()
    );

    let rotated = repository
        .rotate_agent_secret(&agent_id)
        .await
        .expect("rotate secret");
    let new = rotated.secret.expose_once();
    assert_ne!(old, new);
    assert!(
        repository
            .lookup_policy_by_token(&old)
            .await
            .expect("stale lookup")
            .is_none()
    );
    assert!(
        repository
            .lookup_policy_by_token(&new)
            .await
            .expect("new lookup")
            .is_some()
    );
    let (digest_length, last4): (i64, String) =
        sqlx::query_as("SELECT length(secret_hash),secret_last4 FROM mcp_agents WHERE id=?")
            .bind(agent_id.as_str())
            .fetch_one(repository.pool())
            .await
            .expect("stored secret metadata");
    assert_eq!(digest_length, 32);
    assert_eq!(
        last4,
        new.chars()
            .skip(new.chars().count() - 4)
            .collect::<String>()
    );
}

#[tokio::test]
async fn agent_update_allowlist_and_delete_are_complete() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let created = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Before".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent");
    let id = created.agent.id;
    repository
        .set_agent_allowed_tools(&id, &["tool-device-list".to_owned()])
        .await
        .expect("set allowlist");
    let updated = repository
        .update_agent(
            &id,
            NewMcpAgent {
                id: None,
                name: "After".to_owned(),
                enabled: false,
            },
        )
        .await
        .expect("update agent");
    assert_eq!(updated.name, "After");
    assert!(!updated.enabled);
    assert_eq!(
        repository
            .agent_allowed_tool_ids(&id)
            .await
            .expect("read allowlist"),
        vec!["tool-device-list"]
    );
    repository.delete_agent(&id).await.expect("delete agent");
    assert!(repository.agent(&id).await.expect("read deleted").is_none());
}

#[tokio::test]
async fn chunks_are_ordered_and_idempotent() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let session = terminal_session("session-ordered", None);
    repository
        .upsert_terminal_session(&session)
        .await
        .expect("session");
    let chunks = vec![
        chunk(&session.id, 2),
        chunk(&session.id, 0),
        chunk(&session.id, 1),
    ];

    assert_eq!(
        repository
            .append_terminal_chunks(&chunks)
            .await
            .expect("first append"),
        3
    );
    assert_eq!(
        repository
            .append_terminal_chunks(&chunks)
            .await
            .expect("duplicate append"),
        0
    );
    let stored = repository
        .terminal_chunks(&session.id, None, 10)
        .await
        .expect("read chunks");
    assert_eq!(
        stored.iter().map(|item| item.sequence).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wal_allows_concurrent_readers_and_writer() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let session = terminal_session("session-wal", None);
    repository
        .upsert_terminal_session(&session)
        .await
        .expect("session");

    let writer_repository = repository.clone();
    let session_id = session.id.clone();
    let writer = tokio::spawn(async move {
        for sequence in 0..50 {
            writer_repository
                .append_terminal_chunks(&[chunk(&session_id, sequence)])
                .await
                .expect("append chunk");
        }
    });
    let mut readers = Vec::new();
    for _ in 0..4 {
        let reader_repository = repository.clone();
        readers.push(tokio::spawn(async move {
            for _ in 0..25 {
                let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM terminal_event_chunks")
                    .fetch_one(reader_repository.pool())
                    .await
                    .expect("WAL read");
                tokio::task::yield_now().await;
            }
        }));
    }
    writer.await.expect("writer task");
    for reader in readers {
        reader.await.expect("reader task");
    }
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(repository.pool())
        .await
        .expect("journal mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

#[tokio::test]
async fn restart_recovery_interrupts_running_task_and_session() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let device = repository.local_device().await.expect("device");
    let task_id = TaskId::new("running-task").expect("task ID");
    let session = terminal_session("running-session", Some(task_id.clone()));
    repository
        .upsert_task(&Task {
            id: task_id.clone(),
            agent_id: None,
            device_id: device.id,
            conversation_scope_hash: None,
            title: None,
            source: None,
            project_folder: Some("D:\\DEV\\Recovery".to_owned()),
            allow_execute: Some(true),
            status: TaskStatus::Running,
            active_session_id: Some(session.id.clone()),
            generation: 1,
            stopped_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .await
        .expect("task");
    repository
        .upsert_terminal_session(&session)
        .await
        .expect("session");

    let report = repository.bootstrap().await.expect("restart bootstrap");
    assert_eq!(
        (report.interrupted_tasks, report.interrupted_sessions),
        (1, 1)
    );
    let stored_task = repository
        .task(&task_id)
        .await
        .expect("read task")
        .expect("task exists");
    assert_eq!(stored_task.status, TaskStatus::Interrupted);
    assert_eq!(
        stored_task.project_folder.as_deref(),
        Some("D:\\DEV\\Recovery")
    );
    let status: String = sqlx::query_scalar("SELECT status FROM terminal_sessions WHERE id=?")
        .bind(session.id.as_str())
        .fetch_one(repository.pool())
        .await
        .expect("session status");
    assert_eq!(status, "interrupted");
}

#[tokio::test]
async fn recent_tasks_hide_claimed_subagent_tasks() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let device = repository.local_device().await.expect("device");
    let parent_id = TaskId::new("parent-task").expect("parent task ID");
    let child_id = TaskId::new("child-task").expect("child task ID");

    for (id, title, updated_at_ms) in [(&parent_id, "Parent", 1_i64), (&child_id, "Child", 2_i64)] {
        repository
            .upsert_task(&Task {
                id: id.clone(),
                agent_id: None,
                device_id: device.id.clone(),
                conversation_scope_hash: None,
                title: Some(title.to_owned()),
                source: Some("mcp".to_owned()),
                project_folder: None,
                allow_execute: Some(true),
                status: TaskStatus::Running,
                active_session_id: None,
                generation: 1,
                stopped_at_ms: None,
                created_at_ms: updated_at_ms,
                updated_at_ms,
            })
            .await
            .expect("task");
    }
    sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?, ?, ?)")
        .bind("subagent-test")
        .bind(parent_id.as_str())
        .bind("turn-parent")
        .bind(child_id.as_str())
        .bind("Child Agent")
        .bind("Inspect delegated work")
        .bind("running")
        .bind(2_i64)
        .bind(2_i64)
        .execute(repository.pool())
        .await
        .expect("subagent relation");

    let recent = repository.list_tasks(20).await.expect("recent tasks");
    assert_eq!(
        recent
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        vec![parent_id.as_str()]
    );
    assert!(
        repository
            .task(&child_id)
            .await
            .expect("read child directly")
            .is_some()
    );
}
#[tokio::test]
async fn newer_schema_is_rejected_before_migrations() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    sqlx::query("UPDATE schema_version SET version=? WHERE singleton_id=1")
        .bind(CURRENT_SCHEMA_VERSION + 1)
        .execute(repository.pool())
        .await
        .expect("raise schema");

    let error = repository
        .bootstrap()
        .await
        .expect_err("newer schema must fail");
    assert!(
        matches!(error, StorageError::SchemaTooNew { found, supported }
        if found == CURRENT_SCHEMA_VERSION + 1 && supported == CURRENT_SCHEMA_VERSION)
    );
}

#[tokio::test]
async fn importer_is_idempotent_and_continues_after_corrupt_jsonl_line() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let legacy = directory.path().join("legacy");
    tokio::fs::create_dir_all(legacy.join("artifacts"))
        .await
        .expect("legacy directories");
    write(
        &legacy.join("device.json"),
        br#"{"deviceId":"legacy-device","installationId":"legacy-install","name":"Legacy"}"#,
    )
    .await;
    write(&legacy.join("access.json"), br#"{"agents":[{"id":"legacy-agent","name":"Imported","token":"legacy-secret-token","enabled":true}]}"#).await;
    write(&legacy.join("events.jsonl"), b"{\"id\":\"first\",\"taskId\":\"legacy-task\",\"payload\":{\"text\":\"one\"}}\nnot-json\n{\"id\":\"second\",\"taskId\":\"legacy-task\",\"payload\":{\"text\":\"two\"}}\n").await;
    write(&legacy.join("artifacts").join("result.txt"), b"artifact").await;
    let importer = LegacyImporter::new(repository.clone());

    let first = importer.import_legacy(&legacy).await.expect("first import");
    assert_eq!(first.imported_events, 2);
    assert_eq!(first.warnings.len(), 1);
    assert!(first.warnings[0].contains(":2: malformed JSONL"));
    assert!(
        repository
            .lookup_policy_by_token("legacy-secret-token")
            .await
            .expect("imported secret")
            .is_some()
    );
    let second = importer
        .import_legacy(&legacy)
        .await
        .expect("second import");
    assert_eq!(second.imported_sources, 0);
    assert_eq!(second.skipped_sources, 4);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events")
        .fetch_one(repository.pool())
        .await
        .expect("event count");
    let artifacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifact_registry")
        .fetch_one(repository.pool())
        .await
        .expect("artifact count");
    assert_eq!((events, artifacts), (2, 1));
}

async fn write(path: &Path, bytes: &[u8]) {
    tokio::fs::write(path, bytes).await.expect("write fixture");
}
