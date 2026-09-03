//! Direct, bounded local-machine execution primitives for ChatCMD.

mod blob_store;
mod budget;
mod filesystem;
mod git_service;
mod policy;
mod process_runner;
mod services;
mod shell;
mod skill_service;
mod telemetry;
mod tool_result;
mod types;
mod workspace_ignore;

pub use blob_store::*;
pub use budget::*;
pub use filesystem::{FileVersion, SearchProgress, WorkspaceService};
pub use git_service::GitService;
pub use policy::{
    ApprovalDecision, ExecutionPolicy, PolicyAuthorizer, PolicyContext, PolicyDecision,
    PolicyEngine,
};
pub use services::ProcessService;
pub use shell::ShellRuntime;
pub use skill_service::{
    ManagedSkill, SkillInstallCandidate, SkillInstallPreview, SkillOption, SkillOptionChoice,
    SkillService,
};
pub use telemetry::*;
pub use tool_result::*;
pub use types::*;
pub use workspace_ignore::{TraversalOptions, WorkspaceIgnorePolicy, is_default_ignored_component};
