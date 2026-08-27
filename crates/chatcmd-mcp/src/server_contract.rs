use chatcmd_runtime::RuntimeError;
use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    tool_handler,
};
use serde_json::Value;

use super::McpServer;

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "IDENTITY: one ChatGPT chat equals one ChatCMD task; one user message equals one turn. Generate one unique turnId on the first ChatCMD call for a user message, then reuse it unchanged for every later call in that message. Before every ChatCMD call, reuse the newest taskId returned in this ChatGPT chat. Omit taskId only when this chat has never returned one. ChatCMD validates the private ChatGPT conversation identity server-side; a stale taskId from another chat must not merge two chats. When any ChatCMD shell or workspace tool is used in a user turn, agent_turn_complete MUST be called exactly once immediately before replying to the user. Use the same taskId and turnId as that turn's tools, pass the exact final user-facing response text as content, finish all other tool calls first, and do not call another tool afterward."
        )
    }
}

pub(super) fn error_value(error: &RuntimeError) -> Value {
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
    if lower.contains("authorization")
        || lower.contains("bearer ")
        || lower.contains("token=")
        || lower.contains("/mcp/")
    {
        "[REDACTED]".to_owned()
    } else {
        value.to_owned()
    }
}
