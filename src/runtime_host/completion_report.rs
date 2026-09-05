//! Server-normalized completion quality reports backed by persisted tool events.

use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, TaskId, TerminalEventStore as _, TimelineEvent,
    TurnId,
};
use chatcmd_runtime::OperationContext;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row as _;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

const MAX_ITEMS: usize = 64;
const MAX_TEXT_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum WorkOutcome {
    #[default]
    Completed,
    Partial,
    Blocked,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum VerificationIntent {
    NotRun,
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CompletionCriterionInput {
    pub(super) criterion: String,
    #[serde(default)]
    pub(super) evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CompleteInput {
    pub(super) content: String,
    #[serde(default)]
    pub(super) suggested_title: Option<String>,
    #[serde(default)]
    pub(super) work_outcome: Option<WorkOutcome>,
    #[serde(default)]
    pub(super) verification_intent: Option<VerificationIntent>,
    #[serde(default)]
    pub(super) verification_reason: Option<String>,
    #[serde(default)]
    pub(super) verification_scope: Option<String>,
    #[serde(default)]
    pub(super) criteria: Vec<CompletionCriterionInput>,
    #[serde(default)]
    pub(super) evidence_refs: Vec<String>,
    #[serde(default)]
    pub(super) blockers: Vec<String>,
    #[serde(default)]
    pub(super) limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum VerificationState {
    Passed,
    Failed,
    NotRun,
    NotApplicable,
    Stale,
    Unknown,
}

struct EvidenceOwner {
    task_id: String,
    turn_id: String,
    agent_id: String,
    event_time: i64,
    delegated_child: bool,
}

impl RuntimeHost {
    pub(super) async fn normalize_completion_report(
        &self,
        context: &OperationContext,
        input: &CompleteInput,
    ) -> Value {
        let turn_id = context.turn_id.as_deref().unwrap_or_default();
        let refs = requested_refs(input);
        let (records, diagnostics) = match self.resolve_evidence(context, turn_id, &refs).await {
            Ok(value) => value,
            Err(code) => (
                Vec::new(),
                vec![
                    json!({"code": code, "message": "Evidence could not be resolved; finalization continued without a verified claim."}),
                ],
            ),
        };
        let verification = aggregate_verification(input, &refs, &records, &diagnostics);
        let criteria = normalized_criteria(input, &records);
        json!({
            "schemaVersion": 1,
            "lifecycle": "completed",
            "workOutcome": input.work_outcome.unwrap_or_default(),
            "workOutcomeProvenance": if input.work_outcome.is_some() { "agentDeclared" } else { "legacyDefault" },
            "verification": verification,
            "verificationProvenance": "serverEvidenceResolverV1",
            "verificationReason": bounded_optional(input.verification_reason.as_deref()),
            "verificationScope": bounded_optional(input.verification_scope.as_deref()),
            "criteria": criteria,
            "evidence": records,
            "diagnostics": diagnostics,
            "blockers": bounded_list(&input.blockers),
            "limitations": bounded_list(&input.limitations),
        })
    }

    async fn resolve_evidence(
        &self,
        context: &OperationContext,
        turn_id: &str,
        refs: &[String],
    ) -> Result<(Vec<Value>, Vec<Value>), &'static str> {
        let task_id = context.task_id.as_deref().unwrap_or_default();
        let rows = sqlx::query(
            "SELECT turn_id,payload_json,created_at_ms FROM timeline_events WHERE task_id=? AND kind='tool_result' AND json_extract(payload_json,'$.tool')='command_run' ORDER BY created_at_ms,event_id",
        )
        .bind(task_id)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| "evidenceStoreUnavailable")?;
        let mut by_id = BTreeMap::<String, EvidenceOwner>::new();
        for row in rows {
            let payload = serde_json::from_str::<Value>(&row.get::<String, _>("payload_json"))
                .unwrap_or(Value::Null);
            let output = payload.get("output").cloned().unwrap_or(Value::Null);
            if let Some(id) = output.get("executionId").and_then(Value::as_str) {
                by_id.insert(
                    id.to_owned(),
                    EvidenceOwner {
                        task_id: task_id.to_owned(),
                        turn_id: row.get::<Option<String>, _>("turn_id").unwrap_or_default(),
                        agent_id: context.agent_id.clone(),
                        event_time: row.get("created_at_ms"),
                        delegated_child: false,
                    },
                );
            }
        }
        let child_rows = sqlx::query(
            "SELECT e.turn_id,e.payload_json,e.created_at_ms,r.child_task_id,t.agent_id FROM subagent_runs r JOIN tasks t ON t.id=r.child_task_id JOIN timeline_events e ON e.task_id=r.child_task_id WHERE r.parent_task_id=? AND r.parent_turn_id=? AND r.child_task_id IS NOT NULL AND e.kind='tool_result' AND json_extract(e.payload_json,'$.tool')='command_run' ORDER BY e.created_at_ms,e.event_id",
        )
        .bind(task_id).bind(turn_id).fetch_all(self.repository.pool()).await
        .map_err(|_| "evidenceStoreUnavailable")?;
        for row in child_rows {
            let payload = serde_json::from_str::<Value>(&row.get::<String, _>("payload_json"))
                .unwrap_or(Value::Null);
            if let Some(id) = payload
                .get("output")
                .and_then(|value| value.get("executionId"))
                .and_then(Value::as_str)
            {
                by_id.entry(id.to_owned()).or_insert_with(|| EvidenceOwner {
                    task_id: row.get("child_task_id"),
                    turn_id: row.get::<Option<String>, _>("turn_id").unwrap_or_default(),
                    agent_id: row.get("agent_id"),
                    event_time: row.get("created_at_ms"),
                    delegated_child: true,
                });
            }
        }
        let mut records = Vec::with_capacity(refs.len());
        let mut diagnostics = Vec::new();
        let mut current_states =
            BTreeMap::<std::path::PathBuf, chatcmd_runtime::CommandSourceState>::new();
        for reference in refs {
            let Some(owner) = by_id.get(reference) else {
                diagnostics.push(json!({"code": "evidenceNotFound", "evidenceRef": reference}));
                continue;
            };
            let mut owner_context = context.clone();
            owner_context.task_id = Some(owner.task_id.clone());
            owner_context.turn_id = Some(owner.turn_id.clone());
            owner_context.agent_id.clone_from(&owner.agent_id);
            let execution = match self.command.result(&owner_context, reference) {
                Ok(value) => {
                    let current_source = if let Some(state) = current_states.get(&value.cwd) {
                        state.clone()
                    } else {
                        let state =
                            chatcmd_runtime::capture_command_source_state(value.cwd.clone()).await;
                        current_states.insert(value.cwd.clone(), state.clone());
                        state
                    };
                    match serde_json::to_value(value) {
                        Ok(mut value) => {
                            value["sourceStateCurrent"] = json!(current_source);
                            value
                        }
                        Err(_) => {
                            diagnostics.push(json!({"code": "evidenceSerializationFailed", "evidenceRef": reference}));
                            continue;
                        }
                    }
                }
                Err(error) => {
                    diagnostics.push(json!({"code": error.code, "evidenceRef": reference}));
                    continue;
                }
            };
            let output = &execution;
            let integration_current = owner.delegated_child || owner.turn_id == turn_id;
            let status = evidence_status(output, integration_current);
            let reason = evidence_reason(output, integration_current);
            let finished_at = output
                .get("finishedAtUnixMs")
                .cloned()
                .unwrap_or_else(|| json!(owner.event_time));
            records.push(json!({
                "executionId": reference,
                "taskId": owner.task_id,
                "turnId": owner.turn_id,
                "delegatedChild": owner.delegated_child,
                "cwd": output.get("cwd"),
                "command": output.get("command"),
                "startedAtMs": output.get("startedAtUnixMs"),
                "finishedAtMs": finished_at,
                "terminalState": output.get("terminalState"),
                "exitCode": output.get("exitCode"),
                "timedOut": output.get("timedOut"),
                "cancelled": output.get("cancelled"),
                "artifactRef": output.get("artifactRef"),
                "sourceStateBefore": output.get("sourceStateBefore"),
                "sourceStateAfter": output.get("sourceStateAfter"),
                "sourceStateCurrent": output.get("sourceStateCurrent"),
                "status": status,
                "reason": reason,
                "parser": {"name": "commandTerminal", "version": 1}
            }));
        }
        Ok((records, diagnostics))
    }

    pub(super) async fn persist_completion_report(
        &self,
        context: &OperationContext,
        report: &Value,
    ) {
        let Some((task, turn, session)) = completion_ids(context) else {
            return;
        };
        let key = format!(
            "completion-quality-{}",
            Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                format!("{}\0{}", context.request_id, turn.as_str()).as_bytes(),
            )
        );
        let payload = json!({"status": "quality", "qualityReport": report});
        let event = TimelineEvent {
            id: match EventId::new(&key) {
                Ok(value) => value,
                Err(_) => return,
            },
            task_id: task,
            turn_id: Some(turn),
            session_id: Some(session),
            actor: ActorKind::Assistant,
            kind: EventKind::Status,
            idempotency_key: key.clone(),
            payload_json: payload.to_string(),
            metadata_json: None,
            created_at_ms: now_ms(),
        };
        if let Err(error) = self.repository.append_timeline_events(&[event]).await {
            tracing::warn!(error = ?error, "completion quality report could not be persisted; finalization continues");
            return;
        }
        self.publish_event(
            key,
            EventKind::Status.as_str(),
            context.task_id.clone(),
            context.mcp_session_id.clone(),
            context.turn_id.clone(),
            payload,
        );
    }
}

fn requested_refs(input: &CompleteInput) -> Vec<String> {
    input
        .evidence_refs
        .iter()
        .chain(input.criteria.iter().flat_map(|item| &item.evidence_refs))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .take(MAX_ITEMS)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn evidence_status(output: &Value, current_turn: bool) -> VerificationState {
    if !current_turn || source_state_changed(output) {
        return VerificationState::Stale;
    }
    if output.get("terminalState").and_then(Value::as_str) == Some("unknown") {
        return VerificationState::Unknown;
    }
    if output.get("timedOut").and_then(Value::as_bool) == Some(true)
        || output.get("cancelled").and_then(Value::as_bool) == Some(true)
        || output.get("exitCode").and_then(Value::as_i64) != Some(0)
    {
        return VerificationState::Failed;
    }
    if has_fresh_source_state(output) {
        VerificationState::Passed
    } else {
        VerificationState::Unknown
    }
}

fn evidence_reason(output: &Value, current_turn: bool) -> &'static str {
    if !current_turn {
        "evidence belongs to an earlier turn; integration freshness is unproven"
    } else if source_state_changed(output) {
        "source state changed while the command ran or after it completed"
    } else if output.get("terminalState").and_then(Value::as_str) == Some("unknown") {
        "command terminal state is unknown after recovery"
    } else if output.get("timedOut").and_then(Value::as_bool) == Some(true) {
        "command timed out"
    } else if output.get("cancelled").and_then(Value::as_bool) == Some(true) {
        "command was cancelled"
    } else if output.get("exitCode").and_then(Value::as_i64) != Some(0) {
        "command did not exit successfully"
    } else if !has_fresh_source_state(output) {
        "command passed but source-state freshness was not captured"
    } else {
        "server-owned command evidence passed"
    }
}

fn has_fresh_source_state(output: &Value) -> bool {
    let before = output.get("sourceStateBefore");
    let after = output.get("sourceStateAfter");
    let current = output.get("sourceStateCurrent");
    before.is_some_and(|value| !value.is_null())
        && after.is_some_and(|value| !value.is_null())
        && current.is_some_and(|value| !value.is_null())
        && before == after
        && after == current
        && before
            .and_then(|value| value.get("complete"))
            .and_then(Value::as_bool)
            == Some(true)
}

fn source_state_changed(output: &Value) -> bool {
    let before = output.get("sourceStateBefore");
    let after = output.get("sourceStateAfter");
    let current = output.get("sourceStateCurrent");
    (before.is_some_and(|value| !value.is_null())
        && after.is_some_and(|value| !value.is_null())
        && before != after)
        || (after.is_some_and(|value| !value.is_null())
            && current.is_some_and(|value| !value.is_null())
            && after != current)
}

fn aggregate_verification(
    input: &CompleteInput,
    refs: &[String],
    records: &[Value],
    diagnostics: &[Value],
) -> VerificationState {
    if refs.is_empty() {
        return match input.verification_intent {
            Some(VerificationIntent::NotApplicable)
                if input
                    .verification_reason
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty()) =>
            {
                VerificationState::NotApplicable
            }
            Some(VerificationIntent::NotApplicable) => VerificationState::Unknown,
            _ => VerificationState::NotRun,
        };
    }
    if records
        .iter()
        .any(|value| value.get("status") == Some(&json!("failed")))
    {
        VerificationState::Failed
    } else if records
        .iter()
        .any(|value| value.get("status") == Some(&json!("stale")))
    {
        VerificationState::Stale
    } else if !diagnostics.is_empty()
        || records
            .iter()
            .any(|value| value.get("status") == Some(&json!("unknown")))
        || input
            .verification_scope
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        || input.criteria.is_empty()
        || input
            .criteria
            .iter()
            .any(|item| item.evidence_refs.is_empty())
    {
        VerificationState::Unknown
    } else {
        VerificationState::Passed
    }
}

fn normalized_criteria(input: &CompleteInput, records: &[Value]) -> Vec<Value> {
    let known = records
        .iter()
        .filter_map(|value| value.get("executionId").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    input
        .criteria
        .iter()
        .take(MAX_ITEMS)
        .map(|item| {
            let refs = item
                .evidence_refs
                .iter()
                .take(MAX_ITEMS)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            let covered =
                !refs.is_empty() && refs.iter().all(|reference| known.contains(reference));
            json!({"criterion": bounded(&item.criterion), "evidenceRefs": refs, "covered": covered})
        })
        .collect()
}

fn bounded_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .take(MAX_ITEMS)
        .map(|value| bounded(value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn bounded_optional(value: Option<&str>) -> Option<String> {
    value.map(bounded).filter(|value| !value.is_empty())
}

fn bounded(value: &str) -> String {
    value.trim().chars().take(MAX_TEXT_CHARS).collect()
}

fn completion_ids(context: &OperationContext) -> Option<(TaskId, TurnId, SessionId)> {
    Some((
        TaskId::new(context.task_id.as_deref()?).ok()?,
        TurnId::new(context.turn_id.as_deref()?).ok()?,
        SessionId::new(context.mcp_session_id.as_deref()?).ok()?,
    ))
}

#[cfg(test)]
#[path = "completion_report_tests.rs"]
mod tests;
