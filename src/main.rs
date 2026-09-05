#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
// RuntimeError is intentionally a structured cross-layer error type; boxing it would alter APIs.
#![allow(clippy::result_large_err)]

mod api;
mod catalog_seed;
mod chatgpt_message;
mod chatgpt_queue;
mod chatgpt_transcript;
#[cfg(all(not(debug_assertions), any(target_os = "windows", target_os = "macos")))]
mod desktop_tray;
#[cfg(feature = "embedded-web")]
mod embedded_web;
mod gui_auth;
mod log_helper;
mod mutation_journal_bridge;
mod runtime_host;
mod version;
mod websocket;

use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use axum::{Router, routing::get};
use chatcmd_core::PolicyLookup;
use chatcmd_mcp::{AuthProvider, HttpSecurity, McpServer, OriginPolicy};
use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, CommandExecutionService, EventSink, ExecutionPolicy, GitService,
    PolicyDecision, PolicyEngine, ProcessService, RuntimeConfig, RuntimeError, RuntimeResult,
    ShellRuntime, SkillService, TimelineEvent, WorkspaceService,
};
use chatcmd_storage::{SqliteRepository, resolve_database_path};
use serde_json::json;
use tokio::sync::broadcast;
#[cfg(not(feature = "embedded-web"))]
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use catalog_seed::seed_catalog;
use tracing::info;

use runtime_host::RuntimeHost;
use websocket::{AppEvent, AppState, ws_handler};

#[cfg(debug_assertions)]
#[tokio::main]
async fn main() -> Result<()> {
    apply_elevated_restart_delay();
    run_server(None).await
}

#[cfg(all(not(debug_assertions), any(target_os = "windows", target_os = "macos")))]
fn main() -> Result<()> {
    apply_elevated_restart_delay();
    let port = configured_port()?;
    let management_url = format!("http://127.0.0.1:{port}");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("chatcmd-server".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("create ChatCMD Tokio runtime");
            if runtime.block_on(run_server(Some(ready_tx))).is_err() {
                std::process::exit(1);
            }
        })
        .context("start ChatCMD server thread")?;
    desktop_tray::run(management_url, ready_rx)
}

#[cfg(all(
    not(debug_assertions),
    not(any(target_os = "windows", target_os = "macos"))
))]
#[tokio::main]
async fn main() -> Result<()> {
    apply_elevated_restart_delay();
    run_server(None).await
}

fn apply_elevated_restart_delay() {
    let mut args = std::env::args().skip(1);
    let mut delay_ms = None;
    #[cfg(target_os = "macos")]
    let mut ready_port = None;
    #[cfg(target_os = "macos")]
    let mut ready_token = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--elevated-restart-delay-ms" => {
                delay_ms = args.next().and_then(|value| value.parse::<u64>().ok());
            }
            #[cfg(target_os = "macos")]
            "--elevated-restart-ready-port" => {
                ready_port = args.next().and_then(|value| value.parse::<u16>().ok());
            }
            #[cfg(target_os = "macos")]
            "--elevated-restart-ready-token" => {
                ready_token = args.next();
            }
            _ => {}
        }
    }
    #[cfg(target_os = "macos")]
    if let (Some(port), Some(token)) = (ready_port, ready_token) {
        if api::signal_elevated_restart_ready(port, &token).is_err() {
            std::process::exit(1);
        }
    }
    if let Some(delay_ms) = delay_ms {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms.min(5_000)));
    }
}

async fn run_server(ready: Option<std::sync::mpsc::Sender<()>>) -> Result<()> {
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
    let port = configured_port()?;
    let database_override = std::env::var_os("CHATCMD_DB_PATH").map(PathBuf::from);
    let database_path =
        resolve_database_path(database_override.as_deref()).context("resolve SQLite path")?;
    let (repository, bootstrap) = SqliteRepository::open(&database_path, 4)
        .await
        .context("open and bootstrap SQLite")?;
    #[cfg(debug_assertions)]
    info!(
        device_id = %bootstrap.device.id,
        installation_id = %bootstrap.device.installation_id,
        machine_id = bootstrap.device.machine_id.as_deref().unwrap_or("unavailable"),
        name = %bootstrap.device.name,
        platform = %bootstrap.device.platform,
        os_version = bootstrap.device.os_version.as_deref().unwrap_or("unknown"),
        architecture = %bootstrap.device.architecture,
        app_version = %bootstrap.device.app_version,
        created_at_ms = bootstrap.device.created_at_ms,
        updated_at_ms = bootstrap.device.updated_at_ms,
        "Debug device information"
    );
    seed_catalog(&repository)
        .await
        .context("seed MCP tool catalog")?;
    let mutation_journal_sink = Arc::new(
        mutation_journal_bridge::SqliteMutationJournalSink::start(repository.clone())
            .context("start filesystem journal persistence")?,
    );

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
        .context("initialize workspace service")?
        .with_mutation_journal_sink(mutation_journal_sink);
    let recovered_mutations = workspace
        .recover_interrupted_mutations()
        .await
        .context("recover interrupted filesystem mutations")?;
    if recovered_mutations > 0 {
        info!(
            recovered_mutations,
            "Recovered interrupted filesystem mutations"
        );
    }
    let blob_root = database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("blobs-v1");
    let blob_store = chatcmd_runtime::BlobStore::new(blob_root).context("initialize blob store")?;
    let blob_gc = blob_store.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(error) = blob_gc.gc() {
                tracing::warn!(error = ?error, "blob garbage collection failed");
            }
        }
    });
    let shell = ShellRuntime::new(config.clone(), policy_engine.clone(), event_sink);
    let git = GitService::new(workspace.clone(), 200_000);
    let command = CommandExecutionService::new(
        workspace.clone(),
        Arc::new(policy_engine.clone()),
        database_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("command-artifacts-v1"),
        config.max_concurrent_operations,
    );
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
        blob_store.clone(),
        git,
        command,
        process,
        skills.clone(),
        event_tx.clone(),
    ));
    let expired_plan_questions = runtime
        .expire_pending_plan_questions_on_startup()
        .await
        .context("expire pending plan questions from previous host session")?;
    if expired_plan_questions > 0 {
        info!(
            expired_plan_questions,
            "Expired pending plan questions after restart"
        );
    }
    runtime
        .restore_repository_indexes()
        .await
        .context("restore persisted repository indexes")?;
    runtime.start_repository_index_reconcile();
    let activity_registry = runtime.activity_registry();
    let plan_prompt_registry = runtime.plan_prompt_registry();
    let telemetry_registry = runtime.telemetry_registry();
    let _finalization_watchdog = runtime.start_finalization_watchdog();
    let security = HttpSecurity::new(
        Arc::new(DatabaseAuth(repository.clone())),
        Arc::new(LocalOrigins {
            port,
            allow_missing: ip.is_loopback(),
        }),
    );
    let mcp = chatcmd_mcp::axum_router_with_host_validation(
        McpServer::new(runtime.clone()),
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
        plan_prompt_registry,
        telemetry_registry,
        blob_store.clone(),
        event_tx,
    ));
    api::start_data_cleanup_scheduler(state.clone());
    let management = Router::new()
        .nest("/api", api::router(state.clone()))
        .route(
            "/ping",
            get(|| async { axum::Json(json!({ "pong": true, "service": "ChatCMD" })) }),
        )
        .route("/ws", get(ws_handler));
    #[cfg(feature = "embedded-web")]
    let management = management.fallback(embedded_web::serve).with_state(state);
    #[cfg(not(feature = "embedded-web"))]
    let management = {
        let frontend_dir = resolve_frontend_dir();
        let frontend_index = frontend_dir.join("index.html");
        let frontend =
            ServeDir::new(&frontend_dir).not_found_service(ServeFile::new(frontend_index));
        management.fallback_service(frontend).with_state(state)
    };
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
    if let Some(sender) = ready {
        let _ = sender.send(());
    }
    info!(%address, database=%database_path.display(), "ChatCmdClient started");
    axum::serve(listener, app)
        .await
        .context("serve local application")
}

fn configured_port() -> Result<u16> {
    let port = std::env::var("CHATCMD_PORT").map_or(Ok(8080_u16), |value| {
        value.parse().context("CHATCMD_PORT must be 1..65535")
    })?;
    if port == 0 {
        bail!("CHATCMD_PORT must be 1..65535");
    }
    Ok(port)
}

#[cfg(not(feature = "embedded-web"))]
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

    #[test]
    fn runtime_dispatcher_matches_generated_mcp_catalog() {
        let mut dispatched = [
            include_str!("runtime_host/dispatch.rs"),
            include_str!("runtime_host/dispatch/filesystem_tools.rs"),
        ]
        .into_iter()
        .flat_map(str::lines)
        .filter_map(|line| {
            let rest = line.strip_prefix("            \"")?;
            let (name, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then(|| name.to_owned())
        })
        .collect::<Vec<_>>();
        dispatched.sort_unstable();
        dispatched.dedup();
        assert_eq!(dispatched.as_slice(), chatcmd_mcp::TOOL_NAMES.as_slice());
    }
}
