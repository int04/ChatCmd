use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::Value;

use super::{RuntimeHost, inputs::CompleteInput, parse};

impl RuntimeHost {
    pub(super) async fn complete_agent_turn(
        &self,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let input: CompleteInput = parse(arguments)?;
        if let (Some(task_id), Some(turn_id)) =
            (context.task_id.as_deref(), context.turn_id.as_deref())
            && self.activities.has_active_turn(task_id, turn_id)
        {
            crate::log_helper::log_issue(
                file!(),
                line!(),
                &format!(
                    "active_tools_running: agent_turn_complete rejected; taskId={task_id}; turnId={turn_id}"
                ),
            );
            return Err(RuntimeError::new(
                "active_tools_running",
                "one or more tools are still running in this turn; wait for them to finish before completing the turn",
            ));
        }
        self.ensure_subagents_finished(context).await?;
        self.save_agent_event(
            context,
            "completed",
            &input.content,
            input.suggested_title.as_deref(),
        )
        .await
    }
}
