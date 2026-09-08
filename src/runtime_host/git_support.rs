use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};

use super::RuntimeHost;

impl RuntimeHost {
    pub(super) async fn resolve_git_cwd(
        &self,
        context: &OperationContext,
        explicit: Option<std::path::PathBuf>,
    ) -> RuntimeResult<std::path::PathBuf> {
        if explicit.as_ref().is_some_and(|cwd| cwd.is_absolute()) {
            return Ok(explicit.expect("absolute cwd checked above"));
        }
        let project_folder =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await?
                .map(std::path::PathBuf::from)
                .ok_or_else(|| {
                    RuntimeError::new(
                        "project_folder_required",
                        "git cwd requires the task project folder unless an explicit absolute path is provided",
                    )
                })?;
        Ok(explicit.map_or(project_folder.clone(), |cwd| project_folder.join(cwd)))
    }
}
