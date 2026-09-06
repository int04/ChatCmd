use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{
    RuntimeHost, now_ms,
    plan_prompt_persistence::{
        cancel_abandoned_plan_prompt, persist_pending_plan_prompt, persist_plan_prompt_timeout,
    },
};

const PLAN_QUESTION_TIMEOUT_MS: i64 = 120_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PlanQuestionKind {
    #[default]
    Clarification,
    ExecutionConsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ConsentState {
    Approved,
    Denied,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanPromptView {
    pub(crate) id: String,
    pub(crate) task_id: String,
    pub(crate) turn_id: String,
    pub(crate) question: String,
    pub(crate) options: [String; 2],
    pub(crate) question_kind: PlanQuestionKind,
    pub(crate) issuer_agent_id: String,
    pub(crate) scope_digest: String,
    pub(crate) created_at_ms: i64,
    pub(crate) deadline_at_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanPromptAnswer {
    pub(crate) option_index: Option<u8>,
    pub(crate) text: String,
    pub(crate) custom: bool,
    pub(crate) consent_state: Option<ConsentState>,
}

#[derive(Debug, Clone)]
pub(crate) enum PlanPromptResolution {
    Option(u8),
    Custom(String),
    ApproveExecution,
    DenyExecution,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanPromptResolveError {
    Unavailable,
    NotFound,
    InvalidOption,
    InvalidCustom,
    InvalidResolution,
    ScopeMismatch,
    Expired,
    ReceiverGone,
}

struct PendingPlanPrompt {
    view: PlanPromptView,
    sender: oneshot::Sender<PlanPromptAnswer>,
}

#[derive(Clone, Default)]
pub(crate) struct PlanPromptRegistry {
    pending: Arc<Mutex<HashMap<String, PendingPlanPrompt>>>,
}

pub(crate) struct PlanPromptGuard {
    registry: PlanPromptRegistry,
    id: String,
    persisted: Option<(sqlx::SqlitePool, PlanPromptView)>,
}

impl PlanPromptRegistry {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register(
        &self,
        task_id: String,
        turn_id: String,
        issuer_agent_id: String,
        question: String,
        options: [String; 2],
        question_kind: PlanQuestionKind,
        created_at_ms: i64,
        timeout_ms: i64,
    ) -> Result<
        (
            PlanPromptView,
            oneshot::Receiver<PlanPromptAnswer>,
            PlanPromptGuard,
        ),
        PlanPromptResolveError,
    > {
        let scope_digest = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!(
                "{task_id}\0{turn_id}\0{issuer_agent_id}\0{question_kind:?}\0{question}\0{}\0{}",
                options[0], options[1]
            )
            .as_bytes(),
        )
        .to_string();
        let view = PlanPromptView {
            id: format!("plan-question-{}", Uuid::new_v4()),
            task_id,
            turn_id,
            question,
            options,
            question_kind,
            issuer_agent_id,
            scope_digest,
            created_at_ms,
            deadline_at_ms: created_at_ms.saturating_add(timeout_ms),
        };
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?;
        pending.insert(
            view.id.clone(),
            PendingPlanPrompt {
                view: view.clone(),
                sender,
            },
        );
        drop(pending);
        let guard = PlanPromptGuard {
            registry: self.clone(),
            id: view.id.clone(),
            persisted: None,
        };
        Ok((view, receiver, guard))
    }

    pub(crate) fn pending(&self) -> Result<Vec<PlanPromptView>, PlanPromptResolveError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?;
        let mut values = pending
            .values()
            .map(|entry| entry.view.clone())
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(values)
    }

    pub(crate) fn view(&self, id: &str) -> Result<PlanPromptView, PlanPromptResolveError> {
        self.pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?
            .get(id)
            .map(|entry| entry.view.clone())
            .ok_or(PlanPromptResolveError::NotFound)
    }

    pub(crate) fn resolve(
        &self,
        id: &str,
        task_id: Option<&str>,
        turn_id: Option<&str>,
        resolution: PlanPromptResolution,
        resolved_at_ms: i64,
    ) -> Result<PlanPromptView, PlanPromptResolveError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?;
        let answer = {
            let entry = pending.get(id).ok_or(PlanPromptResolveError::NotFound)?;
            if entry.view.question_kind == PlanQuestionKind::ExecutionConsent
                && (task_id.is_none() || turn_id.is_none())
            {
                return Err(PlanPromptResolveError::ScopeMismatch);
            }
            if task_id.is_some_and(|value| value != entry.view.task_id)
                || turn_id.is_some_and(|value| value != entry.view.turn_id)
            {
                return Err(PlanPromptResolveError::ScopeMismatch);
            }
            if resolved_at_ms >= entry.view.deadline_at_ms {
                pending.remove(id);
                return Err(PlanPromptResolveError::Expired);
            }
            match (entry.view.question_kind, resolution) {
                (PlanQuestionKind::Clarification, PlanPromptResolution::Option(index @ 1..=2)) => {
                    PlanPromptAnswer {
                        option_index: Some(index),
                        text: entry.view.options[usize::from(index - 1)].clone(),
                        custom: false,
                        consent_state: None,
                    }
                }
                (PlanQuestionKind::Clarification, PlanPromptResolution::Option(_)) => {
                    return Err(PlanPromptResolveError::InvalidOption);
                }
                (PlanQuestionKind::Clarification, PlanPromptResolution::Custom(text)) => {
                    let text = text.trim();
                    if text.is_empty() || text.chars().count() > 2_000 {
                        return Err(PlanPromptResolveError::InvalidCustom);
                    }
                    PlanPromptAnswer {
                        option_index: None,
                        text: text.to_owned(),
                        custom: true,
                        consent_state: None,
                    }
                }
                (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::ApproveExecution) => {
                    PlanPromptAnswer {
                        option_index: None,
                        text: "Approved".to_owned(),
                        custom: false,
                        consent_state: Some(ConsentState::Approved),
                    }
                }
                (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::DenyExecution) => {
                    PlanPromptAnswer {
                        option_index: None,
                        text: "Denied".to_owned(),
                        custom: false,
                        consent_state: Some(ConsentState::Denied),
                    }
                }
                (PlanQuestionKind::ExecutionConsent, PlanPromptResolution::Cancel) => {
                    PlanPromptAnswer {
                        option_index: None,
                        text: "Cancelled".to_owned(),
                        custom: false,
                        consent_state: Some(ConsentState::Cancelled),
                    }
                }
                _ => return Err(PlanPromptResolveError::InvalidResolution),
            }
        };
        let entry = pending.remove(id).ok_or(PlanPromptResolveError::NotFound)?;
        let view = entry.view.clone();
        entry
            .sender
            .send(answer)
            .map_err(|_| PlanPromptResolveError::ReceiverGone)?;
        Ok(view)
    }

    pub(crate) fn remove(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    pub(crate) fn expire(
        &self,
        id: &str,
        now_ms: i64,
    ) -> Result<PlanPromptView, PlanPromptResolveError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?;
        let entry = pending.get(id).ok_or(PlanPromptResolveError::NotFound)?;
        if now_ms < entry.view.deadline_at_ms {
            return Err(PlanPromptResolveError::InvalidResolution);
        }
        pending
            .remove(id)
            .map(|entry| entry.view)
            .ok_or(PlanPromptResolveError::NotFound)
    }
}

impl RuntimeHost {
    #[allow(dead_code)] // Backward-compatible clarification entry point for non-MCP callers.
    pub(super) async fn ask_plan_question(
        &self,
        context: &OperationContext,
        question: String,
        options: [String; 2],
    ) -> RuntimeResult<serde_json::Value> {
        self.ask_plan_question_with_kind(
            context,
            question,
            options,
            PlanQuestionKind::Clarification,
        )
        .await
    }

    pub(super) async fn ask_plan_question_with_kind(
        &self,
        context: &OperationContext,
        question: String,
        options: [String; 2],
        question_kind: PlanQuestionKind,
    ) -> RuntimeResult<serde_json::Value> {
        let question = question.trim();
        if question.is_empty() || question.chars().count() > 2_000 {
            return Err(RuntimeError::new(
                "invalid_arguments",
                "question must contain 1..=2000 characters",
            ));
        }
        let options = options.map(|option| option.trim().to_owned());
        if options
            .iter()
            .any(|option| option.is_empty() || option.chars().count() > 500)
            || options[0] == options[1]
        {
            return Err(RuntimeError::new(
                "invalid_arguments",
                "options must contain exactly two distinct non-empty values up to 500 characters each",
            ));
        }
        let task_id = context
            .task_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| RuntimeError::new("invalid_arguments", "taskId is required"))?
            .to_owned();
        let turn_id = context
            .turn_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| RuntimeError::new("invalid_arguments", "turnId is required"))?
            .to_owned();
        let (view, receiver, mut guard) = self
            .plan_prompts
            .register(
                task_id.clone(),
                turn_id.clone(),
                context.agent_id.clone(),
                question.to_owned(),
                options,
                question_kind,
                now_ms(),
                PLAN_QUESTION_TIMEOUT_MS,
            )
            .map_err(|_| {
                RuntimeError::new(
                    "plan_question_unavailable",
                    "plan question registry is unavailable",
                )
            })?;
        persist_pending_plan_prompt(self.repository.pool(), &view).await?;
        guard.persisted = Some((self.repository.pool().clone(), view.clone()));
        self.publish_event(
            format!("{}-pending", view.id),
            "plan.question_pending",
            Some(task_id.clone()),
            None,
            Some(turn_id.clone()),
            json!({
                "questionId": view.id,
                "question": view.question,
                "options": view.options,
                "questionKind": view.question_kind,
                "scopeDigest": view.scope_digest,
                "createdAtMs": view.created_at_ms,
                "deadlineAtMs": view.deadline_at_ms,
            }),
        );

        match tokio::time::timeout(
            Duration::from_millis(PLAN_QUESTION_TIMEOUT_MS as u64),
            receiver,
        )
        .await
        {
            Ok(Ok(answer)) => {
                let consent_state = answer.consent_state;
                self.publish_event(
                    format!("{}-resolved", view.id),
                    "plan.question_resolved",
                    Some(task_id),
                    None,
                    Some(turn_id),
                    json!({
                        "questionId": view.id,
                        "resolution": consent_state.map_or("answered", |state| match state {
                            ConsentState::Approved => "approved",
                            ConsentState::Denied => "denied",
                            ConsentState::Expired => "expired",
                            ConsentState::Cancelled => "cancelled",
                        })
                    }),
                );
                let progress_message = format!(
                    "Câu hỏi lập kế hoạch: {}\nTrả lời: {}",
                    view.question, answer.text
                );
                Ok(json!({
                    "questionId": view.id,
                    "timedOut": false,
                    "questionKind": view.question_kind,
                    "consentState": consent_state,
                    "executionAuthorized": consent_state == Some(ConsentState::Approved),
                    "answer": {
                        "kind": if answer.custom { "custom" } else { "option" },
                        "optionIndex": answer.option_index,
                        "text": answer.text,
                    },
                    "agentProgressMessage": progress_message,
                    "mustCallAgentProgressBeforeContinuing": true,
                }))
            }
            Ok(Err(_)) => Err(RuntimeError::new(
                "plan_question_unavailable",
                "plan question response channel closed before an answer was received",
            )),
            Err(_) => {
                persist_plan_prompt_timeout(self.repository.pool(), &view, view.deadline_at_ms)
                    .await
                    .map_err(|_| {
                        RuntimeError::new(
                            "plan_question_unavailable",
                            "plan question terminal state could not be persisted",
                        )
                    })?;
                let _ = self.plan_prompts.expire(&view.id, view.deadline_at_ms);
                self.publish_event(
                    format!("{}-timeout", view.id),
                    "plan.question_resolved",
                    Some(task_id),
                    None,
                    Some(turn_id),
                    json!({ "questionId": view.id, "resolution": "timeout" }),
                );
                if question_kind == PlanQuestionKind::ExecutionConsent {
                    Ok(json!({
                        "questionId": view.id,
                        "timedOut": true,
                        "questionKind": view.question_kind,
                        "consentState": ConsentState::Expired,
                        "executionAuthorized": false,
                        "mustChooseOneOption": false,
                        "mustCallAgentProgressBeforeContinuing": true,
                    }))
                } else {
                    Ok(json!({
                        "questionId": view.id,
                        "timedOut": true,
                        "questionKind": view.question_kind,
                        "question": view.question,
                        "options": view.options,
                        "mustChooseOneOption": true,
                        "mustCallAgentProgressBeforeContinuing": true,
                    }))
                }
            }
        }
    }
}

impl Drop for PlanPromptGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
        let Some((pool, view)) = self.persisted.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(cancel_abandoned_plan_prompt(pool, view, now_ms()));
        }
    }
}

#[cfg(test)]
#[path = "plan_prompt_tests.rs"]
mod tests;
