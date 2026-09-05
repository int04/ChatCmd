//! Direct, bounded local-machine execution primitives for ChatCMD.

// RuntimeError intentionally carries structured context used across the public runtime API.
// Boxing it would be a broad compatibility change, so keep the established result type.
#![allow(clippy::result_large_err)]

mod artifact_quota;
mod blob_store;
mod budget;
mod command_execution_journal;
mod command_execution_registry;
mod command_runner;
mod command_source_state;
mod filesystem;
mod git_parser;
mod git_service;
mod policy;
mod process_runner;
mod project_context;
mod services;
mod shell;
mod skill_service;
mod telemetry;
mod tool_result;
mod types;
mod workspace_ignore;

pub use blob_store::*;
pub use budget::*;
pub use command_runner::*;
pub use filesystem::{
    FileVersion, MutationFaultInjector, MutationJournalSink, SearchProgress, WorkspaceService,
};
pub use git_service::{GitCommitPreview, GitService};
pub use policy::{
    ApprovalDecision, ExecutionPolicy, PolicyAuthorizer, PolicyContext, PolicyDecision,
    PolicyEngine,
};
pub use project_context::{
    ProjectContextBundle, ProjectContextPolicy, ProjectContextRange, ProjectContextService,
    ProjectRuleKind, ProjectRuleRecord,
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
