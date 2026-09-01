use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde::Serialize;
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

const PLAN_QUESTION_TIMEOUT_MS: i64 = 120_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanPromptView {
    pub(crate) id: String,
    pub(crate) task_id: String,
    pub(crate) turn_id: String,
    pub(crate) question: String,
    pub(crate) options: [String; 2],
    pub(crate) created_at_ms: i64,
    pub(crate) deadline_at_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanPromptAnswer {
    pub(crate) option_index: Option<u8>,
    pub(crate) text: String,
    pub(crate) custom: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum PlanPromptResolution {
    Option(u8),
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanPromptResolveError {
    Unavailable,
    NotFound,
    InvalidOption,
    InvalidCustom,
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
}

impl PlanPromptRegistry {
    pub(crate) fn register(
        &self,
        task_id: String,
        turn_id: String,
        question: String,
        options: [String; 2],
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
        let view = PlanPromptView {
            id: format!("plan-question-{}", Uuid::new_v4()),
            task_id,
            turn_id,
            question,
            options,
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

    pub(crate) fn resolve(
        &self,
        id: &str,
        resolution: PlanPromptResolution,
    ) -> Result<PlanPromptView, PlanPromptResolveError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PlanPromptResolveError::Unavailable)?;
        let answer = {
            let entry = pending.get(id).ok_or(PlanPromptResolveError::NotFound)?;
            match resolution {
                PlanPromptResolution::Option(index @ 1..=2) => PlanPromptAnswer {
                    option_index: Some(index),
                    text: entry.view.options[usize::from(index - 1)].clone(),
                    custom: false,
                },
                PlanPromptResolution::Option(_) => {
                    return Err(PlanPromptResolveError::InvalidOption);
                }
                PlanPromptResolution::Custom(text) => {
                    let text = text.trim();
                    if text.is_empty() || text.chars().count() > 2_000 {
                        return Err(PlanPromptResolveError::InvalidCustom);
                    }
                    PlanPromptAnswer {
                        option_index: None,
                        text: text.to_owned(),
                        custom: true,
                    }
                }
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
}

impl RuntimeHost {
    pub(super) async fn ask_plan_question(
        &self,
        context: &OperationContext,
        question: String,
        options: [String; 2],
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
        let (view, receiver, _guard) = self
            .plan_prompts
            .register(
                task_id.clone(),
                turn_id.clone(),
                question.to_owned(),
                options,
                now_ms(),
                PLAN_QUESTION_TIMEOUT_MS,
            )
            .map_err(|_| {
                RuntimeError::new(
                    "plan_question_unavailable",
                    "plan question registry is unavailable",
                )
            })?;
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
                self.publish_event(
                    format!("{}-resolved", view.id),
                    "plan.question_resolved",
                    Some(task_id),
                    None,
                    Some(turn_id),
                    json!({ "questionId": view.id, "resolution": "answered" }),
                );
                let progress_message = format!(
                    "Câu hỏi lập kế hoạch: {}\nTrả lời: {}",
                    view.question, answer.text
                );
                Ok(json!({
                    "questionId": view.id,
                    "timedOut": false,
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
                self.plan_prompts.remove(&view.id);
                self.publish_event(
                    format!("{}-timeout", view.id),
                    "plan.question_resolved",
                    Some(task_id),
                    None,
                    Some(turn_id),
                    json!({ "questionId": view.id, "resolution": "timeout" }),
                );
                Ok(json!({
                    "questionId": view.id,
                    "timedOut": true,
                    "question": view.question,
                    "options": view.options,
                    "mustChooseOneOption": true,
                    "mustCallAgentProgressBeforeContinuing": true,
                }))
            }
        }
    }
}

impl Drop for PlanPromptGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_option_using_server_owned_option_text() {
        let registry = PlanPromptRegistry::default();
        let (view, receiver, _guard) = registry
            .register(
                "task-1".to_owned(),
                "turn-1".to_owned(),
                "Choose".to_owned(),
                ["PHP".to_owned(), "C#".to_owned()],
                10,
                120_000,
            )
            .expect("register prompt");

        registry
            .resolve(&view.id, PlanPromptResolution::Option(2))
            .expect("resolve option");
        let answer = receiver.await.expect("receive answer");
        assert_eq!(answer.option_index, Some(2));
        assert_eq!(answer.text, "C#");
        assert!(!answer.custom);
        assert!(registry.pending().expect("pending prompts").is_empty());
    }

    #[test]
    fn pending_prompts_are_fifo_and_invalid_answer_keeps_prompt() {
        let registry = PlanPromptRegistry::default();
        let (second, _receiver, _guard) = registry
            .register(
                "task-2".to_owned(),
                "turn-2".to_owned(),
                "Second".to_owned(),
                ["A".to_owned(), "B".to_owned()],
                20,
                120_000,
            )
            .expect("register second prompt");
        let (first, _receiver, _guard) = registry
            .register(
                "task-1".to_owned(),
                "turn-1".to_owned(),
                "First".to_owned(),
                ["A".to_owned(), "B".to_owned()],
                10,
                120_000,
            )
            .expect("register first prompt");

        assert!(matches!(
            registry.resolve(&first.id, PlanPromptResolution::Option(3)),
            Err(PlanPromptResolveError::InvalidOption)
        ));
        let pending = registry.pending().expect("pending prompts");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].id, first.id);
        assert_eq!(pending[1].id, second.id);
    }
}
