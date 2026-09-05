use std::path::{Path, PathBuf};

use chatcmd_runtime::{CommandRunRequest, OperationContext, RuntimeError, RuntimeResult};
use serde_json::Value;

use super::super::{RuntimeHost, parse, value};

impl RuntimeHost {
    pub(super) async fn dispatch_command_run(
        &self,
        context: &OperationContext,
        arguments: Value,
        project_folder: Option<&Path>,
        task_path_scopes: &[PathBuf],
    ) -> RuntimeResult<Value> {
        let mut input: CommandRunRequest = parse(arguments)?;
        if input.cwd.is_relative() {
            input.cwd = project_folder
                .map(|folder| folder.join(&input.cwd))
                .ok_or_else(project_folder_required)?;
        }
        let workspace = self.workspace.with_additional_scopes(task_path_scopes)?;
        let command = self.command.with_workspace(workspace);
        value(command.run(context, input).await?)
    }
}

fn project_folder_required() -> RuntimeError {
    RuntimeError::new(
        "project_folder_required",
        "relative command cwd requires the task project folder; otherwise provide an absolute cwd",
    )
}
