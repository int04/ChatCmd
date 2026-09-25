use super::*;

#[cfg(target_os = "windows")]
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[test]
fn dimensions_are_bounded() {
    assert!(validate_dimensions(1280, 800).is_ok());
    assert!(validate_dimensions(400, 800).is_err());
    assert!(validate_dimensions(1280, 2000).is_err());
}

#[test]
fn brave_is_accepted_by_browser_session_contract() {
    let request: ComputerSessionStartRequest = serde_json::from_value(serde_json::json!({
        "browser": "brave",
        "startUrl": "about:blank"
    }))
    .expect("deserialize Brave session request");
    assert!(matches!(request.browser, ComputerBrowser::Brave));
    let encoded = serde_json::to_value(request.browser).expect("serialize Brave browser");
    assert_eq!(encoded, "brave");
}

#[test]
fn session_owner_is_task_scoped() {
    let mut first = OperationContext::new("one", "agent", "computer_observe");
    first.task_id = Some("task-one".into());
    let mut second = first.clone();
    second.task_id = Some("task-two".into());
    assert!(ensure_owner(&owner(&first), &first).is_ok());
    assert!(ensure_owner(&owner(&first), &second).is_err());
}

#[test]
fn navigation_rejects_local_file_schemes() {
    assert!(cdp::validate_navigation_url("https://example.com").is_ok());
    assert!(cdp::validate_navigation_url("about:blank").is_ok());
    assert!(cdp::validate_navigation_url("file:///C:/secret.txt").is_err());
    assert!(cdp::validate_navigation_url("javascript:alert(1)").is_err());
}

#[test]
fn actions_are_validated_before_execution() {
    assert!(
        validate_actions(
            &[ComputerAction::Click {
                x: 10.0,
                y: 20.0,
                button: MouseButton::Left,
            }],
            800,
            600,
        )
        .is_ok()
    );
    assert!(
        validate_actions(
            &[ComputerAction::Click {
                x: 900.0,
                y: 20.0,
                button: MouseButton::Left,
            }],
            800,
            600,
        )
        .is_err()
    );
    assert!(
        validate_actions(
            &[ComputerAction::Navigate {
                url: "file:///C:/secret.txt".into(),
            }],
            800,
            600,
        )
        .is_err()
    );
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[ignore = "requires an installed Chrome browser"]
async fn isolated_chrome_can_capture_without_desktop_input() {
    let service = ComputerControlService::new();
    let mut context = OperationContext::new("smoke", "test-agent", "computer_session_start");
    context.task_id = Some("test-task".into());
    let info = service
        .start(
            &context,
            ComputerSessionStartRequest {
                browser: ComputerBrowser::Chrome,
                start_url: "about:blank".into(),
                width: 800,
                height: 600,
            },
        )
        .await
        .expect("start isolated Chrome");
    let observation = service
        .observe(&context, &info.session_id)
        .await
        .expect("capture screenshot");
    assert_eq!(observation.url, "about:blank");
    assert!(
        observation
            .screenshot_base64
            .as_deref()
            .is_some_and(|image| image.len() > 100)
    );
    service
        .close(&context, &info.session_id)
        .await
        .expect("close isolated Chrome");
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[ignore = "requires an installed Brave browser"]
async fn isolated_brave_can_capture_without_desktop_input() {
    let service = ComputerControlService::new();
    let context = OperationContext::new("brave-smoke", "test-agent", "computer_session_start");
    let info = service
        .start(
            &context,
            ComputerSessionStartRequest {
                browser: ComputerBrowser::Brave,
                start_url: "about:blank".into(),
                width: 800,
                height: 600,
            },
        )
        .await
        .expect("start isolated Brave");
    assert!(info.headless && info.isolated_profile);
    let observation = service
        .observe(&context, &info.session_id)
        .await
        .expect("capture Brave screenshot");
    assert_eq!(observation.url, "about:blank");
    assert!(observation.screenshot_base64.is_some());
    let session = service
        .session_access(&context, &info.session_id)
        .await
        .expect("access Brave session");
    session
        .client
        .lock()
        .await
        .break_connection_for_test()
        .await
        .expect("break CDP connection");
    let recovered = service
        .observe(&context, &info.session_id)
        .await
        .expect("reconnect read-only Brave observation");
    assert_eq!(recovered.url, "about:blank");
    session
        .client
        .lock()
        .await
        .break_connection_for_test()
        .await
        .expect("break CDP connection again");
    let stopped = service
        .act(
            &context,
            ComputerActRequest {
                session_id: info.session_id.clone(),
                actions: vec![ComputerAction::Screenshot],
                screenshot_after: true,
            },
        )
        .await
        .expect("report action outcome without replay");
    assert_eq!(stopped.completed_action_count, Some(0));
    assert!(
        stopped
            .execution_warning
            .is_some_and(|warning| !warning.retry_action)
    );
    service
        .close(&context, &info.session_id)
        .await
        .expect("close isolated Brave");
}

#[cfg(target_os = "windows")]
#[tokio::test]
#[ignore = "requires an installed Chrome browser"]
async fn isolated_chrome_executes_pointer_free_actions() {
    let (url, server) = serve_test_page().await;
    let service = ComputerControlService::new();
    let mut context = OperationContext::new("action-smoke", "test-agent", "computer_act");
    context.task_id = Some("test-task".into());
    let info = service
        .start(
            &context,
            ComputerSessionStartRequest {
                browser: ComputerBrowser::Chrome,
                start_url: url.clone(),
                width: 800,
                height: 600,
            },
        )
        .await
        .expect("start isolated Chrome");
    let before = service
        .observe(&context, &info.session_id)
        .await
        .expect("initial screenshot");
    let after = service
        .act(
            &context,
            ComputerActRequest {
                session_id: info.session_id.clone(),
                actions: vec![
                    ComputerAction::Click {
                        x: 80.0,
                        y: 38.0,
                        button: MouseButton::Left,
                    },
                    ComputerAction::Type {
                        text: "penguin".into(),
                    },
                    ComputerAction::Screenshot,
                ],
                screenshot_after: false,
            },
        )
        .await
        .expect("execute actions");
    assert_eq!(after.url, url);
    assert_ne!(after.screenshot_base64, before.screenshot_base64);
    assert_eq!(after.completed_action_count, Some(3));
    assert!(after.execution_warning.is_none());
    service
        .close(&context, &info.session_id)
        .await
        .expect("close isolated Chrome");
    server.await.expect("test server task");
}

#[cfg(target_os = "windows")]
async fn serve_test_page() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept browser request");
        let mut request = [0_u8; 4096];
        let _ = socket
            .read(&mut request)
            .await
            .expect("read browser request");
        let body = b"<!doctype html><style>body{margin:0}input{position:fixed;left:20px;top:20px;width:200px;height:30px;font-size:18px}</style><input aria-label='search'>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write headers");
        socket.write_all(body).await.expect("write page");
    });
    (format!("http://{address}/"), task)
}
