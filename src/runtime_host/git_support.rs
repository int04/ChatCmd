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
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, &context.agent_id).await?
        {
            return Ok(project_folder.into());
        }
        self.workspace.roots().first().cloned().ok_or_else(|| {
            RuntimeError::new(
                "workspace_not_configured",
                "git cwd was omitted and no project folder or workspace root is configured",
            )
        })
    }
}
