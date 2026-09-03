#![allow(deprecated)]

use std::{collections::HashSet, sync::Arc};

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
        let fallback = runtime
            .request_subagent_fallback(&parent_context, &registration, &delegated_prompt)
            .await?;
        return Ok(enrich_registration(
            registration,
            json!({
                "dispatchMode": "extensionFallback",
                "nativeDelegationRequired": false,
                "status": "pending",
                "workerStarted": false,
                "fallbackRequested": true,
                "fallbackAttempt": fallback.get("attempt").cloned().unwrap_or(Value::Null),
                "instruction": "ChatCMD queued this child for the ChatGPT browser extension. Do not duplicate the delegated work in the parent. Call agent_subagent_wait until the child finishes or the fallback exhausts its retries."
            }),
        ));
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
    let work = async {
        if peer.supports_sampling_tools() {
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
        }
    };
    tokio::pin!(work);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            result = &mut work => break result,
            _ = heartbeat.tick() => match runtime.heartbeat_subagent(&child_task_id).await {
                Ok(true) => {}
                Ok(false) => break Err(RuntimeError::new("subagent_lease_lost", "child worker no longer owns the active lease")),
                Err(error) => tracing::warn!(child_task_id, code = %error.code, "child heartbeat could not be persisted"),
            }
        }
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
