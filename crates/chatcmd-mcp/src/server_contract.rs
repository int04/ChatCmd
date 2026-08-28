use chatcmd_runtime::RuntimeError;
use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    tool_handler,
};
use serde_json::Value;

use super::McpServer;

const SERVER_INSTRUCTIONS: &str = "IDENTITY: one ChatGPT chat equals one ChatCMD task; one user message equals one turn. Generate one unique turnId for each user message and reuse it unchanged for every ChatCMD call in that message. FIRST TOOL RULE: before calling any other ChatCMD tool in a user turn, call agent_user_message with the exact current user message text as content and that turnId. Do not summarize, rewrite, or omit the user's text. Reuse the newest taskId returned in this ChatGPT chat; omit taskId only when this chat has never returned one. ChatCMD validates the private ChatGPT conversation identity server-side; a stale taskId from another chat must not merge two chats. The server rejects other tools until the current turn's user message has been synchronized. SKILL RULE: after agent_user_message and before repository inspection, design decisions, code changes, or other non-trivial project work, call skills_list once to discover available .agents and .codex skills. Compare the returned skill descriptions with the current user request and intended work. If any skill matches, call skill_read for every relevant matching skill before doing the matching work, then follow those skill instructions. A directly matching skill is mandatory, not optional; do not infer its instructions from the skill name or description alone. For example, UI/color/layout/accessibility work must read a matching UI/UX skill when present, and Rust implementation/review work must read a matching Rust skill when present. Skip skill discovery only for trivial conversational turns or turns that do not require project work. PATH RULE: an existing absolute filesystem path explicitly present in any user message of the current ChatCMD task is a task-scoped access grant for that exact file or directory subtree, even when it is outside configured workspace roots. Use it directly when relevant, including in later turns such as when the user says to continue. Never widen that grant to a parent, sibling, different drive, or another path the user did not write; a path from another task/chat is not granted. EDIT RULE: for targeted text changes, use fs_replace_text; use fs_write_text for whole-file creation or replacement. Do not create or run Python, PowerShell, Node, or shell scripts merely to edit text when native filesystem tools can perform the change; use shell only when the native tools cannot express the required edit. NEW CHAT RULE: only when agent_user_message returns isFirstMessage=true, the exact first user message participates in the Rust task ID seed and agent_turn_complete must include a concise suggestedTitle for that conversation; never rename it from later turns. When any ChatCMD tool is used in a user turn, agent_turn_complete MUST be called exactly once immediately before replying to the user. Use the same taskId and turnId as that turn's tools, pass the exact final user-facing response text as content, finish all other tool calls first, and do not call another tool afterward. SUB-AGENT RULE: call agent_subagent_start once for each delegated child with a concise AI-chosen name and request. The result keeps taskId as the parent coordinator task and exposes childTaskId as the child conversation/task; never replace the parent taskId with childTaskId in later parent calls. Registration is idempotent within one parent turn by name plus delegated request, so a retry returns the same subagentId/childTaskId with duplicate=true instead of creating another child. Inspect dispatchMode: samplingTools or samplingText means ChatCMD is running the child through MCP sampling; localCodex means ChatCMD started a local Codex CLI worker in a read-only sandbox; existing means the same child was already registered/claimed and must not be spawned again. If startup fails after registration, agent_subagent_start returns a normal structured result with status=failed and startupError rather than a tool-level error; do not blindly retry it. Do not create a duplicate host-native child. Before agent_turn_complete in the parent turn, call agent_subagent_wait while allFinished=false. ChatCMD rejects parent finalization while any child remains pending or running.";

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(SERVER_INSTRUCTIONS)
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

#[cfg(test)]
mod tests {
    use super::SERVER_INSTRUCTIONS;

    #[test]
    fn server_instructions_require_skill_discovery_before_project_work() {
        assert!(SERVER_INSTRUCTIONS.contains("call skills_list once"));
        assert!(SERVER_INSTRUCTIONS.contains("call skill_read for every relevant matching skill"));
        assert!(
            SERVER_INSTRUCTIONS.contains("A directly matching skill is mandatory, not optional")
        );
        assert!(SERVER_INSTRUCTIONS.contains("UI/color/layout/accessibility work"));
        assert!(SERVER_INSTRUCTIONS.contains("Rust implementation/review work"));
    }

    #[test]
    fn server_instructions_describe_task_scoped_explicit_path_grants() {
        assert!(SERVER_INSTRUCTIONS.contains("task-scoped access grant"));
        assert!(SERVER_INSTRUCTIONS.contains("Never widen that grant"));
        assert!(SERVER_INSTRUCTIONS.contains("including in later turns"));
        assert!(SERVER_INSTRUCTIONS.contains("path from another task/chat is not granted"));
    }

    #[test]
    fn server_instructions_prefer_native_text_editing() {
        assert!(SERVER_INSTRUCTIONS.contains("use fs_replace_text"));
        assert!(SERVER_INSTRUCTIONS.contains("use fs_write_text"));
        assert!(SERVER_INSTRUCTIONS.contains("Do not create or run Python"));
        assert!(SERVER_INSTRUCTIONS.contains("native filesystem tools"));
    }
}
