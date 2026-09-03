use std::{collections::BTreeMap, sync::Arc};

use chatcmd_core::{McpAgentStore as _, NewMcpAgent};
use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, GitService, NullEventSink, OperationContext,
    PolicyContext, PolicyDecision, PolicyEngine, ProcessService, RuntimeConfig, RuntimeResult,
    ShellRuntime, SkillService, WorkspaceService,
};
use chatcmd_storage::SqliteRepository;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::broadcast;

use super::*;

struct AllowApproval;

impl ApprovalDecision for AllowApproval {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
async fn git_status_uses_task_project_folder_when_cwd_is_omitted() {
    let directory = TempDir::new().expect("temporary directory");
    let project = directory.path().join("git-project");
    std::fs::create_dir_all(&project).expect("create git project");
    let initialized = std::process::Command::new("git")
        .arg("init")
        .arg(&project)
        .output()
        .expect("initialize git repository");
    assert!(initialized.status.success());
    std::fs::write(project.join("marker.txt"), "untracked").expect("write marker");

    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .expect("open repository");
    let created = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Git cwd test".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical test root");
    let policy = PolicyEngine::new(
        Some(ExecutionPolicy {
            default: PolicyDecision::Allow,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }),
        Arc::new(AllowApproval),
    );
    let workspace = WorkspaceService::new(std::slice::from_ref(&root), policy.clone())
        .expect("workspace service");
    let config = RuntimeConfig {
        roots: vec![root.clone()],
        repository_root: Some(root.clone()),
        ..RuntimeConfig::default()
    };
    let shell = ShellRuntime::new(config, policy.clone(), Arc::new(NullEventSink));
    let git = GitService::new(workspace.clone(), 10_000);
    let process = ProcessService::new(policy);
    let skills = SkillService::new(None, Some(&root), 10_000);
    let (events, _) = broadcast::channel(16);
    let host = RuntimeHost::new(
        repository,
        bootstrap.device,
        shell,
        workspace,
        git,
        process,
        skills,
        events,
    );

    let task_id = "task-git-project-folder";
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,NULL,NULL,'mcp',?,'running',NULL,1,NULL,0,0)")
        .bind(task_id)
        .bind(created.agent.id.as_str())
        .bind(host.device.id.as_str())
        .bind(project.display().to_string())
        .execute(host.repository.pool())
        .await
        .expect("insert task project folder");

    let mut context = OperationContext::new(
        "git-status-default",
        created.agent.id.as_str(),
        "git_status",
    );
    context.task_id = Some(task_id.to_owned());

    let result = host
        .dispatch("git_status", context, json!({}))
        .await
        .expect("git status without cwd");
    assert!(
        result["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("marker.txt")
    );

    let mut missing_context = OperationContext::new(
        "git-status-without-project",
        created.agent.id.as_str(),
        "git_status",
    );
    missing_context.task_id = Some("task-without-project-folder".to_owned());
    let error = host
        .resolve_git_cwd(&missing_context, None)
        .await
        .expect_err("git must not fall back to the configured workspace root");
    assert_eq!(error.code, "project_folder_required");
    assert_eq!(
        error.message,
        "git cwd was omitted; provide the project folder or an explicit absolute working path"
    );
}
