//! Official `rmcp` server surface for direct local ChatCMD execution.

mod request_identity;
mod server_contract;
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

    async fn invoke(&self, tool_name: &'static str, arguments: ToolArguments) -> CallToolResult {
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
        match self.runtime.call(tool_name, context, value).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(error_value(&error)),
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
    (fs_read_text, "Read bounded UTF-8 workspace text"),
    (fs_write_text, "Atomically write UTF-8 workspace text"),
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
    (skills_list, "Discover .agents and .codex skills"),
    (skill_read, "Read bounded skill instructions"),
    (task_get, "Read task state"),
    (task_list, "List tasks"),
    (task_set_execution_mode, "Set task execution mode"),
    (task_artifact_list, "List task artifacts"),
    (task_artifact_read, "Read a task artifact"),
    (
        agent_user_message,
        "MANDATORY FIRST TOOL: call this exactly once at the start of every user turn before any other ChatCMD tool. Pass the exact current user message text in content without summarizing or rewriting it. Reuse the same turnId for all later tools in this turn. Retries are idempotent."
    ),
    (
        agent_progress,
        "Publish one concise progress milestone during non-trivial work. Do not call it after agent_turn_complete."
    ),
    (
        agent_turn_complete,
        "MANDATORY FINALIZATION: if any ChatCMD tool was used in this user turn, call this exactly once immediately before replying to the user, after every other tool call has finished. Pass the exact final user-facing response text in the content field. Do not call another tool afterward."
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
mod tests {
    use super::*;

    struct Accept;

    impl AuthProvider for Accept {
        fn authorize<'a>(&'a self, _token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
            Box::pin(async { Ok("agent-test".to_owned()) })
        }
    }

    impl OriginPolicy for Accept {
        fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>> {
            Box::pin(async move {
                if origin == "https://allowed.example" {
                    Ok(())
                } else {
                    Err(RuntimeError::new("origin_denied", "origin is not allowed"))
                }
            })
        }
    }

    #[test]
    fn catalog_names_are_stable_and_unique() {
        let mut names = TOOL_NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOL_NAMES.len());
        assert_eq!(TOOL_NAMES.first(), Some(&"device_list"));
        assert_eq!(TOOL_NAMES.last(), Some(&"agent_turn_complete"));
        assert!(TOOL_NAMES.contains(&"agent_user_message"));
    }

    #[test]
    fn query_tokens_are_rejected() {
        assert!(has_query_token("access_token=secret"));
        assert!(!has_query_token("cursor=token-value"));
    }

    #[test]
    fn local_session_correlation_is_stable_and_secret_free() {
        let first = local_mcp_session_id("agent-test", Some("remote-secret-session"));
        let second = local_mcp_session_id("agent-test", Some("remote-secret-session"));
        assert_eq!(first, second);
        assert!(!first.contains("remote-secret-session"));
        assert_ne!(
            first,
            local_mcp_session_id("other-agent", Some("remote-secret-session"))
        );
        assert_eq!(
            local_mcp_session_id("agent-test", None),
            local_mcp_session_id("agent-test", None)
        );
    }

    #[tokio::test]
    async fn path_token_and_origin_fail_closed() {
        let security = HttpSecurity::new(Arc::new(Accept), Arc::new(Accept));
        let mut legacy_header = HeaderMap::new();
        legacy_header.insert("authorization", "Bearer secret".parse().expect("header"));
        legacy_header.insert("origin", "https://allowed.example".parse().expect("header"));
        assert_eq!(
            security
                .authorize("", &legacy_header, None)
                .await
                .expect_err("authorization header must not replace the path token")
                .code,
            "unauthorized"
        );

        let mut denied = HeaderMap::new();
        denied.insert("origin", "https://denied.example".parse().expect("header"));
        assert_eq!(
            security
                .authorize("secret", &denied, None)
                .await
                .expect_err("denied origin")
                .code,
            "origin_denied"
        );

        let no_origin = HeaderMap::new();
        assert_eq!(
            security
                .authorize("secret", &no_origin, None)
                .await
                .expect_err("missing origin must be decided by policy")
                .code,
            "origin_denied"
        );

        let mut allowed = HeaderMap::new();
        allowed.insert("origin", "https://allowed.example".parse().expect("header"));
        assert_eq!(
            security
                .authorize("secret", &allowed, None)
                .await
                .expect("path token and origin are valid"),
            "agent-test"
        );
        assert_eq!(
            security
                .authorize("secret", &allowed, Some("access_token=other"))
                .await
                .expect_err("query credentials stay unsupported")
                .code,
            "query_token_rejected"
        );
    }
}
