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
        _task_id: Option<&'a str>,
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

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CommonToolArgs {
    /// Caller-generated idempotency key. Usually omit and let ChatCMD generate one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    request_id: String,
    /// Calling agent identifier. The authenticated server identity overrides this value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    agent_id: String,
    /// Task correlation identifier returned by agent_user_message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// Turn correlation identifier reused for every call in the current user turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdMcpSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    authenticated_session_id: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdConversationScopeId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    conversation_scope_id: Option<String>,
}

macro_rules! tool_args {
    ($name:ident {}) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
        }
    };
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),+ $(,)? }) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
            $(
                $(#[$meta])*
                $field: $ty,
            )+
        }
    };
}

tool_args!(NoArgs {});
tool_args!(DeviceGetArgs { device_id: String });
tool_args!(SessionArgs { session_id: String });
tool_args!(PathArgs { path: String });
tool_args!(CwdArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>
});
tool_args!(SkillArgs {
    #[serde(alias = "id")]
    skill_id: String
});
tool_args!(ProcessArgs { process_id: u32 });
tool_args!(ArtifactArgs {
    artifact_id: String
});
tool_args!(ExecutionModeArgs { mode: String });
tool_args!(ShellResizeArgs {
    session_id: String,
    columns: u16,
    rows: u16
});
tool_args!(TransferArgs {
    source: String,
    destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>
});
tool_args!(DeleteArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>
});
tool_args!(GitShowArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>
});
tool_args!(GitCommitArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>
});
tool_args!(ProcessKillArgs {
    process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entire_tree: Option<bool>
});
tool_args!(UserMessageArgs { content: String });
tool_args!(ProgressArgs {
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>
});
tool_args!(CompleteArgs {
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>
});
tool_args!(ShellCreateArgs {
    #[serde(default, alias = "cwd", alias = "initialWorkingDirectory", skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rows: Option<u16>
});
tool_args!(ShellWriteArgs {
    session_id: String,
    #[serde(alias = "input")]
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    append_new_line: Option<bool>
});
tool_args!(ShellWaitArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>
});
tool_args!(ShellReadArgs {
    session_id: String,
    #[serde(default, alias = "startSequence", alias = "fromSequence", skip_serializing_if = "Option::is_none")]
    after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_events: Option<usize>
});
tool_args!(ShellSignalArgs {
    session_id: String,
    signal: String
});
tool_args!(ShellCloseArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    force: Option<bool>
});
tool_args!(ListArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>
});
tool_args!(SearchArgs {
    path: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude: Option<Vec<String>>
});
tool_args!(FindArgs {
    path: String,
    pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_depth: Option<usize>
});
tool_args!(ReadArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_characters: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_count: Option<usize>
});
tool_args!(WriteTextArgs {
    path: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>
});
tool_args!(ReplaceTextArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_occurrences: Option<usize>
});
tool_args!(WriteRawArgs {
    path: String,
    base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>
});
tool_args!(GitDiffArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>
});
tool_args!(GitLogArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>
});
tool_args!(SubagentStartArgs {
    name: String,
    request: String
});
tool_args!(SubagentWaitArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>
});

fn into_tool_arguments<T: Serialize>(arguments: T) -> ToolArguments {
    serde_json::from_value(serde_json::to_value(arguments).expect("typed MCP arguments serialize"))
        .expect("typed MCP arguments convert to shared envelope")
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
    ($(($method:ident, $args:ty, $description:literal)),+ $(,)?) => {
        #[tool_router]
        impl McpServer {
            $(
                #[tool(description = $description)]
                async fn $method(
                    &self,
                    Parameters(arguments): Parameters<$args>,
                ) -> CallToolResult {
                    self.invoke(stringify!($method), into_tool_arguments(arguments)).await
                }
            )+

            #[tool(description = "Create and dispatch one child agent. Required fields: name, request. Pass the AI-chosen name and delegated request. ChatCMD delegates only through model sampling advertised by the connected ChatGPT/MCP host. If sampling is unavailable, no local executor is started and the child is returned as failed so the parent can continue or use host-native delegation when available.")]
            async fn agent_subagent_start(
                &self,
                Parameters(arguments): Parameters<SubagentStartArgs>,
                peer: Peer<RoleServer>,
            ) -> CallToolResult {
                self.invoke_subagent_start(into_tool_arguments(arguments), peer).await
            }
        }
    };
}

tool_methods!(
    (
        device_list,
        NoArgs,
        "List available execution devices. No tool-specific fields."
    ),
    (
        device_get,
        DeviceGetArgs,
        "Inspect one execution device. Required field: deviceId."
    ),
    (
        shell_create,
        ShellCreateArgs,
        "Create a persistent cross-platform PTY session. Canonical working-directory field is workingDirectory; cwd and initialWorkingDirectory are accepted compatibility aliases."
    ),
    (
        shell_write,
        ShellWriteArgs,
        "Write literal input to a PTY session. Required fields: sessionId, text. input is accepted as a compatibility alias for text."
    ),
    (
        shell_wait,
        ShellWaitArgs,
        "Wait without killing the PTY when timeout expires. Required field: sessionId; optional timeoutMs."
    ),
    (
        shell_read,
        ShellReadArgs,
        "Read bounded replayable PTY output. Required field: sessionId; canonical cursor field is afterSequence; startSequence and fromSequence are accepted compatibility aliases."
    ),
    (
        shell_signal,
        ShellSignalArgs,
        "Send a portable terminal signal. Required fields: sessionId, signal."
    ),
    (
        shell_resize,
        ShellResizeArgs,
        "Resize a PTY session. Required fields: sessionId, columns, rows."
    ),
    (
        shell_close,
        ShellCloseArgs,
        "Close or explicitly force-close a PTY session. Required field: sessionId; optional force."
    ),
    (
        shell_list,
        NoArgs,
        "List PTY sessions. No tool-specific fields."
    ),
    (
        shell_inspect,
        SessionArgs,
        "Inspect a PTY session. Required field: sessionId."
    ),
    (
        workspace_roots,
        NoArgs,
        "List roots granted to the current task/conversation. When the task has a project folder, this returns that folder rather than a process-wide or Agent workspace. No tool-specific fields."
    ),
    (
        fs_list,
        ListArgs,
        "List workspace directory entries. Required field: path; optional offset, limit."
    ),
    (
        fs_search,
        SearchArgs,
        "Search text within the workspace. Required fields: path, query; optional caseSensitive, maxResults, maxFileBytes, includeIgnored, exclude. Use '.' for the workspace root rather than an empty path."
    ),
    (
        fs_find,
        FindArgs,
        "Find workspace paths. Required fields: path, pattern; optional maxResults, maxDepth. Use this when a relative file path is uncertain."
    ),
    (
        fs_read_text,
        ReadArgs,
        "Read UTF-8 workspace text. Required field: path; optional maxCharacters, startLine (1-based), lineCount. Prefer line ranges for large files. If a path is uncertain, use fs_find first instead of guessing."
    ),
    (
        fs_write_text,
        WriteTextArgs,
        "Atomically write UTF-8 workspace text. Required fields: path, content; optional overwrite."
    ),
    (
        fs_replace_text,
        ReplaceTextArgs,
        "Safely edit an existing UTF-8 file by exact text replacement. Required fields: path, oldText, newText; optional expectedOccurrences (default 1). oldText must exactly match current file contents; read the target range first when content may have changed."
    ),
    (
        fs_write_raw,
        WriteRawArgs,
        "Atomically write Base64-decoded workspace bytes. Required fields: path, base64; optional overwrite."
    ),
    (
        fs_stat,
        PathArgs,
        "Inspect workspace path metadata. Required field: path."
    ),
    (
        fs_create_directory,
        PathArgs,
        "Create a workspace directory. Required field: path."
    ),
    (
        fs_copy,
        TransferArgs,
        "Copy within canonical workspace scope. Required fields: source, destination; optional overwrite."
    ),
    (
        fs_move,
        TransferArgs,
        "Move within canonical workspace scope. Required fields: source, destination; optional overwrite."
    ),
    (
        fs_delete,
        DeleteArgs,
        "Delete within canonical workspace scope under policy. Required field: path; optional recursive."
    ),
    (
        git_status,
        CwdArgs,
        "Get Git working tree status. Optional cwd; legacy path is accepted as a cwd alias."
    ),
    (
        git_diff,
        GitDiffArgs,
        "Get argument-safe Git diff output. Optional cwd, staged, stat, path. cwd selects the repository; path filters a file within it."
    ),
    (
        git_log,
        GitLogArgs,
        "Get bounded Git history. Optional cwd, count, path."
    ),
    (
        git_branch,
        CwdArgs,
        "List Git branches. Optional cwd; legacy path is accepted as a cwd alias."
    ),
    (
        git_show,
        GitShowArgs,
        "Show a validated Git revision. Required revision; optional cwd and path."
    ),
    (
        git_commit,
        GitCommitArgs,
        "Create a Git commit without shell interpolation. Required field: message; optional cwd, all (default true), paths. Never call with an empty object."
    ),
    (
        process_list,
        NoArgs,
        "List local processes. No tool-specific fields."
    ),
    (
        process_inspect,
        ProcessArgs,
        "Inspect a local process. Required field: processId."
    ),
    (
        process_kill,
        ProcessKillArgs,
        "Terminate a local process under policy. Required field: processId; optional entireTree."
    ),
    (
        skills_list,
        NoArgs,
        "After agent_user_message, discover available .agents and .codex skills before non-trivial project work; no tool-specific fields."
    ),
    (
        skill_read,
        SkillArgs,
        "Read a relevant matching skill. Required field: skillId; id is accepted as a compatibility alias."
    ),
    (
        task_get,
        NoArgs,
        "Read current task state. Uses taskId correlation from the common fields."
    ),
    (task_list, NoArgs, "List tasks. No tool-specific fields."),
    (
        task_set_execution_mode,
        ExecutionModeArgs,
        "Set task execution mode. Required field: mode."
    ),
    (
        task_artifact_list,
        NoArgs,
        "List task artifacts. Uses taskId correlation from the common fields."
    ),
    (
        task_artifact_read,
        ArtifactArgs,
        "Read a task artifact. Required field: artifactId."
    ),
    (
        agent_user_message,
        UserMessageArgs,
        "MANDATORY FIRST TOOL: call exactly once at the start of every user turn. Required field: content containing the exact current user message. Reuse the same turnId for all calls in that turn. Never use agent_user_message for progress, reflections, findings, or commentary after tool results; use agent_progress for those updates."
    ),
    (
        agent_progress,
        ProgressArgs,
        "Publish one concise progress milestone. Required field: message; optional suggestedTitle. For non-trivial project work, call after meaningful file/code inspection, after successful file edits/creates, and after build/test/lint/search/Git/command/deploy results that materially help the user; summarize observable findings or effects, never private chain-of-thought. Do not spam tiny mechanical/no-op calls. Do not call after agent_turn_complete."
    ),
    (
        agent_subagent_wait,
        SubagentWaitArgs,
        "Wait for child agents registered by the current parent turn. Optional timeoutMs. Repeat while allFinished=false before finalizing."
    ),
    (
        agent_turn_complete,
        CompleteArgs,
        "MANDATORY FINALIZATION: call exactly once immediately before replying after every other tool call has finished. Required field: content with the exact final user-facing response; optional suggestedTitle only on the first message of a chat."
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
