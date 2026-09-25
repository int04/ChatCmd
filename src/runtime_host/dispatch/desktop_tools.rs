use chatcmd_runtime::{
    DesktopElementActRequest, DesktopInputActRequest, DesktopInputBeginRequest,
    DesktopInputEndRequest, DesktopObserveRequest, OperationContext, RuntimeError, RuntimeResult,
};
use serde_json::{Value, json};

use super::super::{RuntimeHost, parse, value};

pub(super) fn is_desktop_tool(tool: &str) -> bool {
    matches!(
        tool,
        "desktop_window_list"
            | "desktop_window_observe"
            | "desktop_element_act"
            | "desktop_input_begin"
            | "desktop_input_act"
            | "desktop_input_end"
    )
}

impl RuntimeHost {
    pub(super) async fn dispatch_desktop_tool(
        &self,
        tool: &str,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        match tool {
            "desktop_window_list" => value(self.desktop.list_windows(context).await?),
            "desktop_window_observe" => value(
                self.desktop
                    .observe(context, parse::<DesktopObserveRequest>(arguments)?)
                    .await?,
            ),
            "desktop_element_act" => value(
                self.desktop
                    .element_act(context, parse::<DesktopElementActRequest>(arguments)?)
                    .await?,
            ),
            "desktop_input_begin" => value(
                self.desktop
                    .input_begin(context, parse::<DesktopInputBeginRequest>(arguments)?)
                    .await?,
            ),
            "desktop_input_act" => value(
                self.desktop
                    .input_act(context, parse::<DesktopInputActRequest>(arguments)?)
                    .await?,
            ),
            "desktop_input_end" => {
                self.desktop
                    .input_end(context, parse::<DesktopInputEndRequest>(arguments)?)
                    .await?;
                Ok(json!({ "ended": true }))
            }
            _ => Err(RuntimeError::new("tool_not_found", "unknown desktop tool")),
        }
    }
}
