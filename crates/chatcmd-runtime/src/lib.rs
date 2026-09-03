//! Direct, bounded local-machine execution primitives for ChatCMD.

mod filesystem;
mod policy;
mod services;
mod shell;
mod skill_service;
mod tool_result;
mod types;
mod workspace_ignore;

pub use filesystem::{FileVersion, SearchProgress, WorkspaceService};
pub use policy::{
    ApprovalDecision, ExecutionPolicy, PolicyAuthorizer, PolicyContext, PolicyDecision,
    PolicyEngine,
};
pub use services::{GitService, ProcessService};
pub use shell::ShellRuntime;
pub use skill_service::{
    ManagedSkill, SkillInstallCandidate, SkillInstallPreview, SkillOption, SkillOptionChoice,
    SkillService,
};
pub use tool_result::*;
pub use types::*;
pub use workspace_ignore::{TraversalOptions, WorkspaceIgnorePolicy, is_default_ignored_component};
