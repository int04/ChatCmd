mod api;
mod runtime_host;
mod websocket;

use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use axum::{Router, routing::get};
use chatcmd_core::{
    McpAgentStore, PolicyLookup, ToolCapability, ToolCatalogStore, ToolDefinition, ToolGroup,
    ToolPreset,
};
use chatcmd_mcp::{AuthProvider, HttpSecurity, McpServer, OriginPolicy};
use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, EventSink, ExecutionPolicy, GitService, PolicyDecision,
    PolicyEngine, ProcessService, RuntimeConfig, RuntimeError, RuntimeResult, ShellRuntime,
    SkillService, TimelineEvent, WorkspaceService,
};
use chatcmd_storage::{SqliteRepository, resolve_database_path};
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;

use runtime_host::RuntimeHost;
use websocket::{AppEvent, AppState, ws_handler};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chat_cmd_client=info,tower_http=info".into()),
        )
        .init();

    let bind_address = std::env::var("CHATCMD_BIND").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let ip: IpAddr = bind_address
        .parse()
        .context("CHATCMD_BIND must be an IP address")?;
    let port = std::env::var("CHATCMD_PORT").map_or(Ok(8080_u16), |value| {
        value.parse().context("CHATCMD_PORT must be 1..65535")
    })?;
    if port == 0 {
        bail!("CHATCMD_PORT must be 1..65535");
    }
    let database_override = std::env::var_os("CHATCMD_DB_PATH").map(PathBuf::from);
    let database_path =
        resolve_database_path(database_override.as_deref()).context("resolve SQLite path")?;
    let (repository, bootstrap) = SqliteRepository::open(&database_path, 4)
        .await
        .context("open and bootstrap SQLite")?;
    seed_catalog(&repository)
        .await
        .context("seed MCP tool catalog")?;

    let root = std::env::current_dir()
        .context("resolve current workspace")?
        .canonicalize()
        .context("canonicalize workspace")?;
    let policy = ExecutionPolicy {
        default: PolicyDecision::Allow,
        per_agent_tool: BTreeMap::new(),
        per_root: BTreeMap::new(),
    };
    let policy_engine = PolicyEngine::new(Some(policy), Arc::new(RejectApproval));
    let (event_tx, _) = broadcast::channel(512);
    let event_sink: Arc<dyn EventSink> = Arc::new(BroadcastEvents(event_tx.clone()));
    let config = RuntimeConfig {
        roots: vec![root.clone()],
        user_home: user_home(),
        repository_root: Some(root.clone()),
        ..RuntimeConfig::default()
    };
    let workspace = WorkspaceService::new(&config.roots, policy_engine.clone())
        .context("initialize workspace service")?;
    let shell = ShellRuntime::new(config.clone(), policy_engine.clone(), event_sink);
    let git = GitService::new(workspace.clone(), 200_000);
    let process = ProcessService::new(policy_engine);
    let skills = SkillService::new(
        config.user_home.as_deref(),
        Some(&root),
        config.max_skill_characters,
    );
    let runtime = Arc::new(RuntimeHost::new(
        repository.clone(),
        bootstrap.device.clone(),
        shell.clone(),
        workspace,
        git,
        process,
        skills.clone(),
        event_tx.clone(),
    ));
    let activity_registry = runtime.activity_registry();
    let security = HttpSecurity::new(
        Arc::new(DatabaseAuth(repository.clone())),
        Arc::new(LocalOrigins {
            port,
            allow_missing: ip.is_loopback(),
        }),
    );
    let mcp = chatcmd_mcp::axum_router_with_host_validation(
        McpServer::new(runtime),
        security,
        !ip.is_loopback(),
    );
    let state = Arc::new(AppState::new(
        repository,
        database_path.display().to_string(),
        bind_address.clone(),
        port,
        bootstrap.device,
        shell,
        skills,
        activity_registry,
        event_tx,
    ));
    let frontend_dir = resolve_frontend_dir();
    let frontend_index = frontend_dir.join("index.html");
    let frontend = ServeDir::new(&frontend_dir).not_found_service(ServeFile::new(frontend_index));
    let management = Router::new()
        .nest("/api", api::router(state.clone()))
        .route("/ws", get(ws_handler))
        .fallback_service(frontend)
        .with_state(state);
    let app = mcp
        .merge(management)
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!(
                    "http.request",
                    method = %request.method(),
                    route = %trace_route(request.uri().path())
                )
            },
        ));
    let address = SocketAddr::new(ip, port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("bind {address}"))?;
    info!(%address, database=%database_path.display(), "ChatCmdClient started");
    axum::serve(listener, app)
        .await
        .context("serve local application")
}

fn resolve_frontend_dir() -> PathBuf {
    if let Some(configured) = std::env::var_os("CHATCMD_WEB_DIST") {
        return PathBuf::from(configured);
    }

    let working_dir = PathBuf::from("web/dist");
    if working_dir.join("index.html").is_file() {
        return working_dir;
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_dir) = executable.parent()
    {
        for candidate in [executable_dir.join("web/dist"), executable_dir.join("dist")] {
            if candidate.join("index.html").is_file() {
                return candidate;
            }
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist")
}

async fn seed_catalog(repository: &SqliteRepository) -> Result<(), chatcmd_core::StorageError> {
    let groups = vec![
        tool_group("group-device", "device", "Device", 10),
        tool_group("group-terminal", "terminal", "Terminal", 20),
        tool_group("group-files", "files", "Files & workspace", 30),
        tool_group("group-git", "git", "Git", 40),
        tool_group("group-process", "process", "Processes", 50),
        tool_group("group-skills", "skills", "Skills", 60),
        tool_group("group-tasks", "tasks", "Tasks & agent lifecycle", 70),
    ];
    let tools = chatcmd_mcp::TOOL_NAMES
        .iter()
        .map(|name| ToolDefinition {
            id: seeded_tool_id(name),
            key: (*name).to_owned(),
            group_id: tool_group_id(name).to_owned(),
            title: name.replace('_', " "),
            description: format!("Local {name} operation"),
            input_schema_json: "{}".to_owned(),
            capabilities: if [
                "fs_delete",
                "fs_move",
                "git_commit",
                "process_kill",
                "shell_close",
            ]
            .contains(name)
            {
                vec![ToolCapability::Destructive]
            } else if name.starts_with("fs_write") || *name == "fs_replace_text" {
                vec![ToolCapability::Write]
            } else {
                vec![ToolCapability::Read]
            },
            enabled: true,
        })
        .collect::<Vec<_>>();
    let safe_ids = tools
        .iter()
        .filter(|tool| !tool.capabilities.contains(&ToolCapability::Destructive))
        .map(|tool| tool.id.clone())
        .collect();
    let presets = vec![ToolPreset {
        id: "preset-safe".to_owned(),
        key: "safe".to_owned(),
        name: "Safe local tools".to_owned(),
        description: "All non-destructive local tools".to_owned(),
        tool_ids: safe_ids,
    }];
    repository
        .replace_catalog(&groups, &tools, &presets)
        .await?;

    let write_id = seeded_tool_id("fs_write_text");
    let replace_id = seeded_tool_id("fs_replace_text");
    for agent in repository.list_agents().await? {
        let mut allowed = repository.agent_allowed_tool_ids(&agent.id).await?;
        if allowed.contains(&write_id) && !allowed.contains(&replace_id) {
            allowed.push(replace_id.clone());
            repository
                .set_agent_allowed_tools(&agent.id, &allowed)
                .await?;
        }
    }
    Ok(())
}

fn tool_group(id: &str, key: &str, display_name: &str, sort_order: i32) -> ToolGroup {
    ToolGroup {
        id: id.to_owned(),
        key: key.to_owned(),
        display_name: display_name.to_owned(),
        sort_order,
    }
}

fn tool_group_id(name: &str) -> &'static str {
    if name.starts_with("device_") {
        "group-device"
    } else if name.starts_with("shell_") {
        "group-terminal"
    } else if name.starts_with("fs_") || name == "workspace_roots" {
        "group-files"
    } else if name.starts_with("git_") {
        "group-git"
    } else if name.starts_with("process_") {
        "group-process"
    } else if name.starts_with("skill_") || name.starts_with("skills_") {
        "group-skills"
    } else {
        "group-tasks"
    }
}
fn seeded_tool_id(name: &str) -> String {
    match name {
        "device_list" => "tool-device-list".to_owned(),
        "shell_create" => "tool-shell-create".to_owned(),
        "shell_read" => "tool-shell-read".to_owned(),
        "shell_write" => "tool-shell-write".to_owned(),
        "fs_read_text" => "tool-fs-read".to_owned(),
        _ => format!("tool-{name}"),
    }
}

struct DatabaseAuth(SqliteRepository);
impl AuthProvider for DatabaseAuth {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move {
            self.0
                .lookup_policy_by_token(token)
                .await
                .map_err(|_| RuntimeError::new("unauthorized", "MCP token validation failed"))?
                .map(|policy| policy.agent.id.into_string())
                .ok_or_else(|| RuntimeError::new("unauthorized", "invalid MCP path token"))
        })
    }
}

struct LocalOrigins {
    port: u16,
    allow_missing: bool,
}
impl OriginPolicy for LocalOrigins {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move {
            let allowed = if origin.is_empty() {
                self.allow_missing
            } else {
                [
                    format!("http://localhost:{}", self.port),
                    format!("http://127.0.0.1:{}", self.port),
                    format!("https://localhost:{}", self.port),
                    format!("https://127.0.0.1:{}", self.port),
                ]
                .iter()
                .any(|candidate| candidate == origin)
            };
            if allowed {
                Ok(())
            } else {
                Err(RuntimeError::new("origin_denied", "origin is not allowed"))
            }
        })
    }
}

struct RejectApproval;
impl ApprovalDecision for RejectApproval {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(false) })
    }
}

struct BroadcastEvents(broadcast::Sender<AppEvent>);
impl EventSink for BroadcastEvents {
    fn emit(&self, event: TimelineEvent) {
        let mut envelope = AppEvent::new(
            event.event_type,
            json!({ "status":event.status,"message":event.message,"toolName":event.tool_name,"metadata":event.metadata }),
        );
        envelope.task_id = event.task_id;
        envelope.turn_id = event.turn_id;
        let _ = self.0.send(envelope);
    }
}

fn trace_route(path: &str) -> &str {
    if path.starts_with("/mcp/") {
        "/mcp/{token}"
    } else {
        path
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn missing_origin_only_allowed_on_loopback() {
        let local = LocalOrigins {
            port: 8080,
            allow_missing: true,
        };
        assert!(local.authorize("").await.is_ok());
        let remote = LocalOrigins {
            port: 8080,
            allow_missing: false,
        };
        assert!(remote.authorize("").await.is_err());
        assert!(local.authorize("http://localhost:8080").await.is_ok());
        assert!(local.authorize("http://evil.example").await.is_err());
    }

    #[test]
    fn trace_route_never_logs_mcp_tokens() {
        assert_eq!(trace_route("/mcp/super-secret-token"), "/mcp/{token}");
        assert_eq!(trace_route("/api/health"), "/api/health");
    }
}
