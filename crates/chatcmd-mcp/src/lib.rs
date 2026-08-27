//! Official `rmcp` server surface for direct local ChatCMD execution.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
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

/// Stable ordered tool names exposed by this server.
pub const TOOL_NAMES: &[&str] = &[
    "device_list",
    "device_get",
    "shell_create",
    "shell_write",
    "shell_wait",
    "shell_read",
    "shell_signal",
    "shell_resize",
    "shell_close",
    "shell_list",
    "shell_inspect",
    "workspace_roots",
    "fs_list",
    "fs_search",
    "fs_find",
    "fs_read_text",
    "fs_write_text",
    "fs_write_raw",
    "fs_stat",
    "fs_create_directory",
    "fs_copy",
    "fs_move",
    "fs_delete",
    "git_status",
    "git_diff",
    "git_log",
    "git_branch",
    "git_show",
    "git_commit",
    "process_list",
    "process_inspect",
    "process_kill",
    "skills_list",
    "skill_read",
    "task_get",
    "task_list",
    "task_set_execution_mode",
    "task_artifact_list",
    "task_artifact_read",
    "agent_progress",
    "agent_turn_complete",
];

/// Authentication dependency injected by the HTTP host.
pub trait AuthProvider: Send + Sync {
    fn authorize<'a>(&'a self, bearer_token: &'a str) -> BoxFuture<'a, RuntimeResult<String>>;
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
        let value = Value::Object(arguments.fields.into_iter().collect());
        match self.runtime.call(tool_name, context, value).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(error_value(&error)),
        }
    }
}

macro_rules! tool_methods {
    ($(($method:ident, $description:literal)),+ $(,)?) => {
        #[tool_router(server_handler)]
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
    (git_diff, "Get argument-safe Git diff output"),
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
    (agent_progress, "Publish structured agent progress"),
    (agent_turn_complete, "Publish agent turn completion"),
);

fn error_value(error: &RuntimeError) -> Value {
    serde_json::json!({
        "error": {
            "code": error.code,
            "message": redact(&error.message),
            "retryable": error.retryable,
            "approvalRequired": error.approval_required
        }
    })
}

fn redact(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if lower.contains("authorization") || lower.contains("bearer ") || lower.contains("token=") {
        "[REDACTED]".to_owned()
    } else {
        value.to_owned()
    }
}

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

    async fn authorize(&self, headers: &HeaderMap, query: Option<&str>) -> RuntimeResult<String> {
        if query.is_some_and(has_query_token) {
            return Err(RuntimeError::new(
                "query_token_rejected",
                "authentication token in query is forbidden",
            ));
        }
        let authorization = header(headers, "authorization")?;
        let token = authorization
            .strip_prefix("Bearer ")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RuntimeError::new("unauthorized", "valid Bearer authorization is required")
            })?;
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

fn header<'a>(headers: &'a HeaderMap, name: &str) -> RuntimeResult<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            RuntimeError::new(
                "unauthorized",
                format!("required {name} header is missing or invalid"),
            )
        })
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

async fn security_middleware(
    State(security): State<HttpSecurity>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let agent_id = security
        .authorize(request.headers(), request.uri().query())
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    Ok(AUTHENTICATED_AGENT_ID
        .scope(agent_id, next.run(request))
        .await)
}

/// Build reusable Streamable HTTP service with local rmcp session management.
pub fn streamable_http_service(
    server: McpServer,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    )
}

/// Build an Axum/Tower router protected by mandatory Bearer and Origin checks.
pub fn axum_router(server: McpServer, security: HttpSecurity) -> Router {
    Router::new()
        .nest_service("/mcp", streamable_http_service(server))
        .layer(middleware::from_fn_with_state(
            security,
            security_middleware,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Accept;

    impl AuthProvider for Accept {
        fn authorize<'a>(&'a self, _bearer_token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
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
    }

    #[test]
    fn query_tokens_are_rejected() {
        assert!(has_query_token("access_token=secret"));
        assert!(!has_query_token("cursor=token-value"));
    }

    #[tokio::test]
    async fn bearer_and_origin_fail_closed() {
        let security = HttpSecurity::new(Arc::new(Accept), Arc::new(Accept));
        let empty = HeaderMap::new();
        assert_eq!(
            security
                .authorize(&empty, None)
                .await
                .expect_err("missing bearer")
                .code,
            "unauthorized"
        );

        let mut denied = HeaderMap::new();
        denied.insert("authorization", "Bearer secret".parse().expect("header"));
        denied.insert("origin", "https://denied.example".parse().expect("header"));
        assert_eq!(
            security
                .authorize(&denied, None)
                .await
                .expect_err("denied origin")
                .code,
            "origin_denied"
        );

        let mut no_origin = HeaderMap::new();
        no_origin.insert("authorization", "Bearer secret".parse().expect("header"));
        assert_eq!(
            security
                .authorize(&no_origin, None)
                .await
                .expect_err("missing origin must be decided by policy")
                .code,
            "origin_denied"
        );
    }
}
