//! Durable Compact & Resume state, independent of the browser worker lifetime.
mod checkpoint;
mod guards;
mod queries;
mod start;
mod validation;

pub use guards::{guard_callback, guard_native};
pub use validation::{openai_scope, validate_conversation};

use chatcmd_core::StorageError;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_HANDOFF_BYTES: usize = 512 * 1024;
pub const MAX_DETAIL_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum CompactPhase {
    Preparing,
    WritingHandoff,
    SavingHandoff,
    OpeningNewChat,
    Completed,
    Cancelled,
}

impl CompactPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::WritingHandoff => "writing_handoff",
            Self::SavingHandoff => "saving_handoff",
            Self::OpeningNewChat => "opening_new_chat",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    fn rank(self) -> u8 {
        match self {
            Self::Preparing => 0,
            Self::WritingHandoff => 1,
            Self::SavingHandoff => 2,
            Self::OpeningNewChat => 3,
            Self::Completed => 4,
            Self::Cancelled => 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CompactJob {
    pub id: String,
    pub task_id: String,
    pub phase: CompactPhase,
    pub revision: i64,
    pub old_conversation_id: String,
    pub old_conversation_url: String,
    pub old_model: String,
    pub old_request_id: Option<String>,
    pub old_scope_hash: Option<String>,
    pub new_conversation_id: Option<String>,
    pub new_conversation_url: Option<String>,
    pub handoff_text: Option<String>,
    pub detail: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub agent_name: Option<String>,
    pub project_folder: Option<String>,
    /// Explicit per-job opt-in. Missing/legacy records never continue automatically.
    #[serde(default)]
    pub continue_after_compact: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactCheckpoint {
    pub expected_revision: i64,
    pub phase: Option<CompactPhase>,
    pub handoff_text: Option<String>,
    pub new_conversation_id: Option<String>,
    pub new_conversation_url: Option<String>,
    // Missing retains the detail; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "nullable_detail",
        skip_serializing_if = "Option::is_none"
    )]
    pub detail: Option<Option<String>>,
}

fn nullable_detail<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactTaskJobs {
    pub active: Option<CompactJob>,
    pub history: Vec<CompactJob>,
}

// Keep summary queries from loading or returning potentially large handoff text.
const SUMMARY_COLUMNS: &str = "id,task_id,phase,revision,old_conversation_id,old_conversation_url,old_model,old_request_id,old_scope_hash,new_conversation_id,new_conversation_url,NULL AS handoff_text,detail,created_at_ms,updated_at_ms,completed_at_ms,agent_name,project_folder,continue_after_compact";

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.to_owned())
}
fn conflict(message: &str) -> StorageError {
    StorageError::Conflict(message.to_owned())
}
fn missing() -> StorageError {
    StorageError::NotFound("compact job or ChatGPT task".to_owned())
}
fn db(error: sqlx::Error) -> StorageError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation() || database.message().starts_with("compact:"))
    {
        return conflict("compact state conflicts with an existing binding or operation");
    }
    StorageError::Backend(format!("compact storage: {error}"))
}
