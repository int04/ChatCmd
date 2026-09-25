use super::*;
use serde_json::Value;

#[test]
fn desktop_tools_are_advertised_with_explicit_risk_classes() {
    let expected = [
        (
            "desktop_window_list",
            ToolOperationClass::MetadataRead,
            false,
        ),
        (
            "desktop_window_observe",
            ToolOperationClass::ContentRead,
            true,
        ),
        ("desktop_element_act", ToolOperationClass::Mutation, true),
        (
            "desktop_input_begin",
            ToolOperationClass::ProcessExecution,
            true,
        ),
        ("desktop_input_act", ToolOperationClass::Mutation, true),
        ("desktop_input_end", ToolOperationClass::StopCleanup, false),
    ];
    for (name, operation_class, approval_required) in expected {
        assert!(TOOL_NAMES.iter().any(|candidate| candidate == name));
        let capabilities = tool_capabilities(name);
        assert_eq!(capabilities.operation_class, operation_class);
        assert_eq!(capabilities.approval_required, approval_required);
    }
}

#[test]
fn desktop_input_schema_uses_structured_actions() {
    let tools = McpServer::tool_router().list_all();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "desktop_input_act")
        .expect("desktop_input_act tool");
    let serialized = serde_json::to_value(tool).expect("serialize tool");
    let schema = serialized
        .get("inputSchema")
        .or_else(|| serialized.get("input_schema"))
        .expect("input schema");
    assert_eq!(
        schema.pointer("/properties/inputSessionId/type"),
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
        "type",
    ] {
        assert!(schema_text.contains(action), "missing action {action}");
    }
}

#[test]
fn desktop_element_action_deserializes_with_common_context() {
    let arguments: DesktopElementActArgs = serde_json::from_value(serde_json::json!({
        "taskId": "task",
        "turnId": "turn",
        "observationId": "observation",
        "elementId": "element",
        "action": "set_value",
        "text": "replacement"
    }))
    .expect("desktop element action");
    assert_eq!(arguments.common.task_id.as_deref(), Some("task"));
    assert!(matches!(
        arguments.action,
        chatcmd_runtime::DesktopElementAction::SetValue { ref text } if text == "replacement"
    ));
}

#[test]
fn desktop_result_schemas_are_specific() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("tools array");
    let schema_for = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| tool["resultSchema"].to_string())
            .expect("desktop result schema")
    };
    assert!(schema_for("desktop_window_list").contains("windowId"));
    assert!(schema_for("desktop_window_observe").contains("observationId"));
    assert!(schema_for("desktop_element_act").contains("observationInvalidated"));
    assert!(schema_for("desktop_input_begin").contains("overlayVisible"));
}
