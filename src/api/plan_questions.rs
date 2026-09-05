use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime_host::{
    PlanPromptResolution, PlanPromptResolveError, PlanPromptView,
    plan_prompt_persistence::{is_execution_consent, persist_plan_prompt_resolution},
};

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PlanQuestionAnswerRequest {
    kind: String,
    option_index: Option<u8>,
    text: Option<String>,
    task_id: Option<String>,
    turn_id: Option<String>,
}

pub(super) async fn pending_plan_questions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PlanPromptView>>, Problem> {
    state
        .plan_prompts
        .pending()
        .map(Json)
        .map_err(plan_prompt_problem)
}

pub(super) async fn answer_plan_question(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<PlanQuestionAnswerRequest>,
) -> Result<Json<Value>, Problem> {
    if id.trim().is_empty() || id.len() > 200 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid plan question",
            "Question ID is required and cannot exceed 200 characters.",
        ));
    }
    let resolution = match request.kind.as_str() {
        "option" => PlanPromptResolution::Option(request.option_index.ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid plan answer",
                "optionIndex is required when kind is 'option'.",
            )
        })?),
        "custom" => PlanPromptResolution::Custom(request.text.clone().unwrap_or_default()),
        "approveExecution" => PlanPromptResolution::ApproveExecution,
        "denyExecution" => PlanPromptResolution::DenyExecution,
        "cancel" => PlanPromptResolution::Cancel,
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid plan answer",
                "kind must be option, custom, approveExecution, denyExecution, or cancel.",
            ));
        }
    };
    let prompt = state
        .plan_prompts
        .view(id.trim())
        .map_err(plan_prompt_problem)?;
    if is_execution_consent(&prompt) && (request.task_id.is_none() || request.turn_id.is_none()) {
        return Err(plan_prompt_problem(PlanPromptResolveError::ScopeMismatch));
    }
    if request
        .task_id
        .as_deref()
        .is_some_and(|value| value != prompt.task_id)
        || request
            .turn_id
            .as_deref()
            .is_some_and(|value| value != prompt.turn_id)
    {
        return Err(plan_prompt_problem(PlanPromptResolveError::ScopeMismatch));
    }
    let resolved_at_ms = current_time_ms();
    persist_plan_prompt_resolution(
        state.repository.pool(),
        &prompt,
        &resolution,
        resolved_at_ms,
    )
    .await
    .map_err(plan_prompt_problem)?;
    let prompt = state
        .plan_prompts
        .resolve(
            id.trim(),
            request.task_id.as_deref(),
            request.turn_id.as_deref(),
            resolution,
            resolved_at_ms,
        )
        .map_err(plan_prompt_problem)?;
    Ok(Json(json!({
        "accepted": true,
        "questionId": prompt.id,
        "taskId": prompt.task_id,
        "turnId": prompt.turn_id,
        "questionKind": prompt.question_kind,
    })))
}

fn plan_prompt_problem(error: PlanPromptResolveError) -> Problem {
    match error {
        PlanPromptResolveError::NotFound => Problem::new(
            StatusCode::NOT_FOUND,
            "Plan question expired",
            "This plan question is no longer pending.",
        ),
        PlanPromptResolveError::InvalidOption => Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid plan answer",
            "optionIndex must be 1 or 2.",
        ),
        PlanPromptResolveError::InvalidCustom => Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid custom answer",
            "Custom answer must contain 1 to 2,000 characters.",
        ),
        PlanPromptResolveError::InvalidResolution => Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid plan answer",
            "This answer kind is not valid for the pending question kind.",
        ),
        PlanPromptResolveError::ScopeMismatch => Problem::new(
            StatusCode::CONFLICT,
            "Plan question scope changed",
            "The task or turn does not match this pending execution consent.",
        ),
        PlanPromptResolveError::Expired => Problem::new(
            StatusCode::GONE,
            "Plan question expired",
            "The response deadline passed and no execution was authorized.",
        ),
        PlanPromptResolveError::ReceiverGone => Problem::new(
            StatusCode::CONFLICT,
            "Plan question closed",
            "The AI turn is no longer waiting for this plan question.",
        ),
        PlanPromptResolveError::Unavailable => Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Plan question unavailable",
            "The local plan question registry is unavailable.",
        ),
    }
}

fn current_time_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::plan_prompt_persistence::{
        execution_consent_kind, persist_pending_plan_prompt,
    };
    use crate::runtime_host::user_message_tests;
    use chatcmd_mcp::RuntimeApi as _;
    use chatcmd_runtime::OperationContext;

    #[tokio::test]
    async fn scoped_consent_is_durable_single_use_and_does_not_mint_authority() {
        let (host, agent_id, directory) = user_message_tests::test_host().await;
        let mut context = OperationContext::new("plan-api-user", &agent_id, "agent_user_message");
        context.turn_id = Some("plan-api-turn".to_owned());
        context.conversation_scope_id = Some("plan-api-scope".to_owned());
        let accepted = host
            .call(
                "agent_user_message",
                context,
                json!({"content":"test durable plan consent"}),
            )
            .await
            .expect("create task");
        let task_id = accepted["taskId"].as_str().expect("task ID").to_owned();
        let turn_id = accepted["turnId"].as_str().expect("turn ID").to_owned();
        let state = Arc::new(
            host.test_app_state(directory.path().join("chatcmd.db").display().to_string()),
        );
        let (api_view, _api_receiver, _api_guard) = state
            .plan_prompts
            .register(
                task_id.clone(),
                turn_id.clone(),
                agent_id,
                "Proceed?".to_owned(),
                ["Yes".to_owned(), "No".to_owned()],
                execution_consent_kind(),
                current_time_ms(),
                10_000,
            )
            .expect("register API prompt");
        persist_pending_plan_prompt(state.repository.pool(), &api_view)
            .await
            .expect("persist API prompt");

        let cross_task = answer_plan_question(
            State(state.clone()),
            Path(api_view.id.clone()),
            Json(PlanQuestionAnswerRequest {
                kind: "approveExecution".to_owned(),
                option_index: None,
                text: None,
                task_id: Some("another-task".to_owned()),
                turn_id: Some(turn_id.clone()),
            }),
        )
        .await
        .expect_err("cross-task answer must fail");
        assert_eq!(cross_task.status, StatusCode::CONFLICT);

        let request = PlanQuestionAnswerRequest {
            kind: "approveExecution".to_owned(),
            option_index: None,
            text: None,
            task_id: Some(task_id.clone()),
            turn_id: Some(turn_id.clone()),
        };
        let Json(response) = answer_plan_question(
            State(state.clone()),
            Path(api_view.id.clone()),
            Json(request),
        )
        .await
        .expect("approve consent");
        assert_eq!(response["accepted"], true);
        let row = sqlx::query("SELECT state,resolution_json FROM plan_questions WHERE id=?")
            .bind(&api_view.id)
            .fetch_one(state.repository.pool())
            .await
            .expect("durable decision");
        assert_eq!(row.get::<String, _>("state"), "approved");
        assert!(
            row.get::<String, _>("resolution_json")
                .contains("C01 tool authorization still applies")
        );
        let grants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM approval_grants WHERE task_id=? AND state='active'",
        )
        .bind(&task_id)
        .fetch_one(state.repository.pool())
        .await
        .expect("grant count");
        assert_eq!(grants, 0);

        let replay = answer_plan_question(
            State(state),
            Path(api_view.id),
            Json(PlanQuestionAnswerRequest {
                kind: "approveExecution".to_owned(),
                option_index: None,
                text: None,
                task_id: Some(task_id),
                turn_id: Some(turn_id),
            }),
        )
        .await
        .expect_err("replay must fail");
        assert_eq!(replay.status, StatusCode::NOT_FOUND);
    }
}
