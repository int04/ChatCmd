use chatcmd_runtime::{
    DesktopControlService, DesktopElementActRequest, DesktopElementAction, DesktopInputActRequest,
    DesktopInputAction, DesktopMouseButton, DesktopObserveRequest, OperationContext,
};
use serde_json::json;

#[test]
fn desktop_observe_defaults_to_screenshot_and_elements() {
    let request: DesktopObserveRequest = serde_json::from_value(json!({ "windowId": "window-1" }))
        .expect("minimal observe request should deserialize");

    assert_eq!(request.window_id, "window-1");
    assert!(request.include_screenshot);
    assert!(request.include_elements);
}

#[test]
fn desktop_observe_rejects_unknown_fields() {
    let error = serde_json::from_value::<DesktopObserveRequest>(json!({
        "windowId": "window-1",
        "nativeHandle": 1234
    }))
    .expect_err("native handles must not be accepted from a caller");

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn semantic_action_uses_flattened_snake_case_contract() {
    let request: DesktopElementActRequest = serde_json::from_value(json!({
        "observationId": "observation-1",
        "elementId": "element-7",
        "action": "set_value",
        "text": "hello"
    }))
    .expect("set_value action should deserialize");

    assert_eq!(request.observation_id, "observation-1");
    assert_eq!(request.element_id, "element-7");
    assert!(matches!(
        request.action,
        DesktopElementAction::SetValue { ref text } if text == "hello"
    ));
}

#[test]
fn takeover_actions_use_camel_case_fields_and_default_left_button() {
    let request: DesktopInputActRequest = serde_json::from_value(json!({
        "inputSessionId": "input-1",
        "actions": [
            { "type": "click", "x": 20, "y": 30 },
            {
                "type": "drag",
                "startX": 10,
                "startY": 11,
                "endX": 90,
                "endY": 91
            }
        ]
    }))
    .expect("bounded takeover actions should deserialize");

    assert_eq!(request.input_session_id, "input-1");
    assert!(matches!(
        request.actions.first(),
        Some(DesktopInputAction::Click {
            button: DesktopMouseButton::Left,
            ..
        })
    ));
    assert!(matches!(
        request.actions.get(1),
        Some(DesktopInputAction::Drag {
            duration_ms: 500,
            ..
        })
    ));
}

#[test]
fn takeover_action_rejects_unexpected_fields() {
    let error = serde_json::from_value::<DesktopInputActRequest>(json!({
        "inputSessionId": "input-1",
        "actions": [{
            "type": "keypress",
            "keys": ["Enter"],
            "windowId": "window-2"
        }]
    }))
    .expect_err("individual action variants must reject unknown fields");

    assert!(error.to_string().contains("unknown field"));
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[ignore = "manual Windows smoke test; enumerates current top-level windows without injecting input"]
async fn desktop_window_binding_smoke_does_not_require_foreground_input() {
    let service = DesktopControlService::new();
    let context = OperationContext::new(
        "desktop-smoke",
        "desktop-smoke-agent",
        "desktop_window_list",
    );
    let listed = service
        .list_windows(&context)
        .await
        .expect("window enumeration should succeed");
    let Some(window) = listed.windows.first() else {
        return;
    };
    let observed = service
        .observe(
            &context,
            DesktopObserveRequest {
                window_id: window.window_id.clone(),
                include_screenshot: false,
                include_elements: false,
            },
        )
        .await
        .expect("metadata-only observation should preserve the opaque binding");
    assert_eq!(observed.window.window_id, window.window_id);
    assert!(observed.screenshot_base64.is_none());
    assert!(observed.elements.is_empty());
}
