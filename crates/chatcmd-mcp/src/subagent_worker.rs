#![allow(deprecated)]

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use rmcp::{
    Peer, RoleServer,
    model::{
        ContentBlock, CreateMessageRequestParams, SamplingMessage, SamplingMessageContentBlock,
        Tool, ToolChoice,
    },
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    RuntimeApi,
    subagent_protocol::{TextAction, parse_text_action},
};

const MAX_ROUNDS: usize = 24;
const MAX_TOOL_RESULT_CHARS: usize = 60_000;
const MAX_FINAL_CHARS: usize = 100_000;
const LOCAL_EXECUTOR_WAIT_MS: u64 = 30_000;
const LOCAL_EXECUTOR_MAX_READ_EVENTS: usize = 2_000;

pub(super) async fn dispatch_registered_subagent(
    runtime: Arc<dyn RuntimeApi>,
    peer: Peer<RoleServer>,
    parent_context: OperationContext,
    registration: Value,
    request: &str,
    tools: Vec<Tool>,
) -> RuntimeResult<Value> {
    let subagent_id = required_string(&registration, "subagentId")?.to_owned();
    let child_task_id = required_child_task_id(&registration)?.to_owned();
    let name = required_string(&registration, "name")?.to_owned();
    let marker = required_string(&registration, "delegationMarker")?.to_owned();
    let duplicate_registration = registration
        .get("duplicate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let registered_status = registration
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending")
        .to_owned();
    if duplicate_registration && registered_status != "pending" {
        return Ok(enrich_registration(
            registration,
            json!({
                "dispatchMode": "existing",
                "workerStarted": false,
                "status": registered_status,
                "instruction": "This delegated child already exists for the same parent turn, name, and request. Do not create another child."
            }),
        ));
    }
    let delegated_prompt = format!(
        "{}

{}",
        request.trim(),
        marker
    );
    let sampling = peer
        .peer_info()
        .is_some_and(|info| info.capabilities.sampling.is_some());

    if !sampling {
        return start_local_codex_subagent(
            runtime,
            parent_context,
            registration,
            subagent_id,
            child_task_id,
            name,
            delegated_prompt,
            request,
        )
        .await;
    }

    let turn_id = format!("turn-{subagent_id}");
    let child_user_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "agent_user_message",
        format!("subagent-user-{subagent_id}"),
    );
    let child_sync = match runtime
        .call(
            "agent_user_message",
            child_user_context,
            json!({ "content": delegated_prompt }),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            let _ = runtime
                .fail_subagent(
                    &child_task_id,
                    &format!("child startup failed: {}", error.message),
                )
                .await;
            return Err(error);
        }
    };
    if duplicate_registration && child_sync.get("duplicate").and_then(Value::as_bool) == Some(true)
    {
        return Ok(enrich_registration(
            registration,
            json!({
                "dispatchMode": "existing",
                "workerStarted": false,
                "status": "running",
                "instruction": "An existing worker already claimed this child. Do not start a duplicate worker."
            }),
        ));
    }

    let tools = tools
        .into_iter()
        .filter(|tool| !is_internal_tool(tool.name.as_ref()))
        .collect::<Vec<_>>();
    if tools.is_empty() {
        let error = RuntimeError::new(
            "subagent_no_tools",
            "no executable tools are available to the child agent",
        );
        let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
        return Err(error);
    }

    let dispatch_mode = if peer.supports_sampling_tools() {
        "samplingTools"
    } else {
        "samplingText"
    };
    let response = enrich_registration(
        registration,
        json!({
            "dispatchMode": dispatch_mode,
            "nativeDelegationRequired": false,
            "status": "running",
            "workerStarted": true
        }),
    );

    let request = request.trim().to_owned();
    tokio::spawn(async move {
        run_claimed_sampling_subagent(
            runtime,
            peer,
            parent_context,
            subagent_id,
            child_task_id,
            turn_id,
            name,
            request,
            tools,
        )
        .await;
    });

    Ok(response)
}

async fn start_local_codex_subagent(
    runtime: Arc<dyn RuntimeApi>,
    parent_context: OperationContext,
    registration: Value,
    subagent_id: String,
    child_task_id: String,
    name: String,
    delegated_prompt: String,
    request: &str,
) -> RuntimeResult<Value> {
    let turn_id = format!("turn-{subagent_id}");
    let child_user_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "agent_user_message",
        format!("subagent-user-{subagent_id}"),
    );
    let child_sync = match runtime
        .call(
            "agent_user_message",
            child_user_context,
            json!({ "content": delegated_prompt }),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            let _ = runtime
                .fail_subagent(
                    &child_task_id,
                    &format!("local child startup failed: {}", error.message),
                )
                .await;
            return Err(error);
        }
    };
    if registration.get("duplicate").and_then(Value::as_bool) == Some(true)
        && child_sync.get("duplicate").and_then(Value::as_bool) == Some(true)
    {
        return Ok(enrich_registration(
            registration,
            json!({
                "dispatchMode": "existing",
                "workerStarted": false,
                "status": "running",
                "instruction": "An existing worker already claimed this child. Do not start a duplicate worker."
            }),
        ));
    }

    let workspace = match runtime.project_folder(&parent_context.agent_id).await {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        Ok(_) => {
            let roots_context = child_context(
                &parent_context,
                &child_task_id,
                &turn_id,
                "workspace_roots",
                format!("subagent-roots-{subagent_id}"),
            );
            let roots = match runtime
                .call("workspace_roots", roots_context, json!({}))
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
                    return Err(error);
                }
            };
            match resolve_workspace_root(&roots) {
                Some(value) => value,
                None => {
                    let error = RuntimeError::new(
                        "subagent_workspace_missing",
                        "local sub-agent executor could not resolve the agent project folder or a workspace root",
                    );
                    let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
                    return Err(error);
                }
            }
        }
        Err(error) => {
            let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
            return Err(error);
        }
    };

    let output_path = local_executor_output_path(&subagent_id);
    let executor_prompt = local_codex_prompt(&name, request);
    let shell_create_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "shell_create",
        format!("subagent-shell-create-{subagent_id}"),
    );
    let shell = match runtime
        .call(
            "shell_create",
            shell_create_context,
            json!({
                "workingDirectory": workspace,
                "environment": {
                    "CHATCMD_SUBAGENT_PROMPT": executor_prompt,
                    "CHATCMD_SUBAGENT_OUTPUT": output_path.to_string_lossy()
                }
            }),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
            return Err(error);
        }
    };
    let session_id = match required_string(&shell, "sessionId") {
        Ok(value) => value.to_owned(),
        Err(error) => {
            let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
            return Err(error);
        }
    };
    let platform = runtime.local_device().platform;
    let command = local_codex_command(&platform);
    let shell_write_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "shell_write",
        format!("subagent-shell-write-{subagent_id}"),
    );
    if let Err(error) = runtime
        .call(
            "shell_write",
            shell_write_context,
            json!({
                "sessionId": session_id,
                "text": command,
                "appendNewLine": true
            }),
        )
        .await
    {
        let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
        return Err(error);
    }

    let response = enrich_registration(
        registration,
        json!({
            "dispatchMode": "localCodex",
            "nativeDelegationRequired": false,
            "status": "running",
            "workerStarted": true,
            "executor": "codex",
            "sandbox": "read-only",
            "instruction": "The MCP client does not advertise sampling, so ChatCMD started a local Codex CLI worker in the reserved child task. Wait for completion with agent_subagent_wait."
        }),
    );

    tokio::spawn(async move {
        run_local_codex_worker(
            runtime,
            parent_context,
            subagent_id,
            child_task_id,
            turn_id,
            session_id,
            output_path,
        )
        .await;
    });
    Ok(response)
}

async fn run_local_codex_worker(
    runtime: Arc<dyn RuntimeApi>,
    parent_context: OperationContext,
    subagent_id: String,
    child_task_id: String,
    turn_id: String,
    session_id: String,
    output_path: PathBuf,
) {
    let mut wait_round = 0usize;
    let exit_code = loop {
        let context = child_context(
            &parent_context,
            &child_task_id,
            &turn_id,
            "shell_wait",
            format!("subagent-shell-wait-{subagent_id}-{wait_round}"),
        );
        let wait = match runtime
            .call(
                "shell_wait",
                context,
                json!({ "sessionId": session_id, "timeoutMs": LOCAL_EXECUTOR_WAIT_MS }),
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
                cleanup_local_output(&output_path);
                return;
            }
        };
        if wait.get("completed").and_then(Value::as_bool) == Some(true) {
            break wait.get("exitCode").and_then(Value::as_i64).unwrap_or(-1);
        }
        wait_round = wait_round.saturating_add(1);
    };

    let read_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "shell_read",
        format!("subagent-shell-read-{subagent_id}"),
    );
    let shell_output = runtime
        .call(
            "shell_read",
            read_context,
            json!({
                "sessionId": session_id,
                "afterSequence": 0,
                "maxEvents": LOCAL_EXECUTOR_MAX_READ_EVENTS
            }),
        )
        .await
        .map(|value| shell_output_text(&value))
        .unwrap_or_default();

    if exit_code != 0 {
        let message = local_executor_failure(exit_code, &shell_output);
        let _ = runtime.fail_subagent(&child_task_id, &message).await;
        cleanup_local_output(&output_path);
        return;
    }

    let final_text = std::fs::read_to_string(&output_path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| fallback_codex_final(&shell_output));
    cleanup_local_output(&output_path);
    let Some(final_text) = final_text else {
        let _ = runtime
            .fail_subagent(
                &child_task_id,
                "local Codex worker exited successfully but produced no final response",
            )
            .await;
        return;
    };

    let completion_context = child_context(
        &parent_context,
        &child_task_id,
        &turn_id,
        "agent_turn_complete",
        format!("subagent-complete-{subagent_id}"),
    );
    if let Err(error) = runtime
        .call(
            "agent_turn_complete",
            completion_context,
            json!({ "content": limit_text(&final_text, MAX_FINAL_CHARS) }),
        )
        .await
    {
        let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
    }
}

fn local_codex_prompt(name: &str, request: &str) -> String {
    format!(
        "You are ChatCMD child agent {name}. Complete only the delegated request below. This fallback worker is intentionally read-only: inspect files, git state/diffs, logs, and other existing data as needed, but do not modify files, commit, install packages, or change system state. Return a concise final result for the parent agent.\n\nDELEGATED REQUEST:\n{}",
        request.trim()
    )
}

fn local_codex_command(platform: &str) -> &'static str {
    if platform.eq_ignore_ascii_case("windows") {
        "codex exec --ephemeral --sandbox read-only --color never -o \"$env:CHATCMD_SUBAGENT_OUTPUT\" \"$env:CHATCMD_SUBAGENT_PROMPT\"; exit $LASTEXITCODE"
    } else {
        "codex exec --ephemeral --sandbox read-only --color never -o \"$CHATCMD_SUBAGENT_OUTPUT\" \"$CHATCMD_SUBAGENT_PROMPT\"; exit $?"
    }
}

fn resolve_workspace_root(value: &Value) -> Option<String> {
    fn root_path(value: &Value) -> Option<String> {
        match value {
            Value::String(path) => (!path.trim().is_empty()).then(|| path.to_owned()),
            Value::Object(object) => object
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
                .map(str::to_owned),
            _ => None,
        }
    }

    match value {
        Value::Array(items) => items.iter().find_map(root_path),
        Value::Object(object) => object
            .get("roots")
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find_map(root_path)),
        _ => None,
    }
}

fn local_executor_output_path(subagent_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("chatcmd-{subagent_id}-final.txt"))
}

fn cleanup_local_output(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

fn shell_output_text(value: &Value) -> String {
    value
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|event| event.get("data").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

fn local_executor_failure(exit_code: i64, output: &str) -> String {
    let tail = output.chars().rev().take(4_000).collect::<String>();
    let tail = tail.chars().rev().collect::<String>();
    if tail.trim().is_empty() {
        format!("local Codex sub-agent executor failed with exit code {exit_code}")
    } else {
        format!(
            "local Codex sub-agent executor failed with exit code {exit_code}: {}",
            tail.trim()
        )
    }
}

fn fallback_codex_final(output: &str) -> Option<String> {
    let marker = "\ncodex\n";
    let start = output.rfind(marker)? + marker.len();
    let tail = &output[start..];
    let end = tail.find("\ntokens used\n").unwrap_or(tail.len());
    let final_text = tail[..end].trim();
    (!final_text.is_empty()).then(|| final_text.to_owned())
}

async fn run_claimed_sampling_subagent(
    runtime: Arc<dyn RuntimeApi>,
    peer: Peer<RoleServer>,
    parent_context: OperationContext,
    subagent_id: String,
    child_task_id: String,
    turn_id: String,
    name: String,
    request: String,
    tools: Vec<Tool>,
) {
    let result = if peer.supports_sampling_tools() {
        run_tool_sampling(
            runtime.clone(),
            &peer,
            &parent_context,
            &subagent_id,
            &child_task_id,
            &turn_id,
            &name,
            &request,
            &tools,
        )
        .await
    } else {
        run_text_sampling(
            runtime.clone(),
            &peer,
            &parent_context,
            &subagent_id,
            &child_task_id,
            &turn_id,
            &name,
            &request,
            &tools,
        )
        .await
    };

    match result {
        Ok(final_text) if !final_text.trim().is_empty() => {
            let completion_context = child_context(
                &parent_context,
                &child_task_id,
                &turn_id,
                "agent_turn_complete",
                format!("subagent-complete-{subagent_id}"),
            );
            if let Err(error) = runtime
                .call(
                    "agent_turn_complete",
                    completion_context,
                    json!({ "content": final_text }),
                )
                .await
            {
                let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
            }
        }
        Ok(_) => {
            let _ = runtime
                .fail_subagent(
                    &child_task_id,
                    "child agent produced an empty final response",
                )
                .await;
        }
        Err(error) => {
            let _ = runtime.fail_subagent(&child_task_id, &error.message).await;
        }
    }
}

async fn run_tool_sampling(
    runtime: Arc<dyn RuntimeApi>,
    peer: &Peer<RoleServer>,
    parent_context: &OperationContext,
    subagent_id: &str,
    child_task_id: &str,
    turn_id: &str,
    name: &str,
    request: &str,
    tools: &[Tool],
) -> RuntimeResult<String> {
    let allowed = tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<HashSet<_>>();
    let mut messages = vec![SamplingMessage::user_text(request.trim())];
    let system_prompt = child_system_prompt(name, false, tools);

    for round in 0..MAX_ROUNDS {
        let params = CreateMessageRequestParams::new(messages.clone(), 4096)
            .with_system_prompt(system_prompt.clone())
            .with_tools(tools.to_vec())
            .with_tool_choice(ToolChoice::auto());
        let sampled = peer.create_message(params).await.map_err(|error| {
            RuntimeError::new(
                "subagent_sampling_failed",
                format!("child model request failed: {error}"),
            )
        })?;
        let message = sampled.message.clone();
        let text = message_text(&message);
        let tool_uses = message
            .content
            .iter()
            .filter_map(|content| content.as_tool_use().cloned())
            .collect::<Vec<_>>();
        messages.push(message);

        if tool_uses.is_empty() {
            if text.trim().is_empty() {
                return Err(RuntimeError::new(
                    "subagent_empty_response",
                    "child model ended without a final response",
                ));
            }
            return Ok(limit_text(&text, MAX_FINAL_CHARS));
        }

        if !text.trim().is_empty() {
            save_progress(
                runtime.clone(),
                parent_context,
                child_task_id,
                turn_id,
                subagent_id,
                round,
                &text,
            )
            .await?;
        }

        for (index, tool_use) in tool_uses.into_iter().enumerate() {
            let response = if !allowed.contains(&tool_use.name) {
                Err(RuntimeError::new(
                    "subagent_tool_denied",
                    format!("child requested unavailable tool {}", tool_use.name),
                ))
            } else {
                call_child_tool(
                    runtime.clone(),
                    parent_context,
                    child_task_id,
                    turn_id,
                    subagent_id,
                    round,
                    index,
                    &tool_use.name,
                    tool_use.input,
                )
                .await
            };
            let content = tool_result_text(response);
            messages.push(SamplingMessage::user_tool_result(
                tool_use.id,
                vec![ContentBlock::text(content)],
            ));
        }
    }

    Err(RuntimeError::new(
        "subagent_round_limit",
        format!("child agent exceeded {MAX_ROUNDS} model rounds"),
    ))
}

async fn run_text_sampling(
    runtime: Arc<dyn RuntimeApi>,
    peer: &Peer<RoleServer>,
    parent_context: &OperationContext,
    subagent_id: &str,
    child_task_id: &str,
    turn_id: &str,
    name: &str,
    request: &str,
    tools: &[Tool],
) -> RuntimeResult<String> {
    let allowed = tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<HashSet<_>>();
    let mut messages = vec![SamplingMessage::user_text(request.trim())];
    let system_prompt = child_system_prompt(name, true, tools);

    for round in 0..MAX_ROUNDS {
        let sampled = peer
            .create_message(
                CreateMessageRequestParams::new(messages.clone(), 4096)
                    .with_system_prompt(system_prompt.clone()),
            )
            .await
            .map_err(|error| {
                RuntimeError::new(
                    "subagent_sampling_failed",
                    format!("child model request failed: {error}"),
                )
            })?;
        let text = message_text(&sampled.message);
        messages.push(sampled.message);
        let action = parse_text_action(&text)?;
        match action {
            TextAction::Final(final_text) => return Ok(limit_text(&final_text, MAX_FINAL_CHARS)),
            TextAction::Tool { name, arguments } => {
                if !allowed.contains(&name) {
                    messages.push(SamplingMessage::user_text(format!(
                        "TOOL_RESULT error: tool {name} is unavailable. Choose one of the advertised tools."
                    )));
                    continue;
                }
                let output = call_child_tool(
                    runtime.clone(),
                    parent_context,
                    child_task_id,
                    turn_id,
                    subagent_id,
                    round,
                    0,
                    &name,
                    arguments,
                )
                .await;
                messages.push(SamplingMessage::user_text(format!(
                    "TOOL_RESULT {}",
                    tool_result_text(output)
                )));
            }
        }
    }

    Err(RuntimeError::new(
        "subagent_round_limit",
        format!("child agent exceeded {MAX_ROUNDS} model rounds"),
    ))
}

async fn call_child_tool(
    runtime: Arc<dyn RuntimeApi>,
    parent_context: &OperationContext,
    child_task_id: &str,
    turn_id: &str,
    subagent_id: &str,
    round: usize,
    index: usize,
    tool: &str,
    mut arguments: Map<String, Value>,
) -> RuntimeResult<Value> {
    sanitize_arguments(&mut arguments);
    let context = child_context(
        parent_context,
        child_task_id,
        turn_id,
        tool,
        format!(
            "subagent-tool-{subagent_id}-{round}-{index}-{}",
            Uuid::new_v4()
        ),
    );
    runtime.call(tool, context, Value::Object(arguments)).await
}

async fn save_progress(
    runtime: Arc<dyn RuntimeApi>,
    parent_context: &OperationContext,
    child_task_id: &str,
    turn_id: &str,
    subagent_id: &str,
    round: usize,
    text: &str,
) -> RuntimeResult<()> {
    let context = child_context(
        parent_context,
        child_task_id,
        turn_id,
        "agent_progress",
        format!("subagent-progress-{subagent_id}-{round}"),
    );
    runtime
        .call(
            "agent_progress",
            context,
            json!({ "message": limit_text(text, 2_000) }),
        )
        .await
        .map(|_| ())
}

fn child_context(
    parent: &OperationContext,
    task_id: &str,
    turn_id: &str,
    tool: &str,
    request_id: String,
) -> OperationContext {
    let mut context = OperationContext::new(request_id, parent.agent_id.clone(), tool);
    context.task_id = Some(task_id.to_owned());
    context.turn_id = Some(turn_id.to_owned());
    context
}

fn child_system_prompt(name: &str, text_protocol: bool, tools: &[Tool]) -> String {
    let tool_summary = tools
        .iter()
        .map(|tool| {
            format!(
                "- {}: {}",
                tool.name,
                tool.description.as_deref().unwrap_or("no description")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text_protocol {
        format!(
            "You are child agent {name}. Complete only the delegated request. Use the available Rust/ChatCMD tools when needed. Do not delegate further. Every response MUST be exactly one JSON object and no markdown. To call a tool: {{\"action\":\"tool\",\"name\":\"tool_name\",\"arguments\":{{...}}}}. When finished: {{\"action\":\"final\",\"content\":\"concise final answer\"}}. Available tools:\n{tool_summary}"
        )
    } else {
        format!(
            "You are child agent {name}. Complete only the delegated request. Use the supplied tools whenever inspection or execution is required. Do not delegate further. When the work is complete, return a concise final answer. Available tools:\n{tool_summary}"
        )
    }
}

fn message_text(message: &SamplingMessage) -> String {
    message
        .content
        .iter()
        .filter_map(SamplingMessageContentBlock::as_text)
        .map(|content| content.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_result_text(result: RuntimeResult<Value>) -> String {
    match result {
        Ok(value) => limit_text(&value.to_string(), MAX_TOOL_RESULT_CHARS),
        Err(error) => limit_text(
            &json!({ "error": { "code": error.code, "message": error.message } }).to_string(),
            MAX_TOOL_RESULT_CHARS,
        ),
    }
}

fn sanitize_arguments(arguments: &mut Map<String, Value>) {
    for key in [
        "requestId",
        "request_id",
        "agentId",
        "agent_id",
        "taskId",
        "task_id",
        "turnId",
        "turn_id",
        "__chatcmdMcpSessionId",
        "__chatcmdConversationScopeId",
    ] {
        arguments.remove(key);
    }
}

fn is_internal_tool(name: &str) -> bool {
    name.starts_with("agent_")
}

fn required_child_task_id(value: &Value) -> RuntimeResult<&str> {
    value
        .get("childTaskId")
        .or_else(|| value.get("taskId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::new("subagent_registration_invalid", "missing childTaskId"))
}

fn required_string<'a>(value: &'a Value, key: &str) -> RuntimeResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::new("subagent_registration_invalid", format!("missing {key}")))
}

fn enrich_registration(mut registration: Value, fields: Value) -> Value {
    let Some(target) = registration.as_object_mut() else {
        return registration;
    };
    if let Some(fields) = fields.as_object() {
        target.extend(fields.clone());
    }
    registration
}

fn limit_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
#[path = "subagent_worker_tests.rs"]
mod tests;
