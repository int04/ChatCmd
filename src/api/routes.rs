use std::sync::Arc;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{any, get, patch, post},
};

use crate::websocket::AppState;

use super::{
    Problem, agents::*, auth, backend, chatgpt::*, chatgpt_completion::*, chatgpt_queue::*, crypto,
    data::*, folders::*, overview::*, payment::*, plan_questions::*, sessions::*, settings::*,
    skills::*, system::*, task_controls::*, task_delete::*, task_views::*, tunnels::*, updates::*,
    workspaces::*,
};

pub(crate) fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let protected = Router::new()
        .route("/overview", get(overview))
        .route("/mcp/status", get(mcp_status))
        .route("/mcp/agents", get(list_agents).post(create_agent))
        .route(
            "/mcp/agents/{id}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/mcp/agents/{id}/rotate-secret", post(rotate_secret))
        .route("/mcp/agents/{id}/enabled", patch(set_enabled))
        .route("/mcp/tools", get(tools))
        .route("/mcp/tool-presets", get(presets))
        .route("/mcp/tunnels", get(list_tunnels).post(create_tunnel))
        .route("/mcp/tunnels/{id}", axum::routing::delete(delete_tunnel))
        .route("/mcp/tunnels/{id}/test", post(test_tunnel))
        .route("/mcp/tunnel-connection", get(managed_tunnel_status))
        .route(
            "/mcp/tunnel-connection/connect",
            post(connect_managed_tunnel),
        )
        .route(
            "/mcp/tunnel-connection/disconnect",
            post(disconnect_managed_tunnel),
        )
        .route("/mcp/agents/{id}/plugin-links", get(plugin_links))
        .route(
            "/mcp/agents/{id}/plugin-links/{tunnel_id}",
            post(copy_plugin_link),
        )
        .route("/system/folder-picker", post(pick_project_folder))
        .route("/system/exit", post(exit_application))
        .route(
            "/workspaces/projects",
            get(workspace_projects).post(save_workspace_project),
        )
        .route(
            "/workspaces/projects/order",
            axum::routing::put(reorder_workspace_projects),
        )
        .route("/chatgpt/requests", post(create_request))
        .route("/chatgpt/requests/{id}", get(request))
        .route("/chatgpt/tasks/{task_id}", get(task_bridge))
        .route("/chatgpt/tasks/{task_id}/messages", post(continue_message))
        .route("/chatgpt/tasks/{task_id}/stop", post(stop_message))
        .route(
            "/chatgpt/tasks/{task_id}/queue",
            get(queue_messages).post(create_queue_message),
        )
        .route(
            "/chatgpt/tasks/{task_id}/queue/order",
            axum::routing::put(reorder_queue_messages),
        )
        .route(
            "/chatgpt/tasks/{task_id}/queue/{message_id}",
            patch(update_queue_message).delete(delete_queue_message),
        )
        .route("/chatgpt/bridge/{request_id}/started", post(bridge_started))
        .route(
            "/chatgpt/bridge/{request_id}/identity",
            post(bridge_identity),
        )
        .route("/chatgpt/bridge/{request_id}/result", post(bridge_result))
        .route(
            "/chatgpt/bridge/{request_id}/browser-completed",
            post(bridge_browser_completed),
        )
        .route("/tasks", get(tasks))
        .route(
            "/tasks/approvals/pending",
            get(pending_conversation_approvals),
        )
        .route("/plan/questions/pending", get(pending_plan_questions))
        .route("/plan/questions/{id}/answer", post(answer_plan_question))
        .route("/tasks/{id}", get(task).delete(delete_task))
        .route("/tasks/{id}/activities/{activity_id}", get(task_activity))
        .route("/tasks/{id}/title", axum::routing::put(set_task_title))
        .route(
            "/tasks/{id}/command-execution-mode",
            get(task_execution_mode).put(set_task_execution_mode),
        )
        .route(
            "/tasks/{id}/activities/{activity_id}/approval",
            post(resolve_task_approval),
        )
        .route(
            "/tasks/{task_id}/activities/{activity_id}/stop",
            post(stop_task_activity),
        )
        .route("/tasks/{id}/{action}", post(task_action))
        .route("/sessions", get(sessions))
        .route("/sessions/terminals/live", get(live_terminals))
        .route("/sessions/{id}/live", get(terminal_live_output))
        .route("/sessions/{id}/input", post(terminal_input))
        .route("/sessions/{id}/resize", post(terminal_resize))
        .route("/sessions/{id}", get(session))
        .route("/sessions/{id}/{action}", post(session_action))
        .route("/skills", get(skills))
        .route("/skills/preview", post(preview_skills))
        .route("/skills/install", post(install_skill))
        .route("/skills/{id}", get(skill).delete(delete_skill))
        .route("/skills/{id}/enabled", patch(set_skill_enabled))
        .route("/skills/{id}/options", patch(set_skill_options))
        .route("/skills/{id}/icon", get(skill_icon))
        .route("/settings", get(settings).put(save_settings))
        .route("/payment/create", post(create_payment))
        .route("/diagnostics/database", get(database_diagnostics))
        .route("/diagnostics/logs", get(diagnostic_logs))
        .route(
            "/diagnostics/user-data",
            axum::routing::delete(delete_all_user_data),
        )
        .route("/updates/status", get(update_status))
        .route("/updates/check", post(check_update))
        .route("/updates/start", post(start_update))
        .route("/updates/restart", post(restart_update))
        .route("/auth/info", get(auth::info))
        .route("/auth/change-password", post(auth::change_password))
        .route("/auth/logout", post(auth::logout))
        .route("/backend/{*path}", any(backend::proxy))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));
    let local = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/crypto/handshake", post(crypto::handshake))
        .route("/system/elevation", get(elevation_status))
        .route("/system/elevation/restart", post(restart_elevated))
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state,
            crypto::encrypted_local_api,
        ))
        .layer(middleware::from_fn(management_header));
    Router::new()
        .route("/ping", get(ping))
        .route("/health", get(health))
        .route("/info", get(info))
        .nest("/local", local)
}

async fn management_header(request: Request, next: Next) -> Result<Response, Problem> {
    let caller = request.headers().get("x-chatcmdclient");
    if caller == Some(&HeaderValue::from_static("local-ui"))
        || caller == Some(&HeaderValue::from_static("chatgpt-extension"))
    {
        Ok(next.run(request).await)
    } else {
        Err(Problem::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "local UI header is required",
        ))
    }
}
