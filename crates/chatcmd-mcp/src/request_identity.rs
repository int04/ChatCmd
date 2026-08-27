use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
};
use serde_json::{Map, Value};

const MAX_MCP_BODY_BYTES: usize = 4 * 1024 * 1024;

pub(crate) async fn bind_authenticated_agent(
    request: Request<Body>,
    agent_id: &str,
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
    bind_agent_value(&mut payload, agent_id);
    let encoded = serde_json::to_vec(&payload).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Request::from_parts(parts, Body::from(encoded)))
}

fn bind_agent_value(value: &mut Value, agent_id: &str) {
    match value {
        Value::Array(items) => {
            for item in items {
                bind_agent_value(item, agent_id);
            }
        }
        Value::Object(object) => {
            if object.get("method").and_then(Value::as_str) == Some("tools/call") {
                bind_tool_arguments(object, agent_id);
            }
        }
        _ => {}
    }
}

fn bind_tool_arguments(object: &mut Map<String, Value>, agent_id: &str) {
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

        bind_agent_value(&mut payload, "agent-from-token");

        assert_eq!(
            payload.pointer("/params/arguments/agentId"),
            Some(&Value::String("agent-from-token".to_owned()))
        );
        assert!(payload.pointer("/params/arguments/agent_id").is_none());
    }

    #[test]
    fn non_tool_messages_are_unchanged() {
        let mut payload = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let original = payload.clone();
        bind_agent_value(&mut payload, "agent-from-token");
        assert_eq!(payload, original);
    }
}
