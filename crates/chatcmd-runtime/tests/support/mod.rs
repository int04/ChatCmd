pub mod fault_injection;
pub mod fixtures;
pub mod process_helper;
pub mod resource_probe;

use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, PolicyDecision, PolicyEngine, RuntimeResult,
    WorkspaceService,
};
use std::{collections::BTreeMap, path::Path, sync::Arc};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

pub fn workspace(root: &Path) -> WorkspaceService {
    WorkspaceService::new(
        &[root.to_path_buf()],
        PolicyEngine::new(
            Some(ExecutionPolicy {
                default: PolicyDecision::Allow,
                per_agent_tool: BTreeMap::new(),
                per_root: BTreeMap::new(),
            }),
            Arc::new(Approve),
        ),
    )
    .expect("create test workspace")
}
