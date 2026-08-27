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
            "IDENTITY: one ChatGPT chat equals one ChatCMD task; one user message equals one turn. Generate one unique turnId for each user message and reuse it unchanged for every ChatCMD call in that message. FIRST TOOL RULE: before calling any other ChatCMD tool in a user turn, call agent_user_message with the exact current user message text as content and that turnId. Do not summarize, rewrite, or omit the user's text. Reuse the newest taskId returned in this ChatGPT chat; omit taskId only when this chat has never returned one. ChatCMD validates the private ChatGPT conversation identity server-side; a stale taskId from another chat must not merge two chats. The server rejects other tools until the current turn's user message has been synchronized. NEW CHAT RULE: only when agent_user_message returns isFirstMessage=true, the exact first user message participates in the Rust task ID seed and agent_turn_complete must include a concise suggestedTitle for that conversation; never rename it from later turns. When any ChatCMD tool is used in a user turn, agent_turn_complete MUST be called exactly once immediately before replying to the user. Use the same taskId and turnId as that turn's tools, pass the exact final user-facing response text as content, finish all other tool calls first, and do not call another tool afterward."
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
