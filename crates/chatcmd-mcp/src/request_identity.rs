use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
};
use serde_json::{Map, Value};

const MAX_MCP_BODY_BYTES: usize = 4 * 1024 * 1024;
const OPENAI_SESSION_META_KEY: &str = "openai/session";
const MAX_CONVERSATION_IDENTITY_BYTES: usize = 4096;
const PRIVATE_SCOPE_ARGUMENT: &str = "__chatcmdConversationScopeId";

pub(crate) async fn bind_authenticated_agent(
    request: Request<Body>,
    agent_id: &str,
    session_id: &str,
) -> Result<Request<Body>, StatusCode> {
    if request.method() != Method::POST {
        return Ok(request);
    }

    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_MCP_BODY_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    if bytes.is_empty() {
        return Ok(Request::from_parts(parts, Body::empty()));
    }

    let mut payload: Value = serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    bind_identity_value(&mut payload, agent_id, session_id);
    let encoded = serde_json::to_vec(&payload).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Request::from_parts(parts, Body::from(encoded)))
}

fn bind_identity_value(value: &mut Value, agent_id: &str, session_id: &str) {
    match value {
        Value::Array(items) => {
            for item in items {
                bind_identity_value(item, agent_id, session_id);
            }
        }
        Value::Object(object)
            if object.get("method").and_then(Value::as_str) == Some("tools/call") =>
        {
            let conversation_scope = read_openai_conversation_scope(object);
            bind_tool_arguments(object, agent_id, session_id, conversation_scope.as_deref());
        }
        _ => {}
    }
}

fn bind_tool_arguments(
    object: &mut Map<String, Value>,
    agent_id: &str,
    session_id: &str,
    conversation_scope: Option<&str>,
) {
    let params = object
        .entry("params")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let arguments = params
        .entry("arguments")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(arguments) = arguments.as_object_mut() else {
        return;
    };
    arguments.remove("agent_id");
    arguments.insert("agentId".to_owned(), Value::String(agent_id.to_owned()));
    arguments.remove("__chatcmd_mcp_session_id");
    arguments.insert(
        "__chatcmdMcpSessionId".to_owned(),
        Value::String(session_id.to_owned()),
    );
    arguments.remove(PRIVATE_SCOPE_ARGUMENT);
    if let Some(scope) = conversation_scope {
        arguments.insert(
            PRIVATE_SCOPE_ARGUMENT.to_owned(),
            Value::String(scope.to_owned()),
        );
    }
}

fn read_openai_conversation_scope(object: &Map<String, Value>) -> Option<String> {
    let raw = object
        .get("params")?
        .as_object()?
        .get("_meta")?
        .as_object()?
        .get(OPENAI_SESSION_META_KEY)?
        .as_str()?
        .trim();
    if raw.is_empty() || raw.len() > MAX_CONVERSATION_IDENTITY_BYTES {
        return None;
    }
    let material = format!("openai\0{raw}");
    Some(format!(
        "openai:{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, material.as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authenticated_agent_overrides_client_agent_id() {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "device_list",
                "arguments": { "agentId": "rust_test", "agent_id": "legacy-wrong-shape" }
            }
        });

        bind_identity_value(&mut payload, "agent-from-token", "mcp-session-safe");

        assert_eq!(
            payload.pointer("/params/arguments/agentId"),
            Some(&Value::String("agent-from-token".to_owned()))
        );
        assert!(payload.pointer("/params/arguments/agent_id").is_none());
        assert_eq!(
            payload.pointer("/params/arguments/__chatcmdMcpSessionId"),
            Some(&Value::String("mcp-session-safe".to_owned()))
        );
    }

    #[test]
    fn openai_session_metadata_is_fingerprinted_and_overrides_spoofed_scope() {
        let mut first = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "shell_create",
                "_meta": { "openai/session": "chat-a" },
                "arguments": { "__chatcmdConversationScopeId": "spoofed" }
            }
        });
        let mut second = first.clone();
        second["params"]["_meta"]["openai/session"] = Value::String("chat-b".to_owned());

        bind_identity_value(&mut first, "agent", "mcp-session");
        bind_identity_value(&mut second, "agent", "mcp-session");

        let first_scope = first
            .pointer("/params/arguments/__chatcmdConversationScopeId")
            .and_then(Value::as_str)
            .expect("first scope");
        let second_scope = second
            .pointer("/params/arguments/__chatcmdConversationScopeId")
            .and_then(Value::as_str)
            .expect("second scope");
        assert!(first_scope.starts_with("openai:"));
        assert_ne!(first_scope, "spoofed");
        assert_ne!(first_scope, second_scope);
    }

    #[test]
    fn missing_openai_session_removes_spoofed_private_scope() {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "shell_create",
                "arguments": { "__chatcmdConversationScopeId": "spoofed" }
            }
        });
        bind_identity_value(&mut payload, "agent", "mcp-session");
        assert!(
            payload
                .pointer("/params/arguments/__chatcmdConversationScopeId")
                .is_none()
        );
    }

    #[test]
    fn non_tool_messages_are_unchanged() {
        let mut payload = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let original = payload.clone();
        bind_identity_value(&mut payload, "agent-from-token", "mcp-session-safe");
        assert_eq!(payload, original);
    }
}
