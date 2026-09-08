use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chatcmd_runtime::OperationContext;
use serde_json::Value;
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(crate) struct ActivityRegistry {
    active: Arc<Mutex<HashMap<String, ActiveActivity>>>,
    in_flight: Arc<Mutex<HashMap<String, OperationContext>>>,
}

#[derive(Clone)]
pub(crate) struct ActiveActivity {
    pub context: OperationContext,
    pub tool: String,
    pub shell_session_id: Option<String>,
    allow_user_input: bool,
    user_input_notify: Option<Arc<Notify>>,
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

/// Tracks every admitted MCP call through persistence, including lifecycle tools.
pub(crate) struct CompactCallGuard {
    registry: ActivityRegistry,
    key: String,
}

impl Drop for CompactCallGuard {
    fn drop(&mut self) {
        if let Ok(mut calls) = self.registry.in_flight.lock() {
            calls.remove(&self.key);
        }
    }
}

impl ActivityRegistry {
    pub(crate) fn track_compact_call(
        &self,
        context: &OperationContext,
    ) -> Result<CompactCallGuard, chatcmd_runtime::RuntimeError> {
        let key = uuid::Uuid::new_v4().to_string();
        self.in_flight
            .lock()
            .map_err(|_| {
                chatcmd_runtime::RuntimeError::new(
                    "compact_activity_unavailable",
                    "Cannot verify the local tool barrier",
                )
            })?
            .insert(key.clone(), context.clone());
        Ok(CompactCallGuard {
            registry: self.clone(),
            key,
        })
    }

    pub(crate) fn compact_settled(&self, task_id: &str, scope: Option<&str>) -> bool {
        // Poisoned/unavailable is not zero. Calls register BEFORE checking the durable
        // fence, so an already-admitted call cannot hide in the admission/dispatch gap.
        self.in_flight.lock().is_ok_and(|calls| {
            !calls.values().any(|context| {
                context.task_id.as_deref() == Some(task_id)
                    || scope.is_some_and(|scope| {
                        context.conversation_scope_id.as_deref() == Some(scope)
                    })
            })
        })
    }

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
        let allow_user_input = tool == "shell_wait"
            && arguments.get("allowUserInput").and_then(Value::as_bool) == Some(true);
        let activity = ActiveActivity {
            context: context.clone(),
            tool: tool.to_owned(),
            shell_session_id: arguments
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            allow_user_input,
            user_input_notify: allow_user_input.then(|| Arc::new(Notify::new())),
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

    pub(crate) fn is_shell_input_allowed(&self, session_id: &str) -> bool {
        let Ok(active) = self.active.lock() else {
            return false;
        };
        let mut matching = active
            .values()
            .filter(|activity| activity.shell_session_id.as_deref() == Some(session_id))
            .peekable();
        if matching.peek().is_none() {
            return true;
        }
        matching.all(|activity| activity.allow_user_input)
    }

    pub(crate) fn shell_user_input_notifier(&self, activity_id: &str) -> Option<Arc<Notify>> {
        self.active
            .lock()
            .ok()?
            .get(activity_id)?
            .user_input_notify
            .clone()
    }

    pub(crate) fn notify_shell_user_input(&self, session_id: &str) {
        let notifiers = self.active.lock().map_or_else(
            |_| Vec::new(),
            |active| {
                active
                    .values()
                    .filter(|activity| activity.shell_session_id.as_deref() == Some(session_id))
                    .filter_map(|activity| activity.user_input_notify.clone())
                    .collect::<Vec<_>>()
            },
        );
        for notifier in notifiers {
            notifier.notify_one();
        }
    }

    pub(crate) fn has_active_turn(&self, task_id: &str, turn_id: &str) -> bool {
        self.active.lock().is_ok_and(|active| {
            active.values().any(|activity| {
                activity.context.task_id.as_deref() == Some(task_id)
                    && activity.context.turn_id.as_deref() == Some(turn_id)
            })
        })
    }

    pub(crate) fn cancel_task(&self, task_id: &str) {
        let cancellations = self.active.lock().map_or_else(
            |_| Vec::new(),
            |active| {
                active
                    .values()
                    .filter(|activity| activity.context.task_id.as_deref() == Some(task_id))
                    .map(|activity| activity.context.cancellation.clone())
                    .collect::<Vec<_>>()
            },
        );
        for cancellation in cancellations {
            cancellation.cancel();
        }
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

    #[tokio::test]
    async fn shell_input_requires_explicit_wait_handoff() {
        let registry = ActivityRegistry::default();
        let context = OperationContext::new("wait-1", "agent", "shell_wait");
        assert!(registry.is_shell_input_allowed("shell-1"));

        let wait_guard = registry
            .register(
                &context,
                "shell_wait",
                &json!({"sessionId":"shell-1","allowUserInput":true}),
            )
            .unwrap();
        assert!(registry.is_shell_busy("shell-1"));
        assert!(registry.is_shell_input_allowed("shell-1"));
        let notifier = registry.shell_user_input_notifier("wait-1").unwrap();
        registry.notify_shell_user_input("shell-1");
        tokio::time::timeout(std::time::Duration::from_millis(50), notifier.notified())
            .await
            .unwrap();

        let read_context = OperationContext::new("read-1", "agent", "shell_read");
        let read_guard = registry
            .register(&read_context, "shell_read", &json!({"sessionId":"shell-1"}))
            .unwrap();
        assert!(!registry.is_shell_input_allowed("shell-1"));

        drop(read_guard);
        assert!(registry.is_shell_input_allowed("shell-1"));
        drop(wait_guard);
        assert!(!registry.is_shell_busy("shell-1"));
        assert!(registry.is_shell_input_allowed("shell-1"));

        let normal_wait = registry
            .register(&context, "shell_wait", &json!({"sessionId":"shell-1"}))
            .unwrap();
        assert!(!registry.is_shell_input_allowed("shell-1"));
        assert!(registry.shell_user_input_notifier("wait-1").is_none());
        drop(normal_wait);
    }
}
