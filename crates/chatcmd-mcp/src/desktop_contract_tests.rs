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
    for flag in [
        "observeAfter",
        "includeScreenshot",
        "includeElements",
        "knownScreenshotToken",
    ] {
        assert!(
            schema.pointer(&format!("/properties/{flag}")).is_some(),
            "missing fast-loop flag {flag}"
        );
    }
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
fn desktop_observe_schema_accepts_opt_in_image_deduplication() {
    let tool = McpServer::tool_router()
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "desktop_window_observe")
        .expect("desktop observe tool");
    let serialized = serde_json::to_value(tool).expect("serialize tool");
    let schema = serialized
        .get("inputSchema")
        .or_else(|| serialized.get("input_schema"))
        .expect("input schema");
    assert!(schema.pointer("/properties/knownScreenshotToken").is_some());
}

#[test]
fn desktop_element_action_deserializes_with_common_context() {
    let tools = McpServer::tool_router().list_all();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "desktop_element_act")
        .expect("desktop_element_act tool");
    let serialized = serde_json::to_value(tool).expect("serialize tool");
    let schema = serialized
        .get("inputSchema")
        .or_else(|| serialized.get("input_schema"))
        .expect("input schema");
    for flag in [
        "observeAfter",
        "includeScreenshot",
        "includeElements",
        "knownScreenshotToken",
    ] {
        assert!(
            schema.pointer(&format!("/properties/{flag}")).is_some(),
            "missing fast-loop flag {flag}"
        );
    }

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
    assert!(arguments.observe_after);
    assert!(arguments.include_screenshot);
    assert!(!arguments.include_elements);
}

#[test]
fn desktop_action_requests_default_to_fast_visual_verification() {
    let element: chatcmd_runtime::DesktopElementActRequest =
        serde_json::from_value(serde_json::json!({
            "observationId": "observation",
            "elementId": "element",
            "action": "invoke"
        }))
        .expect("desktop element request");
    assert!(element.observe_after);
    assert!(element.include_screenshot);
    assert!(!element.include_elements);

    let input: chatcmd_runtime::DesktopInputActRequest =
        serde_json::from_value(serde_json::json!({
            "inputSessionId": "session",
            "actions": [{"type": "wait", "durationMs": 1}]
        }))
        .expect("desktop input request");
    assert!(input.observe_after);
    assert!(input.include_screenshot);
    assert!(!input.include_elements);
}

#[test]
fn nested_desktop_observation_screenshot_is_promoted_to_image_content() {
    let result = tool_result_with_image(serde_json::json!({
        "completed": true,
        "observationInvalidated": true,
        "observation": {
            "observationId": "next",
            "screenshotBase64": "aGVsbG8="
        }
    }));
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
            .and_then(|value| value.pointer("/observation/screenshotBase64"))
            .is_none()
    );
}

#[test]
fn desktop_screenshot_token_omits_only_an_identical_image() {
    let mut first = serde_json::json!({
        "completed": true,
        "observation": {"observationId": "first", "screenshotBase64": "aGVsbG8="}
    });
    deduplicate_desktop_screenshot(&mut first, None);
    let token = first["screenshotToken"]
        .as_str()
        .expect("token for first image")
        .to_owned();
    assert_eq!(first["screenshotUnchanged"], false);
    assert_eq!(
        tool_result_with_image(first)
            .content
            .iter()
            .filter(|content| content.as_image().is_some())
            .count(),
        1
    );

    let mut repeated = serde_json::json!({
        "completed": true,
        "observation": {"observationId": "second", "screenshotBase64": "aGVsbG8="}
    });
    deduplicate_desktop_screenshot(&mut repeated, Some(&token));
    assert_eq!(repeated["screenshotToken"], token);
    assert_eq!(repeated["screenshotUnchanged"], true);
    assert!(repeated.pointer("/observation/screenshotBase64").is_none());
    assert!(
        tool_result_with_image(repeated)
            .content
            .iter()
            .all(|content| content.as_image().is_none())
    );

    let mut changed = serde_json::json!({"screenshotBase64": "ZGlmZmVyZW50"});
    deduplicate_desktop_screenshot(&mut changed, Some(&token));
    assert_eq!(changed["screenshotUnchanged"], false);
    assert_ne!(changed["screenshotToken"], token);
    assert!(changed.get("screenshotBase64").is_some());

    let mut without_image = serde_json::json!({"observationId": "metadata-only"});
    deduplicate_desktop_screenshot(&mut without_image, Some(&token));
    assert!(without_image.get("screenshotToken").is_none());
    assert!(without_image.get("screenshotUnchanged").is_none());
}

#[test]
fn screenshot_token_is_not_forwarded_to_runtime_arguments() {
    let mut arguments = serde_json::json!({
        "windowId": "window",
        "knownScreenshotToken": "sha256:known"
    });
    assert_eq!(
        desktop_known_screenshot_token("desktop_window_observe", &mut arguments).as_deref(),
        Some("sha256:known")
    );
    assert!(arguments.get("knownScreenshotToken").is_none());
    assert_eq!(arguments["windowId"], "window");
}

#[test]
fn desktop_refusals_preserve_observed_reason_and_never_suggest_bypass() {
    for (code, outcome, recovery) in [
        ("approval_required", "notStarted", "requestApproval"),
        ("approval_stale", "notStarted", "requestApprovalAgain"),
        ("policy_denied", "notStarted", "stopAndReportPolicyDenied"),
        ("desktop_target_denied", "notStarted", "stopAndReportDenied"),
        (
            "desktop_action_outcome_unknown",
            "unknown",
            "observeAgainWithoutRepeatingAction",
        ),
    ] {
        let value = error_value(&RuntimeError::new(code, "observed reason"));
        assert_eq!(value["error"]["code"], code);
        assert_eq!(value["error"]["message"], "observed reason");
        assert_eq!(value["error"]["outcome"], outcome);
        assert_eq!(value["error"]["recovery"], recovery);
    }
}

#[test]
fn desktop_flow_respects_configured_action_time_policy_without_bypass_flag() {
    let tools = McpServer::tool_router().list_all();
    for name in ["desktop_element_act", "desktop_input_act"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("desktop action tool");
        let description = tool.description.as_deref().expect("tool description");
        assert!(description.contains("configured execution policy"));
        assert!(description.contains("action-time approval when required"));
        assert!(description.contains("isolated in its own call"));
        assert!(description.contains("hard-denied") || description.contains("hard denies"));

        let serialized = serde_json::to_value(tool).expect("serialize tool");
        let schema = serialized
            .get("inputSchema")
            .or_else(|| serialized.get("input_schema"))
            .expect("input schema")
            .to_string();
        assert!(!schema.contains("userConfirmed"));
    }
}

#[test]
fn post_action_observation_failure_is_a_non_retriable_action_warning() {
    let warning = chatcmd_runtime::DesktopVerificationWarning::post_action_observation_failed();
    let value = serde_json::to_value(warning).expect("verification warning");
    assert_eq!(value["code"], "postActionObservationFailed");
    assert_eq!(value["actionCompleted"], true);
    assert_eq!(value["retryAction"], false);
    assert_eq!(value["recovery"], "observeAgainWithoutRepeatingAction");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| message.contains("without repeating the action"))
    );

    let tools = McpServer::tool_router().list_all();
    for name in ["desktop_element_act", "desktop_input_act"] {
        let description = tools
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.description.as_deref())
            .expect("desktop action description");
        assert!(description.contains("verificationWarning"));
        assert!(description.contains("without repeating"));
    }
}

#[test]
fn partial_physical_input_is_a_non_retriable_batch_warning() {
    let warning = chatcmd_runtime::DesktopInputExecutionWarning::partial_input_execution();
    let value = serde_json::to_value(warning).expect("input execution warning");
    assert_eq!(value["code"], "partialInputExecution");
    assert_eq!(value["retryAction"], false);
    assert_eq!(value["recovery"], "observeAgainWithoutRepeatingInput");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| message.contains("without repeating input"))
    );

    let tools = McpServer::tool_router().list_all();
    let description = tools
        .iter()
        .find(|tool| tool.name == "desktop_input_act")
        .and_then(|tool| tool.description.as_deref())
        .expect("desktop input description");
    assert!(description.contains("completedActionCount"));
    assert!(description.contains("executionWarning"));
    assert!(description.contains("never replay the batch"));
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
    let element_act = schema_for("desktop_element_act");
    assert!(element_act.contains("observationInvalidated"));
    assert!(element_act.contains("observationId"));
    assert!(element_act.contains("verificationWarning"));
    assert!(element_act.contains("observeAgainWithoutRepeatingAction"));
    let input_act = schema_for("desktop_input_act");
    assert!(input_act.contains("overlayVisible"));
    assert!(input_act.contains("observationId"));
    assert!(input_act.contains("verificationWarning"));
    assert!(input_act.contains("observeAgainWithoutRepeatingAction"));
    assert!(input_act.contains("completedActionCount"));
    assert!(input_act.contains("executionWarning"));
    assert!(input_act.contains("observeAgainWithoutRepeatingInput"));
    for name in [
        "desktop_window_observe",
        "desktop_element_act",
        "desktop_input_act",
    ] {
        let schema = schema_for(name);
        assert!(schema.contains("screenshotToken"));
        assert!(schema.contains("screenshotUnchanged"));
    }
}
