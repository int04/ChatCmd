use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCreateRequest {
    pub request_id: String,
    pub working_directory: Option<PathBuf>,
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub columns: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellSessionInfo {
    pub session_id: String,
    pub status: String,
    pub process_id: Option<u32>,
    pub executable: String,
    pub initial_working_directory: PathBuf,
    pub columns: u16,
    pub rows: u16,
    pub created_at_unix_ms: u128,
    pub exit_code: Option<i32>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellWriteRequest {
    pub request_id: String,
    pub session_id: String,
    pub text: String,
    #[serde(default = "default_true")]
    pub append_new_line: bool,
    #[serde(default)]
    pub input_kind: ShellInputKind,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ShellInputKind {
    #[default]
    Interactive,
    Paste,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellSignal {
    #[serde(alias = "SIGINT", alias = "sigint")]
    CtrlC,
    #[serde(alias = "SIGBREAK", alias = "sigbreak")]
    CtrlBreak,
    #[serde(alias = "EOF", alias = "eof")]
    Eof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEvent {
    pub sequence: u64,
    pub timestamp_unix_ms: u128,
    pub event_type: String,
    pub stream: String,
    pub data: String,
    pub encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellReadResult {
    pub session_id: String,
    pub oldest_available_sequence: u64,
    pub latest_available_sequence: u64,
    pub replay_truncated: bool,
    pub dropped_bytes: u64,
    pub dropped_events: u64,
    pub events: Vec<ShellEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellWaitResult {
    pub session_id: String,
    pub completed: bool,
    pub wait_timed_out: bool,
    pub exit_code: Option<i32>,
    pub last_sequence: u64,
}
