use chatcmd_runtime::{
    ComputerActRequest, ComputerSessionStartRequest, OperationContext, RuntimeError, RuntimeResult,
};
use serde_json::{Value, json};

use super::super::{RuntimeHost, inputs::SessionInput, parse, value};

pub(super) fn is_computer_tool(tool: &str) -> bool {
    matches!(
        tool,
        "computer_session_start" | "computer_observe" | "computer_act" | "computer_session_close"
    )
}

impl RuntimeHost {
    pub(super) async fn dispatch_computer_tool(
        &self,
        tool: &str,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        match tool {
            "computer_session_start" => value(
                self.computer
                    .start(context, parse::<ComputerSessionStartRequest>(arguments)?)
                    .await?,
            ),
            "computer_observe" => {
                let input: SessionInput = parse(arguments)?;
                value(self.computer.observe(context, &input.session_id).await?)
            }
            "computer_act" => value(
                self.computer
                    .act(context, parse::<ComputerActRequest>(arguments)?)
                    .await?,
            ),
            "computer_session_close" => {
                let input: SessionInput = parse(arguments)?;
                self.computer.close(context, &input.session_id).await?;
                Ok(json!({ "closed": true }))
            }
            _ => Err(RuntimeError::new("tool_not_found", "unknown computer tool")),
        }
    }
}
