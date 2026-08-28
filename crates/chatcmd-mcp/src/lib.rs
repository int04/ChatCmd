//! Official `rmcp` server surface for direct local ChatCMD execution.

mod request_identity;
mod server_contract;
mod subagent_protocol;
mod subagent_worker;
mod tool_catalog;

use axum::{
    Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
};
use chatcmd_runtime::{BoxFuture, DeviceDescriptor, OperationContext, RuntimeError, RuntimeResult};
use rmcp::{
    Peer, RoleServer,
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars, tool, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

use server_contract::error_value;

pub use tool_catalog::TOOL_NAMES;

/// Path-token authentication dependency injected by the HTTP host.
pub trait AuthProvider: Send + Sync {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>>;
}

tokio::task_local! {
    static AUTHENTICATED_AGENT_ID: String;
}

/// Origin allow-list dependency injected by the HTTP host.
pub trait OriginPolicy: Send + Sync {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>>;
}

/// Runtime dispatch boundary. Implementations inject policy, task, device, and local runtime services.
pub trait RuntimeApi: Send + Sync {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>>;

    fn local_device(&self) -> DeviceDescriptor;

    fn project_folder<'a>(
        &'a self,
        _agent_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Option<String>>> {
        Box::pin(async { Ok(None) })
    }

    fn fail_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>>;
}

/// Shared typed argument envelope. Unknown fields remain structured and never become shell text.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolArguments {
    /// Caller-generated idempotency key.
    #[serde(default)]
    pub request_id: String,
    /// Calling agent identifier used by policy checks.
    #[serde(default)]
    pub agent_id: String,
    /// Task correlation identifier.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Turn correlation identifier.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// HTTP-bound, server-derived correlation. Hidden from MCP input schema.
    #[serde(default, rename = "__chatcmdMcpSessionId")]
    #[schemars(skip)]
    pub(crate) authenticated_session_id: Option<String>,
    /// HTTP-bound private conversation identity. Hidden from MCP input schema.
    #[serde(default, rename = "__chatcmdConversationScopeId")]
    #[schemars(skip)]
    pub(crate) conversation_scope_id: Option<String>,
    /// Tool-specific typed JSON fields.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// Cloneable rmcp handler backed by injected services.
#[derive(Clone)]
pub struct McpServer {
    runtime: Arc<dyn RuntimeApi>,
}

impl McpServer {
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeApi>) -> Self {
        Self { runtime }
    }

    fn prepare_call(
        &self,
        tool_name: &'static str,
        arguments: ToolArguments,
    ) -> (OperationContext, Value) {
        let request_id = if arguments.request_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            arguments.request_id.clone()
        };
        let authenticated_agent = AUTHENTICATED_AGENT_ID
            .try_with(Clone::clone)
            .unwrap_or(arguments.agent_id);
        let mut context = OperationContext::new(request_id, authenticated_agent, tool_name);
        context.task_id = arguments.task_id;
        context.turn_id = arguments.turn_id;
        context.mcp_session_id = arguments.authenticated_session_id;
        context.conversation_scope_id = arguments.conversation_scope_id;
        let value = Value::Object(arguments.fields.into_iter().collect());
        (context, value)
    }

    async fn invoke(&self, tool_name: &'static str, arguments: ToolArguments) -> CallToolResult {
        let (context, value) = self.prepare_call(tool_name, arguments);
        match self.runtime.call(tool_name, context, value).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(error_value(&error)),
        }
    }

    async fn invoke_subagent_start(
        &self,
        arguments: ToolArguments,
        peer: Peer<RoleServer>,
    ) -> CallToolResult {
        let (context, value) = self.prepare_call("agent_subagent_start", arguments);
        let request = value
            .get("request")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let registration = match self
            .runtime
            .call("agent_subagent_start", context.clone(), value)
            .await
        {
            Ok(value) => value,
            Err(error) => return CallToolResult::structured_error(error_value(&error)),
        };
        let tools = Self::tool_router().list_all();
        let registered = registration.clone();
        match subagent_worker::dispatch_registered_subagent(
            self.runtime.clone(),
            peer,
            context,
            registration,
            &request,
            tools,
        )
        .await
        {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => {
                if let Some(child_task_id) = registered
                    .get("childTaskId")
                    .or_else(|| registered.get("taskId"))
                    .and_then(Value::as_str)
                {
                    let _ = self
                        .runtime
                        .fail_subagent(child_task_id, &error.message)
                        .await;
                }
                let mut failed = registered;
                if let Some(object) = failed.as_object_mut() {
                    object.insert("status".to_owned(), Value::String("failed".to_owned()));
                    object.insert(
                        "dispatchMode".to_owned(),
                        Value::String("failed".to_owned()),
                    );
                    object.insert("workerStarted".to_owned(), Value::Bool(false));
                    object.insert(
                        "startupError".to_owned(),
                        serde_json::json!({"code": error.code, "message": error.message}),
                    );
                }
                CallToolResult::structured(failed)
            }
        }
    }
}

macro_rules! tool_methods {
    ($(($method:ident, $description:literal)),+ $(,)?) => {
        #[tool_router]
        impl McpServer {
            $(
                #[tool(description = $description)]
                async fn $method(
                    &self,
                    Parameters(arguments): Parameters<ToolArguments>,
                ) -> CallToolResult {
                    self.invoke(stringify!($method), arguments).await
                }
            )+

            #[tool(description = "Create and dispatch one child agent. Pass the AI-chosen name and delegated request. ChatCMD uses server-to-model sampling when available; otherwise it starts a local Codex CLI worker in a read-only sandbox inside the reserved child task.")]
            async fn agent_subagent_start(
                &self,
                Parameters(arguments): Parameters<ToolArguments>,
                peer: Peer<RoleServer>,
            ) -> CallToolResult {
                self.invoke_subagent_start(arguments, peer).await
            }
        }
    };
}

tool_methods!(
    (device_list, "List available execution devices"),
    (device_get, "Inspect one execution device"),
    (
        shell_create,
        "Create a persistent cross-platform PTY session"
    ),
    (shell_write, "Write literal input to a PTY session"),
    (
        shell_wait,
        "Wait without killing the PTY when timeout expires"
    ),
    (shell_read, "Read bounded replayable PTY output"),
    (shell_signal, "Send a portable terminal signal"),
    (shell_resize, "Resize a PTY session"),
    (shell_close, "Close or explicitly force-close a PTY session"),
    (shell_list, "List PTY sessions"),
    (shell_inspect, "Inspect a PTY session"),
    (workspace_roots, "List canonical workspace roots"),
    (fs_list, "List workspace directory entries"),
    (fs_search, "Search text within the workspace"),
    (fs_find, "Find workspace paths"),
    (
        fs_read_text,
        "Read UTF-8 workspace text. Fields: path; optional maxCharacters, startLine (1-based), lineCount. Prefer line ranges for large files."
    ),
    (fs_write_text, "Atomically write UTF-8 workspace text"),
    (
        fs_replace_text,
        "Safely edit an existing UTF-8 file by exact text replacement. Fields: path, oldText, newText, optional expectedOccurrences (default 1). Prefer this over Python/PowerShell scripts for targeted text edits."
    ),
    (
        fs_write_raw,
        "Atomically write Base64-decoded workspace bytes"
    ),
    (fs_stat, "Inspect workspace path metadata"),
    (fs_create_directory, "Create a workspace directory"),
    (fs_copy, "Copy within canonical workspace scope"),
    (fs_move, "Move within canonical workspace scope"),
    (
        fs_delete,
        "Delete within canonical workspace scope under policy"
    ),
    (git_status, "Get Git working tree status"),
    (
        git_diff,
        "Get argument-safe Git diff output with optional stat summary"
    ),
    (git_log, "Get bounded Git history"),
    (git_branch, "List Git branches"),
    (git_show, "Show a validated Git revision"),
    (
        git_commit,
        "Create a Git commit without shell interpolation"
    ),
    (process_list, "List local processes"),
    (process_inspect, "Inspect a local process"),
    (process_kill, "Terminate a local process under policy"),
    (
        skills_list,
        "After agent_user_message, discover available .agents and .codex skills before non-trivial project work; use skill_read for relevant matches before doing that work"
    ),
    (
        skill_read,
        "Read a relevant matching skill before performing the work it governs, then follow the returned bounded instructions"
    ),
    (task_get, "Read task state"),
    (task_list, "List tasks"),
    (task_set_execution_mode, "Set task execution mode"),
    (task_artifact_list, "List task artifacts"),
    (task_artifact_read, "Read a task artifact"),
    (
        agent_user_message,
        "MANDATORY FIRST TOOL: call this exactly once at the start of every user turn before any other ChatCMD tool. Pass the exact current user message text in content without summarizing or rewriting it. Reuse the same turnId for all later tools in this turn. On a brand-new chat, ChatCMD uses this exact first message together with the private conversation scope as the stable task identity seed and returns isFirstMessage=true. Retries are idempotent."
    ),
    (
        agent_progress,
        "Publish one concise progress milestone during non-trivial work. Do not call it after agent_turn_complete."
    ),
    (
        agent_subagent_wait,
        "Wait for child agents registered by the current parent turn. Repeat while allFinished=false before finalizing the parent response."
    ),
    (
        agent_turn_complete,
        "MANDATORY FINALIZATION: if any ChatCMD tool was used in this user turn, call this exactly once immediately before replying to the user, after every other tool call has finished. Pass the exact final user-facing response text in content. If agent_user_message returned isFirstMessage=true, also pass one concise conversation title in suggestedTitle; do this only for that first turn. Do not call another tool afterward."
    ),
);

/// Fail-closed HTTP security state.
#[derive(Clone)]
pub struct HttpSecurity {
    auth: Arc<dyn AuthProvider>,
    origins: Arc<dyn OriginPolicy>,
}

impl HttpSecurity {
    #[must_use]
    pub fn new(auth: Arc<dyn AuthProvider>, origins: Arc<dyn OriginPolicy>) -> Self {
        Self { auth, origins }
    }

    async fn authorize(
        &self,
        token: &str,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> RuntimeResult<String> {
        if query.is_some_and(has_query_token) {
            return Err(RuntimeError::new(
                "query_token_rejected",
                "authentication token in query is forbidden",
            ));
        }
        if token.is_empty() || token.len() > 512 {
            return Err(RuntimeError::new(
                "unauthorized",
                "valid MCP path token is required",
            ));
        }
        let agent_id = self.auth.authorize(token).await?;
        self.origins
            .authorize(
                headers
                    .get("origin")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
            )
            .await?;
        Ok(agent_id)
    }
}

fn has_query_token(query: &str) -> bool {
    query.split('&').any(|pair| {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        matches!(
            name.to_ascii_lowercase().as_str(),
            "token" | "access_token" | "bearer_token"
        )
    })
}

#[derive(Clone)]
struct McpHttpState {
    security: HttpSecurity,
    service: StreamableHttpService<McpServer, LocalSessionManager>,
}

async fn mcp_handler(
    State(state): State<McpHttpState>,
    Path(token): Path<String>,
    mut request: Request<Body>,
) -> Response {
    let agent_id = match state
        .security
        .authorize(&token, request.headers(), request.uri().query())
        .await
    {
        Ok(agent_id) => agent_id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let header_session = request
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok());
    let session_id = local_mcp_session_id(&agent_id, header_session);

    // Bind tool identity and safe correlation before rmcp moves work into its session task.
    // Client-provided `agent_id` values (for example a ChatGPT connector name) never win.
    request =
        match request_identity::bind_authenticated_agent(request, &agent_id, &session_id).await {
            Ok(request) => request,
            Err(status) => return status.into_response(),
        };

    // Keep the credential at the HTTP boundary. Downstream rmcp handlers receive
    // a stable credential-free URI and the authenticated agent identity only.
    *request.uri_mut() = Uri::from_static("/mcp");
    match AUTHENTICATED_AGENT_ID
        .scope(agent_id, state.service.clone().oneshot(request))
        .await
    {
        Ok(response) => response.into_response(),
        Err(infallible) => match infallible {},
    }
}

fn local_mcp_session_id(agent_id: &str, header_session: Option<&str>) -> String {
    let scope = header_session.unwrap_or("agent-fallback");
    let material = format!("agent:{agent_id}\0session:{scope}");
    format!(
        "mcp-session-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

/// Build reusable Streamable HTTP service with local rmcp session management.
pub fn streamable_http_service(
    server: McpServer,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    streamable_http_service_with_config(server, StreamableHttpServerConfig::default())
}

fn streamable_http_service_with_config(
    server: McpServer,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    )
}

/// Build an Axum router protected by a token path segment and Origin checks.
pub fn axum_router(server: McpServer, security: HttpSecurity) -> Router {
    axum_router_with_host_validation(server, security, true)
}

/// Build an Axum router while optionally disabling rmcp Host validation.
///
/// Host validation should only be disabled when the listener itself is loopback-only
/// and an external reverse proxy is the sole public ingress. Token and Origin checks
/// remain active at the ChatCmdClient boundary.
pub fn axum_router_with_host_validation(
    server: McpServer,
    security: HttpSecurity,
    validate_host: bool,
) -> Router {
    let config = if validate_host {
        StreamableHttpServerConfig::default()
    } else {
        StreamableHttpServerConfig::default().disable_allowed_hosts()
    };
    let state = McpHttpState {
        security,
        service: streamable_http_service_with_config(server, config),
    };
    Router::new()
        .route("/mcp/{token}", any(mcp_handler))
        .with_state(state)
}

#[cfg(test)]
mod lib_tests;
