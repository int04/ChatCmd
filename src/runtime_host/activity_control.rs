use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chatcmd_runtime::OperationContext;
use serde_json::Value;

#[derive(Clone, Default)]
pub(crate) struct ActivityRegistry {
    active: Arc<Mutex<HashMap<String, ActiveActivity>>>,
}

#[derive(Clone)]
pub(crate) struct ActiveActivity {
    pub context: OperationContext,
    pub tool: String,
    pub shell_session_id: Option<String>,
    stop_reason: Arc<Mutex<Option<String>>>,
}

pub(crate) struct ActivityGuard {
    registry: ActivityRegistry,
    activity_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopActivityResult {
    Stopped,
    OwnershipMismatch,
    NotRunning,
}

impl ActivityRegistry {
    pub(crate) fn register(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> Option<ActivityGuard> {
        if !is_stoppable_tool(tool) {
            return None;
        }
        let activity_id = context.request_id.clone();
        let activity = ActiveActivity {
            context: context.clone(),
            tool: tool.to_owned(),
            shell_session_id: arguments
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            stop_reason: Arc::new(Mutex::new(None)),
        };
        self.active
            .lock()
            .ok()?
            .insert(activity_id.clone(), activity);
        Some(ActivityGuard {
            registry: self.clone(),
            activity_id,
        })
    }

    pub(crate) fn prepare_stop(
        &self,
        task_id: &str,
        activity_id: &str,
        turn_id: Option<&str>,
        reason: Option<&str>,
    ) -> (StopActivityResult, Option<ActiveActivity>) {
        let Ok(active) = self.active.lock() else {
            return (StopActivityResult::NotRunning, None);
        };
        let Some(activity) = active.get(activity_id).cloned() else {
            return (StopActivityResult::NotRunning, None);
        };
        if activity.context.task_id.as_deref() != Some(task_id)
            || turn_id.is_some_and(|expected| activity.context.turn_id.as_deref() != Some(expected))
        {
            return (StopActivityResult::OwnershipMismatch, None);
        }
        if let Ok(mut value) = activity.stop_reason.lock() {
            *value = reason
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }
        (StopActivityResult::Stopped, Some(activity))
    }

    pub(crate) fn stop_reason(&self, activity_id: &str) -> Option<String> {
        let active = self.active.lock().ok()?;
        active.get(activity_id)?.stop_reason.lock().ok()?.clone()
    }

    pub(crate) fn is_shell_busy(&self, session_id: &str) -> bool {
        self.active.lock().is_ok_and(|active| {
            active
                .values()
                .any(|activity| activity.shell_session_id.as_deref() == Some(session_id))
        })
    }

    fn remove(&self, activity_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(activity_id);
        }
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.activity_id);
    }
}

fn is_stoppable_tool(tool: &str) -> bool {
    !matches!(
        tool,
        "agent_user_message"
            | "agent_progress"
            | "agent_turn_complete"
            | "device_list"
            | "device_get"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stop_requires_matching_task_and_turn_and_keeps_reason() {
        let registry = ActivityRegistry::default();
        let mut context = OperationContext::new("activity-1", "agent", "git_status");
        context.task_id = Some("task-1".to_owned());
        context.turn_id = Some("turn-1".to_owned());
        let _guard = registry
            .register(&context, "git_status", &json!({"cwd":"."}))
            .unwrap();

        assert_eq!(
            registry
                .prepare_stop("task-2", "activity-1", Some("turn-1"), None)
                .0,
            StopActivityResult::OwnershipMismatch
        );
        assert!(!context.cancellation.is_cancelled());

        assert_eq!(
            registry
                .prepare_stop(
                    "task-1",
                    "activity-1",
                    Some("turn-1"),
                    Some(" user requested stop ")
                )
                .0,
            StopActivityResult::Stopped
        );
        assert!(!context.cancellation.is_cancelled());
        assert_eq!(
            registry.stop_reason("activity-1").as_deref(),
            Some("user requested stop")
        );
        context.cancellation.cancel();
        assert!(context.cancellation.is_cancelled());
    }
}
