use chatcmd_runtime::RuntimeError;
use rmcp::{
    model::{ServerCapabilities, ServerInfo},
    tool_handler, ServerHandler,
};
use serde_json::Value;

use super::McpServer;

const SERVER_INSTRUCTIONS: &str = "IDENTITY: one ChatGPT chat equals one ChatCMD task; one user message equals one turn. Generate one unique turnId for each user message and reuse it unchanged for every ChatCMD call in that message. FIRST TOOL RULE: before calling any other ChatCMD tool in a user turn, call agent_user_message with the exact current user message text as content and that turnId. Do not summarize, rewrite, or omit the user's text. Reuse the newest taskId returned in this ChatGPT chat; omit taskId only when this chat has never returned one. ChatCMD validates the private ChatGPT conversation identity server-side; a stale taskId from another chat must not merge two chats. The server rejects other tools until the current turn's user message has been synchronized. Call agent_user_message exactly once per user turn. Never use agent_user_message for progress, reflections, findings, or commentary after tool results; use agent_progress for those updates. TOOL DISCOVERY RECOVERY RULE: ChatCMD exposes a broad, stable tool catalog and the host may lazy-load only a subset of tool schemas in a turn. A schema that is not currently visible is not evidence that the MCP server lost that tool. If a ChatCMD tool required to complete the user's request or any rule below is not currently visible or loaded, use the host's connector/resource discovery mechanism to discover and load that tool in the same turn, then continue the work. On ChatGPT connector hosts, use the connector discovery entrypoint available to the model (for example api_tool.list_resources) on the current connector with a focused query such as fs_, shell_, git_, skill, task, or agent. Before replying that a tool is unavailable, missing, not loaded, or cannot be used in the current turn, you MUST attempt discovery at least once for the needed capability in that same turn. Do not stop, defer implementation, or ask the user to send another message merely because a needed tool schema has not been loaded yet. SKILL RULE: after agent_user_message and before repository inspection, design decisions, code changes, or other non-trivial project work, call skills_list once to discover available .agents and .codex skills. Compare the returned skill descriptions with the current user request and intended work. If any skill matches, call skill_read for every relevant matching skill before doing the matching work, then follow those skill instructions. A directly matching skill is mandatory, not optional; do not infer its instructions from the skill name or description alone. For example, UI/color/layout/accessibility work must read a matching UI/UX skill when present, and Rust implementation/review work must read a matching Rust skill when present. Skip skill discovery only for trivial conversational turns or turns that do not require project work. INITIAL ACK RULE: for every non-trivial user request, immediately after agent_user_message and before skills_list or any other substantive tool call, call agent_progress once with a concise summary of what the user asked for and what you are going to do next. This first acknowledgement is mandatory even when the task seems obvious; do not postpone it until after repository inspection or tool results. PROGRESS CADENCE RULE: for every non-trivial project turn, agent_progress is mandatory throughout the entire turn, not only near the beginning. After the initial acknowledgement, aim for a progress checkpoint after roughly 2-4 substantive operations or at the end of one coherent batch of tightly related low-level calls; prefer meaningful milestones over mechanical per-tool updates so progress reporting does not materially slow execution. A substantive call includes repository/file inspection, search, edit/create/delete, shell/process work, Git work, build/test/lint, deployment, or another operation that advances the task. POST-ACTION REFLECTION RULE: after finishing a meaningful file read/code inspection or a coherent batch of tightly related reads/searches, call agent_progress with the concrete understanding or finding you just gained before moving into a new substantive phase. Immediately after successfully editing or creating a file, call agent_progress with what changed and the relevant effect before continuing. Immediately after a build, test, lint, search, Git operation, command, deployment, or other verification step returns a meaningful result, call agent_progress with that concrete result before starting the next substantive operation. SHELL PENDING RULE: when shell_wait or shell_read shows a long-running command is still pending and more polling is needed, send agent_progress with what command/process is running, the current known stage/output, and what result you are waiting for or will check next. Do not repeat an identical progress update for rapid consecutive polls; one update may cover a short polling loop until the state/output changes materially or a noticeable wait has elapsed. ERROR RECOVERY RULE: whenever any tool, command, build, test, lint, Git operation, deployment, or verification step returns an error, non-zero exit code, rejection, or other task-relevant failure, call agent_progress before retrying, changing approach, or invoking a fallback. The progress message must identify the failed operation, summarize the observable error, state whether a likely cause is known, and say what recovery or alternative approach you will try next; if no safe alternative is available, say so. Never silently retry after an error. STRONG PROGRESS HABIT: treat progress updates as an AI execution discipline rather than a server-side gate. Prefer calling agent_progress after fs_find/fs_search/fs_read_text and other meaningful filesystem results before moving to the next substantive read/search/edit, after pending shell polling, and before retrying a failed operation. Do not let progress messaging block or materially slow the actual task; when several tightly related low-level operations form one coherent step, group them and report the meaningful checkpoint rather than adding unnecessary round trips. These progress messages must summarize observable results and decisions, not private chain-of-thought. Do not emit progress for tiny mechanical no-ops or duplicate pagination chunks unless a dedicated rule above requires it. MIRROR RULE: whenever you are about to emit a user-visible commentary/progress/update message about current work, findings, next steps, phase changes, long-running operations, or completion status before the final answer, first call agent_progress with a concise message carrying the same substantive information. Do not emit multiple user-visible progress/commentary updates in a row without mirroring each distinct milestone through agent_progress. If a commentary update contains only conversational filler and no substantive project status, omit the commentary instead of sending an unmirrored status. This mirror requirement applies only to user-visible progress summaries, never to private chain-of-thought, hidden reasoning, or internal scratch work. Progress messages must be concise, concrete, user-visible summaries of the current work or confirmed findings; do not expose private chain-of-thought and do not send generic filler such as 'Working on it' or 'Please wait'. Never call agent_progress after agent_turn_complete. TOOL ARGUMENT RULE: treat each tool's generated JSON schema as the canonical contract. Use the canonical field names shown by the schema and never invent a field name from an output object or from another tool. Compatibility aliases may be accepted by the server, but do not prefer them over the schema. PATH RULE: an existing absolute filesystem path explicitly present in any user message of the current ChatCMD task is a task-scoped access grant for that exact file or directory subtree, even when it is outside configured workspace roots. Use it directly when relevant, including in later turns such as when the user says to continue. Never widen that grant to a parent, sibling, different drive, or another path the user did not write; a path from another task/chat is not granted. PROJECT CONTEXT RULE: project/workspace context belongs to the current task/conversation, never to the Agent. Before filesystem, Git, repository, codebase, or project shell work, if the current task does not already have a project folder and the user has not supplied an explicit absolute work path, do not guess or infer a folder from the Agent, workspace_roots, current process directory, another task, or a previously used project. Ask the user to provide the project folder or absolute work path first, and do not call filesystem, Git, or project shell tools until that context is available. PATH DISCOVERY RULE: never guess a relative project path. If the exact relative path was not supplied by the user or returned by a prior ChatCMD filesystem/path result in this task, call fs_find from path '.' first and use the returned path. Use '.' rather than an empty string for the workspace root. EDIT RULE: for targeted text changes, use fs_replace_text; use fs_write_text for whole-file creation or replacement. For fs_replace_text, copy oldText exactly from the latest current file content; if the file may have changed since it was read, read the target range again before replacing. Do not reconstruct oldText from memory. Do not create or run Python, PowerShell, Node, or shell scripts merely to edit text when native filesystem tools can perform the change; use shell only when the native tools cannot express the required edit. NEW CHAT RULE: only when agent_user_message returns isFirstMessage=true, the exact first user message participates in the Rust task ID seed and agent_turn_complete must include a concise suggestedTitle for that conversation; never rename it from later turns. When any ChatCMD tool is used in a user turn, agent_turn_complete MUST be called exactly once immediately before replying to the user. Use the same taskId and turnId as that turn's tools, pass the exact final user-facing response text as content, finish all other tool calls first, and do not call another tool afterward. SUB-AGENT RULE: the parent ChatGPT may delegate when the user explicitly asks to split work across agents or when the parent independently judges delegation useful for parallel or specialized work. EXPLICIT MULTI-AGENT INTENT RULE: if agent_user_message.content clearly asks to split work across agents, for example phrases equivalent to 'chia agent', 'chia ra N agent', 'dùng nhiều agent', 'split into agents', or 'use multiple agents', the parent MUST attempt host-native delegation/subagent execution before doing the delegated work itself. Prefer the ChatGPT host's native delegation capability when available, and register/synchronize each delegated child with ChatCMD via agent_subagent_start so the parent/child task relationship remains visible to ChatCMD. Do not substitute a local Codex fallback for this explicit multi-agent request. When delegating, call agent_subagent_start once for each delegated child with a concise AI-chosen name and request. The result keeps taskId as the parent coordinator task and exposes childTaskId as the child conversation/task; never replace the parent taskId with childTaskId in later parent calls. Registration is idempotent within one parent turn by name plus delegated request, so a retry returns the same subagentId/childTaskId with duplicate=true instead of creating another child. Inspect dispatchMode: samplingTools or samplingText means ChatCMD is running the child through MCP sampling; samplingUnavailable means the connected ChatGPT/MCP host does not advertise sampling, no local executor was started, and the parent MUST continue handling the original user request itself whenever that request is still feasible without a child agent. When samplingUnavailable follows an explicit multi-agent request, the parent MUST explicitly report that native child-agent delegation was unavailable for this host/session, then continue the feasible work itself instead of stopping. The parent MUST NOT end the turn merely to report that sub-agent creation is unavailable, and MUST NOT treat samplingUnavailable as completion of the user's request. If the host provides native delegation, use it instead. existing means the same child was already registered/claimed and must not be spawned again. If startup fails after registration, agent_subagent_start returns a normal structured result with status=failed and startupError rather than a tool-level error; do not blindly retry it. Do not create a duplicate host-native child. Before agent_turn_complete in the parent turn, call agent_subagent_wait while allFinished=false. ChatCMD rejects parent finalization while any child remains pending or running.";

const TASK_WORKSPACE_INSTRUCTIONS: &str = "TASK WORKSPACE RESULT RULE: treat projectFolder returned by agent_user_message as the authoritative workspace for the current task. workspace_roots is task-scoped: when the task has a project folder it returns that folder, never the Agent folder or process-wide server root. Do not reject an explicit task project folder because it differs from a previous workspace_roots result from another task or connection.";

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            format!("{SERVER_INSTRUCTIONS} {TASK_WORKSPACE_INSTRUCTIONS}"),
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

#[cfg(test)]
mod tests {
    use super::{SERVER_INSTRUCTIONS, TASK_WORKSPACE_INSTRUCTIONS};

    #[test]
    fn server_instructions_require_lazy_tool_discovery_in_same_turn() {
        assert!(SERVER_INSTRUCTIONS.contains("TOOL DISCOVERY RECOVERY RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("broad, stable tool catalog"));
        assert!(SERVER_INSTRUCTIONS.contains("not evidence that the MCP server lost that tool"));
        assert!(SERVER_INSTRUCTIONS.contains("lazy-load only a subset of tool schemas"));
        assert!(SERVER_INSTRUCTIONS.contains("connector/resource discovery mechanism"));
        assert!(SERVER_INSTRUCTIONS.contains("api_tool.list_resources"));
        assert!(SERVER_INSTRUCTIONS.contains("fs_, shell_, git_, skill, task, or agent"));
        assert!(SERVER_INSTRUCTIONS.contains("MUST attempt discovery at least once"));
        assert!(SERVER_INSTRUCTIONS.contains("in that same turn"));
        assert!(SERVER_INSTRUCTIONS.contains("Do not stop, defer implementation"));
        assert!(SERVER_INSTRUCTIONS.contains("ask the user to send another message"));
    }

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
    fn server_instructions_keep_user_message_and_progress_roles_separate() {
        assert!(SERVER_INSTRUCTIONS.contains("Call agent_user_message exactly once per user turn"));
        assert!(SERVER_INSTRUCTIONS.contains(
            "Never use agent_user_message for progress, reflections, findings, or commentary after tool results"
        ));
        assert!(SERVER_INSTRUCTIONS.contains("use agent_progress for those updates"));
    }

    #[test]
    fn server_instructions_require_strong_progress_updates() {
        assert!(SERVER_INSTRUCTIONS.contains("INITIAL ACK RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("immediately after agent_user_message"));
        assert!(
            SERVER_INSTRUCTIONS.contains("before skills_list or any other substantive tool call")
        );
        assert!(SERVER_INSTRUCTIONS
            .contains("what the user asked for and what you are going to do next"));
        assert!(SERVER_INSTRUCTIONS.contains("PROGRESS CADENCE RULE"));
        assert!(
            SERVER_INSTRUCTIONS.contains("throughout the entire turn, not only near the beginning")
        );
        assert!(SERVER_INSTRUCTIONS.contains("roughly 2-4 substantive operations"));
        assert!(
            SERVER_INSTRUCTIONS.contains("one coherent batch of tightly related low-level calls")
        );
        assert!(SERVER_INSTRUCTIONS.contains("POST-ACTION REFLECTION RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("meaningful file read/code inspection"));
        assert!(SERVER_INSTRUCTIONS.contains("coherent batch of tightly related reads/searches"));
        assert!(SERVER_INSTRUCTIONS.contains("successfully editing or creating a file"));
        assert!(SERVER_INSTRUCTIONS.contains("SHELL PENDING RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("long-running command is still pending"));
        assert!(SERVER_INSTRUCTIONS
            .contains("Do not repeat an identical progress update for rapid consecutive polls"));
        assert!(SERVER_INSTRUCTIONS.contains("ERROR RECOVERY RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("non-zero exit code"));
        assert!(SERVER_INSTRUCTIONS.contains("Never silently retry after an error"));
        assert!(SERVER_INSTRUCTIONS.contains("recovery or alternative approach"));
        assert!(SERVER_INSTRUCTIONS.contains("STRONG PROGRESS HABIT"));
        assert!(
            SERVER_INSTRUCTIONS.contains("AI execution discipline rather than a server-side gate")
        );
        assert!(SERVER_INSTRUCTIONS.contains("fs_find/fs_search/fs_read_text"));
        assert!(SERVER_INSTRUCTIONS
            .contains("Do not let progress messaging block or materially slow the actual task"));
        assert!(SERVER_INSTRUCTIONS.contains("not private chain-of-thought"));
        assert!(SERVER_INSTRUCTIONS.contains("MIRROR RULE"));
        assert!(SERVER_INSTRUCTIONS
            .contains("about to emit a user-visible commentary/progress/update message"));
        assert!(SERVER_INSTRUCTIONS.contains("first call agent_progress"));
        assert!(SERVER_INSTRUCTIONS
            .contains("Do not emit multiple user-visible progress/commentary updates in a row"));
        assert!(SERVER_INSTRUCTIONS.contains("never to private chain-of-thought"));
        assert!(SERVER_INSTRUCTIONS.contains("Never call agent_progress after agent_turn_complete"));
    }

    #[test]
    fn server_instructions_describe_task_scoped_explicit_path_grants() {
        assert!(SERVER_INSTRUCTIONS.contains("task-scoped access grant"));
        assert!(SERVER_INSTRUCTIONS.contains("Never widen that grant"));
        assert!(SERVER_INSTRUCTIONS.contains("including in later turns"));
        assert!(SERVER_INSTRUCTIONS.contains("path from another task/chat is not granted"));
    }

    #[test]
    fn server_instructions_require_project_path_when_context_is_missing() {
        assert!(SERVER_INSTRUCTIONS.contains("PROJECT CONTEXT RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("never to the Agent"));
        assert!(SERVER_INSTRUCTIONS.contains("do not guess or infer a folder"));
        assert!(SERVER_INSTRUCTIONS.contains("workspace_roots"));
        assert!(SERVER_INSTRUCTIONS
            .contains("Ask the user to provide the project folder or absolute work path first"));
    }

    #[test]
    fn server_instructions_make_task_workspace_results_authoritative() {
        assert!(TASK_WORKSPACE_INSTRUCTIONS.contains("projectFolder"));
        assert!(TASK_WORKSPACE_INSTRUCTIONS.contains("workspace_roots is task-scoped"));
        assert!(TASK_WORKSPACE_INSTRUCTIONS.contains("never the Agent folder"));
        assert!(TASK_WORKSPACE_INSTRUCTIONS.contains("process-wide server root"));
    }

    #[test]
    fn server_instructions_prevent_argument_and_path_guessing() {
        assert!(SERVER_INSTRUCTIONS.contains("generated JSON schema as the canonical contract"));
        assert!(SERVER_INSTRUCTIONS.contains("never guess a relative project path"));
        assert!(SERVER_INSTRUCTIONS.contains("call fs_find from path '.' first"));
        assert!(SERVER_INSTRUCTIONS
            .contains("copy oldText exactly from the latest current file content"));
    }

    #[test]
    fn server_instructions_prefer_native_text_editing() {
        assert!(SERVER_INSTRUCTIONS.contains("use fs_replace_text"));
        assert!(SERVER_INSTRUCTIONS.contains("use fs_write_text"));
        assert!(SERVER_INSTRUCTIONS.contains("Do not create or run Python"));
        assert!(SERVER_INSTRUCTIONS.contains("native filesystem tools"));
    }

    #[test]
    fn server_instructions_require_explicit_multi_agent_intent_to_try_native_delegation() {
        assert!(SERVER_INSTRUCTIONS.contains("EXPLICIT MULTI-AGENT INTENT RULE"));
        assert!(SERVER_INSTRUCTIONS.contains("'chia agent'"));
        assert!(SERVER_INSTRUCTIONS.contains("'chia ra N agent'"));
        assert!(SERVER_INSTRUCTIONS.contains("'dùng nhiều agent'"));
        assert!(
            SERVER_INSTRUCTIONS.contains("MUST attempt host-native delegation/subagent execution")
        );
        assert!(SERVER_INSTRUCTIONS.contains(
            "register/synchronize each delegated child with ChatCMD via agent_subagent_start"
        ));
        assert!(SERVER_INSTRUCTIONS.contains("Do not substitute a local Codex fallback"));
    }

    #[test]
    fn server_instructions_require_parent_to_continue_when_sampling_is_unavailable() {
        assert!(
            SERVER_INSTRUCTIONS.contains("parent MUST continue handling the original user request")
        );
        assert!(SERVER_INSTRUCTIONS
            .contains("MUST explicitly report that native child-agent delegation was unavailable"));
        assert!(SERVER_INSTRUCTIONS.contains(
            "MUST NOT end the turn merely to report that sub-agent creation is unavailable"
        ));
        assert!(SERVER_INSTRUCTIONS.contains("MUST NOT treat samplingUnavailable as completion"));
    }
}
