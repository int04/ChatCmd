use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime_host::{PlanPromptResolution, PlanPromptResolveError, PlanPromptView};

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PlanQuestionAnswerRequest {
    kind: String,
    option_index: Option<u8>,
    text: Option<String>,
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
        "custom" => PlanPromptResolution::Custom(request.text.unwrap_or_default()),
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid plan answer",
                "kind must be 'option' or 'custom'.",
            ));
        }
    };
    let prompt = state
        .plan_prompts
        .resolve(id.trim(), resolution)
        .map_err(plan_prompt_problem)?;
    Ok(Json(json!({
        "accepted": true,
        "questionId": prompt.id,
        "taskId": prompt.task_id,
        "turnId": prompt.turn_id,
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
