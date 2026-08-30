use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};

use super::RuntimeHost;

impl RuntimeHost {
    pub(super) async fn resolve_git_cwd(
        &self,
        context: &OperationContext,
        explicit: Option<std::path::PathBuf>,
    ) -> RuntimeResult<std::path::PathBuf> {
        if let Some(cwd) = explicit {
            return Ok(cwd);
        }
        if let Some(project_folder) =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await?
        {
            return Ok(project_folder.into());
        }
        Err(RuntimeError::new(
            "project_folder_required",
            "git cwd was omitted; provide the project folder or an explicit absolute working path",
        ))
    }
}
