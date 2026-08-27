use crate::{BoxFuture, RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny,
    Approval,
}

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub agent_id: String,
    pub tool_name: String,
    pub root: Option<PathBuf>,
    pub destructive: bool,
}

#[derive(Debug, Clone)]
pub struct ExecutionPolicy {
    pub default: PolicyDecision,
    pub per_agent_tool: BTreeMap<(String, String), PolicyDecision>,
    pub per_root: BTreeMap<PathBuf, PolicyDecision>,
}

impl ExecutionPolicy {
    #[must_use]
    pub fn fail_closed() -> Self {
        Self {
            default: PolicyDecision::Approval,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn decision(&self, context: &PolicyContext) -> PolicyDecision {
        if let Some(value) = self
            .per_agent_tool
            .get(&(context.agent_id.clone(), context.tool_name.clone()))
        {
            return *value;
        }
        if let Some(root) = &context.root {
            for (configured, value) in &self.per_root {
                if root.starts_with(configured) {
                    return *value;
                }
            }
        }
        self.default
    }
}

pub trait ApprovalDecision: Send + Sync {
    fn request<'a>(&'a self, context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>>;
}

/// Injectable policy authorization boundary for runtime and protocol hosts.
pub trait PolicyAuthorizer: Send + Sync {
    fn authorize<'a>(&'a self, context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<()>>;
}

#[derive(Clone)]
pub struct PolicyEngine {
    policy: Option<ExecutionPolicy>,
    approver: Arc<dyn ApprovalDecision>,
}

impl PolicyEngine {
    #[must_use]
    pub fn new(policy: Option<ExecutionPolicy>, approver: Arc<dyn ApprovalDecision>) -> Self {
        Self { policy, approver }
    }

    pub async fn authorize(&self, context: &PolicyContext) -> RuntimeResult<()> {
        let decision = self
            .policy
            .as_ref()
            .map_or(PolicyDecision::Approval, |policy| policy.decision(context));
        match decision {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny => Err(RuntimeError::new(
                "policy_denied",
                "execution policy denied this operation",
            )),
            PolicyDecision::Approval => {
                if self.approver.request(context).await? {
                    Ok(())
                } else {
                    Err(RuntimeError::approval(
                        "operation requires explicit approval",
                    ))
                }
            }
        }
    }
}

impl PolicyAuthorizer for PolicyEngine {
    fn authorize<'a>(&'a self, context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move { PolicyEngine::authorize(self, context).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reject;
    impl ApprovalDecision for Reject {
        fn request<'a>(
            &'a self,
            _context: &'a PolicyContext,
        ) -> BoxFuture<'a, RuntimeResult<bool>> {
            Box::pin(async { Ok(false) })
        }
    }

    #[tokio::test]
    async fn missing_policy_fails_closed() {
        let engine = PolicyEngine::new(None, Arc::new(Reject));
        let result = engine
            .authorize(&PolicyContext {
                agent_id: "a".into(),
                tool_name: "fs_delete".into(),
                root: None,
                destructive: true,
            })
            .await;
        assert_eq!(
            result.expect_err("missing policy must not allow").code,
            "approval_required"
        );
    }
}
