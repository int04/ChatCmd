//! Durable plan-question audit and single-winner terminal transitions.

use chatcmd_runtime::{RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::{Row as _, SqlitePool};

use super::plan_prompt::{
    PlanPromptResolution, PlanPromptResolveError, PlanPromptView, PlanQuestionKind,
};
use super::{RuntimeHost, now_ms};

pub(crate) fn is_execution_consent(view: &PlanPromptView) -> bool {
    view.question_kind == PlanQuestionKind::ExecutionConsent
}

#[cfg(test)]
pub(crate) const fn execution_consent_kind() -> PlanQuestionKind {
    PlanQuestionKind::ExecutionConsent
}

pub(crate) async fn persist_pending_plan_prompt(
    pool: &SqlitePool,
    view: &PlanPromptView,
) -> RuntimeResult<()> {
    let kind = kind_name(view.question_kind);
    let options_json = serde_json::to_string(&view.options)
        .map_err(|_| storage_error("plan question options could not be encoded"))?;
    let scope_context = json!({
        "basis": "task+turn+issuer+kind+question+options",
        "limitation": "plan revision and versioned project-scope snapshot are unavailable",
        "authorizationEffect": "none; execution remains subject to C01 tool authorization",
    })
    .to_string();
    sqlx::query("INSERT INTO plan_questions(id,task_id,turn_id,issuer_agent_id,kind,question,options_json,scope_digest,scope_context_json,state,resolution_json,created_at_ms,deadline_at_ms,resolved_at_ms) VALUES(?,?,?,?,?,?,?,?,?,'pending',NULL,?,?,NULL)")
        .bind(&view.id)
        .bind(&view.task_id)
        .bind(&view.turn_id)
        .bind(&view.issuer_agent_id)
        .bind(kind)
        .bind(&view.question)
        .bind(options_json)
        .bind(&view.scope_digest)
        .bind(scope_context)
        .bind(view.created_at_ms)
        .bind(view.deadline_at_ms)
        .execute(pool)
        .await
        .map_err(|_| storage_error("plan question could not be persisted"))?;
    Ok(())
}

pub(crate) async fn persist_plan_prompt_resolution(
    pool: &SqlitePool,
    view: &PlanPromptView,
    resolution: &PlanPromptResolution,
    resolved_at_ms: i64,
) -> Result<(), PlanPromptResolveError> {
    if resolved_at_ms >= view.deadline_at_ms {
        return match persist_plan_prompt_timeout(pool, view, resolved_at_ms).await {
            Ok(()) => Err(PlanPromptResolveError::Expired),
            Err(error) => Err(error),
        };
    }
    let terminal = terminal_record(view, resolution)?;
    let affected = sqlx::query("UPDATE plan_questions SET state=?,resolution_json=?,resolved_at_ms=? WHERE id=? AND task_id=? AND turn_id=? AND state='pending' AND deadline_at_ms>?")
        .bind(terminal.state)
        .bind(terminal.resolution.to_string())
        .bind(resolved_at_ms)
        .bind(&view.id)
        .bind(&view.task_id)
        .bind(&view.turn_id)
        .bind(resolved_at_ms)
        .execute(pool)
        .await
        .map_err(|_| PlanPromptResolveError::Unavailable)?
        .rows_affected();
    if affected == 1 {
        return Ok(());
    }
    classify_failed_transition(pool, view, resolved_at_ms).await
}

pub(crate) async fn persist_plan_prompt_timeout(
    pool: &SqlitePool,
    view: &PlanPromptView,
    resolved_at_ms: i64,
) -> Result<(), PlanPromptResolveError> {
    if resolved_at_ms < view.deadline_at_ms {
        return Err(PlanPromptResolveError::InvalidResolution);
    }
    let resolution = json!({
        "kind": "timeout",
        "consentState": (view.question_kind == PlanQuestionKind::ExecutionConsent)
            .then_some("expired"),
        "executionAuthorized": false,
    });
    let affected = sqlx::query("UPDATE plan_questions SET state='expired',resolution_json=?,resolved_at_ms=? WHERE id=? AND task_id=? AND turn_id=? AND state='pending'")
        .bind(resolution.to_string())
        .bind(resolved_at_ms)
        .bind(&view.id)
        .bind(&view.task_id)
        .bind(&view.turn_id)
        .execute(pool)
        .await
        .map_err(|_| PlanPromptResolveError::Unavailable)?
        .rows_affected();
    if affected == 1 {
        Ok(())
    } else {
        classify_failed_transition(pool, view, resolved_at_ms).await
    }
}

pub(crate) async fn cancel_abandoned_plan_prompt(
    pool: SqlitePool,
    view: PlanPromptView,
    resolved_at_ms: i64,
) {
    let resolution = json!({
        "kind": "cancelled",
        "consentState": (view.question_kind == PlanQuestionKind::ExecutionConsent)
            .then_some("cancelled"),
        "executionAuthorized": false,
        "reason": "request disconnected or was cancelled while awaiting an answer",
    });
    if let Err(error) = sqlx::query("UPDATE plan_questions SET state='cancelled',resolution_json=?,resolved_at_ms=? WHERE id=? AND task_id=? AND turn_id=? AND state='pending'")
        .bind(resolution.to_string())
        .bind(resolved_at_ms)
        .bind(&view.id)
        .bind(&view.task_id)
        .bind(&view.turn_id)
        .execute(&pool)
        .await
    {
        tracing::error!(error = ?error, question_id = view.id, "abandoned plan question could not be closed");
    }
}

impl RuntimeHost {
    pub(crate) async fn expire_pending_plan_questions_on_startup(&self) -> RuntimeResult<u64> {
        let resolved_at_ms = now_ms();
        let resolution = json!({
            "kind": "hostRestarted",
            "consentState": "expired",
            "executionAuthorized": false,
            "reason": "pending plan question cannot survive a host restart",
        });
        sqlx::query("UPDATE plan_questions SET state='expired',resolution_json=?,resolved_at_ms=? WHERE state='pending'")
            .bind(resolution.to_string())
            .bind(resolved_at_ms)
            .execute(self.repository.pool())
            .await
            .map(|result| result.rows_affected())
            .map_err(|_| storage_error("pending plan questions could not be expired at startup"))
    }
}

struct TerminalRecord {
    state: &'static str,
    resolution: Value,
}

fn terminal_record(
    view: &PlanPromptView,
    resolution: &PlanPromptResolution,
) -> Result<TerminalRecord, PlanPromptResolveError> {
    match (view.question_kind, resolution) {
        (PlanQuestionKind::Clarification, PlanPromptResolution::Option(index @ 1..=2)) => {
            Ok(TerminalRecord {
                state: "answered",
                resolution: json!({
                    "kind": "option",
                    "optionIndex": index,
                    "text": view.options[usize::from(index - 1)],
                }),
            })
        }
        (PlanQuestionKind::Clarification, PlanPromptResolution::Option(_)) => {
            Err(PlanPromptResolveError::InvalidOption)
        }
        (PlanQuestionKind::Clarification, PlanPromptResolution::Custom(text)) => {
            let text = text.trim();
            if text.is_empty() || text.chars().count() > 2_000 {
                return Err(PlanPromptResolveError::InvalidCustom);
            }
            Ok(TerminalRecord {
                state: "answered",
                resolution: json!({"kind":"custom","text":text}),
            })
        }
        (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::ApproveExecution) => {
            Ok(consent_record("approved", true))
        }
        (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::DenyExecution) => {
            Ok(consent_record("denied", false))
        }
        (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::Cancel) => {
            Ok(consent_record("cancelled", false))
        }
        _ => Err(PlanPromptResolveError::InvalidResolution),
    }
}

fn consent_record(state: &'static str, execution_authorized: bool) -> TerminalRecord {
    TerminalRecord {
        state,
        resolution: json!({
            "kind": state,
            "consentState": state,
            "executionAuthorized": execution_authorized,
            "authorizationEffect": "none; C01 tool authorization still applies",
        }),
    }
}

async fn classify_failed_transition(
    pool: &SqlitePool,
    view: &PlanPromptView,
    resolved_at_ms: i64,
) -> Result<(), PlanPromptResolveError> {
    let row = sqlx::query(
        "SELECT task_id,turn_id,state,deadline_at_ms FROM plan_questions WHERE id=? LIMIT 1",
    )
    .bind(&view.id)
    .fetch_optional(pool)
    .await
    .map_err(|_| PlanPromptResolveError::Unavailable)?
    .ok_or(PlanPromptResolveError::NotFound)?;
    if row.get::<String, _>("task_id") != view.task_id
        || row.get::<String, _>("turn_id") != view.turn_id
    {
        return Err(PlanPromptResolveError::ScopeMismatch);
    }
    if row.get::<String, _>("state") != "pending" {
        return Err(PlanPromptResolveError::NotFound);
    }
    if resolved_at_ms >= row.get::<i64, _>("deadline_at_ms") {
        Err(PlanPromptResolveError::Expired)
    } else {
        Err(PlanPromptResolveError::Unavailable)
    }
}

const fn kind_name(kind: PlanQuestionKind) -> &'static str {
    match kind {
        PlanQuestionKind::Clarification => "clarification",
        PlanQuestionKind::ExecutionConsent => "executionConsent",
    }
}

fn storage_error(message: &str) -> RuntimeError {
    RuntimeError::new("storage_error", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::{PlanPromptRegistry, user_message_tests};
    use serde_json::json;

    async fn persisted_view(
        host: &RuntimeHost,
        agent_id: &str,
        suffix: &str,
        created_at_ms: i64,
        timeout_ms: i64,
    ) -> PlanPromptView {
        let context = user_message_tests::turn_context(
            &format!("request-{suffix}"),
            agent_id,
            "agent_user_message",
            &format!("turn-{suffix}"),
            &format!("scope-{suffix}"),
        );
        let accepted = host
            .call_persisted(
                "agent_user_message",
                context,
                json!({"content":"persist plan question"}),
            )
            .await
            .expect("create task and turn");
        let registry = PlanPromptRegistry::default();
        let (view, _receiver, _guard) = registry
            .register(
                accepted["taskId"].as_str().expect("task ID").to_owned(),
                accepted["turnId"].as_str().expect("turn ID").to_owned(),
                agent_id.to_owned(),
                "Proceed?".to_owned(),
                ["Yes".to_owned(), "No".to_owned()],
                PlanQuestionKind::ExecutionConsent,
                created_at_ms,
                timeout_ms,
            )
            .expect("register prompt");
        persist_pending_plan_prompt(host.repository.pool(), &view)
            .await
            .expect("persist prompt");
        view
    }

    #[tokio::test]
    async fn concurrent_answer_and_timeout_have_exactly_one_terminal_winner() {
        let (host, agent_id, _directory) = user_message_tests::test_host().await;
        let view = persisted_view(&host, &agent_id, "race", 10, 10).await;
        let answer = persist_plan_prompt_resolution(
            host.repository.pool(),
            &view,
            &PlanPromptResolution::ApproveExecution,
            19,
        );
        let timeout = persist_plan_prompt_timeout(host.repository.pool(), &view, 20);
        let (answer, timeout) = tokio::join!(answer, timeout);

        assert_eq!(
            usize::from(answer.is_ok()) + usize::from(timeout.is_ok()),
            1
        );
        let row = sqlx::query(
            "SELECT state,resolution_json,resolved_at_ms FROM plan_questions WHERE id=?",
        )
        .bind(&view.id)
        .fetch_one(host.repository.pool())
        .await
        .expect("terminal prompt");
        assert!(matches!(
            row.get::<String, _>("state").as_str(),
            "approved" | "expired"
        ));
        assert!(row.get::<Option<String>, _>("resolution_json").is_some());
        assert!(row.get::<Option<i64>, _>("resolved_at_ms").is_some());
    }

    #[tokio::test]
    async fn startup_expires_pending_without_reviving_approved_consent() {
        let (host, agent_id, _directory) = user_message_tests::test_host().await;
        let pending = persisted_view(&host, &agent_id, "pending", 10, 1_000).await;
        let approved = persisted_view(&host, &agent_id, "approved", 10, 1_000).await;
        persist_plan_prompt_resolution(
            host.repository.pool(),
            &approved,
            &PlanPromptResolution::ApproveExecution,
            20,
        )
        .await
        .expect("approve before restart");

        assert_eq!(
            host.expire_pending_plan_questions_on_startup()
                .await
                .expect("startup recovery"),
            1
        );
        let states = sqlx::query("SELECT id,state,resolution_json FROM plan_questions")
            .fetch_all(host.repository.pool())
            .await
            .expect("persisted states");
        let state = |id: &str| {
            states
                .iter()
                .find(|row| row.get::<String, _>("id") == id)
                .map(|row| row.get::<String, _>("state"))
                .expect("question state")
        };
        assert_eq!(state(&pending.id), "expired");
        assert_eq!(state(&approved.id), "approved");
        assert!(states.iter().all(|row| {
            !row.get::<String, _>("resolution_json")
                .contains("executionAuthorized\":true")
                || row.get::<String, _>("id") == approved.id
        }));
    }

    #[tokio::test]
    async fn dropped_waiter_persists_cancelled_without_waiting_for_deadline() {
        let (host, agent_id, _directory) = user_message_tests::test_host().await;
        let initial = user_message_tests::turn_context(
            "disconnect-user",
            &agent_id,
            "agent_user_message",
            "disconnect-turn",
            "disconnect-scope",
        );
        let accepted = host
            .call_persisted(
                "agent_user_message",
                initial,
                json!({"content":"ask then disconnect"}),
            )
            .await
            .expect("create turn");
        let mut context = chatcmd_runtime::OperationContext::new(
            "disconnect-question",
            &agent_id,
            "agent_plan_question",
        );
        context.task_id = accepted["taskId"].as_str().map(str::to_owned);
        context.turn_id = accepted["turnId"].as_str().map(str::to_owned);
        let waiting_host = host.clone();
        let waiter = tokio::spawn(async move {
            waiting_host
                .ask_plan_question_with_kind(
                    &context,
                    "Proceed?".to_owned(),
                    ["Yes".to_owned(), "No".to_owned()],
                    PlanQuestionKind::ExecutionConsent,
                )
                .await
        });
        let mut question_id = None;
        for _ in 0..100 {
            let value = sqlx::query_scalar::<_, String>(
                "SELECT id FROM plan_questions WHERE state='pending' LIMIT 1",
            )
            .fetch_optional(host.repository.pool())
            .await
            .expect("read pending");
            if let Some(value) = value {
                question_id = Some(value);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let question_id = question_id.expect("question became pending");
        waiter.abort();
        for _ in 0..100 {
            let state: String = sqlx::query_scalar("SELECT state FROM plan_questions WHERE id=?")
                .bind(&question_id)
                .fetch_one(host.repository.pool())
                .await
                .expect("read state");
            if state == "cancelled" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("dropped waiter did not persist cancellation");
    }
}
