use std::time::Duration;

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult, ShellWaitResult, ToolUsage};

use super::super::RuntimeHost;

impl RuntimeHost {
    pub(super) async fn wait_for_shell_handoff(
        &self,
        context: &OperationContext,
        session_id: &str,
        timeout: Duration,
        allow_user_input: bool,
    ) -> RuntimeResult<(ShellWaitResult, ToolUsage)> {
        if !allow_user_input {
            return self
                .shell
                .wait_with_context(context, session_id, timeout)
                .await;
        }
        let notifier = self
            .activities
            .shell_user_input_notifier(&context.request_id)
            .ok_or_else(|| {
                RuntimeError::new(
                    "shell_input_handoff_unavailable",
                    "terminal input handoff is not active for this shell wait",
                )
            })?;
        tokio::select! {
            result = self.shell.wait_with_context(context, session_id, timeout) => result,
            () = notifier.notified() => {
                let (mut result, usage) = self
                    .shell
                    .wait_with_context(context, session_id, Duration::from_millis(1))
                    .await?;
                if !result.completed {
                    result.wait_timed_out = false;
                }
                Ok((result, usage))
            }
        }
    }
}
