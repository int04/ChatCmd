use super::*;
use serde_json::Value;

#[test]
fn computer_tools_are_advertised_with_explicit_risk_classes() {
    let expected = [
        (
            "computer_session_start",
            ToolOperationClass::ProcessExecution,
        ),
        ("computer_observe", ToolOperationClass::ContentRead),
        ("computer_act", ToolOperationClass::Mutation),
        ("computer_session_close", ToolOperationClass::StopCleanup),
    ];
    for (name, operation_class) in expected {
        assert!(TOOL_NAMES.iter().any(|candidate| candidate == name));
        assert_eq!(tool_capabilities(name).operation_class, operation_class);
    }
    assert!(tool_capabilities("computer_session_start").approval_required);
    assert!(tool_capabilities("computer_observe").approval_required);
    assert!(tool_capabilities("computer_act").approval_required);
    assert!(!tool_capabilities("computer_session_close").approval_required);
}

#[test]
fn computer_action_schema_uses_structured_actions() {
    let tools = McpServer::tool_router().list_all();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "computer_act")
        .expect("computer_act tool");
    let serialized = serde_json::to_value(tool).expect("serialize tool");
    let schema = serialized
        .get("inputSchema")
        .or_else(|| serialized.get("input_schema"))
        .expect("input schema");
    assert_eq!(
        schema.pointer("/properties/sessionId/type"),
        Some(&Value::String("string".into()))
    );
    assert_eq!(
        schema.pointer("/properties/actions/type"),
        Some(&Value::String("array".into()))
    );
    let schema_text = schema.to_string();
    for action in [
        "click",
        "double_click",
        "drag",
        "scroll",
        "keypress",
        "navigate",
    ] {
        assert!(schema_text.contains(action), "missing action {action}");
    }
}

#[test]
fn screenshot_is_promoted_to_mcp_image_content() {
    let result = tool_result_with_image(serde_json::json!({
        "sessionId": "session",
        "screenshotBase64": "aGVsbG8="
    }));
    assert_eq!(result.content.len(), 2);
    assert!(
        result
            .content
            .iter()
            .any(|content| content.as_image().is_some())
    );
    assert!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("screenshotBase64"))
            .is_none()
    );
}
