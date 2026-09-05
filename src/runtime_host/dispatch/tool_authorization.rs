use chatcmd_core::{McpAgentStore as _, ToolCatalogStore as _};
use chatcmd_runtime::{RuntimeError, RuntimeResult};

use super::super::{RuntimeHost, invalid, storage_error};

impl RuntimeHost {
    pub(in crate::runtime_host) async fn authorize_tool(
        &self,
        agent_id: &str,
        tool: &str,
    ) -> RuntimeResult<()> {
        let id = chatcmd_core::AgentId::new(agent_id)
            .map_err(|_| invalid("agentId", "must be a non-empty string"))?;
        self.repository
            .agent(&id)
            .await
            .map_err(storage_error)?
            .filter(|agent| agent.enabled)
            .ok_or_else(|| RuntimeError::new("unauthorized", "agent is disabled or missing"))?;
        if matches!(
            tool,
            "agent_user_message"
                | "agent_progress"
                | "agent_plan_question"
                | "agent_subagent_start"
                | "agent_subagent_wait"
                | "agent_turn_complete"
        ) {
            return Ok(());
        }
        let allowed = self
            .repository
            .agent_allowed_tool_ids(&id)
            .await
            .map_err(storage_error)?;
        let tools = self.repository.list_tools().await.map_err(storage_error)?;
        if tools.iter().any(|candidate| {
            candidate.key == tool && candidate.enabled && allowed.contains(&candidate.id)
        }) {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "policy_denied",
                "agent tool allowlist denied this operation",
            ))
        }
    }
}
