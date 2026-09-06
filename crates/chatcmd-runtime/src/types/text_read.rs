use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextReadResult {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadBudget {
    #[serde(default = "default_text_read_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_text_read_max_bytes_read")]
    pub max_bytes_read: u64,
}

impl Default for TextReadBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_text_read_timeout_ms(),
            max_bytes_read: default_text_read_max_bytes_read(),
        }
    }
}

const fn default_text_read_timeout_ms() -> u64 {
    10_000
}

const fn default_text_read_max_bytes_read() -> u64 {
    8 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", rename_all = "camelCase")]
pub enum TextReadRange {
    Line { start: usize, limit: usize },
    Byte { start: u64, limit: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadRequestV2 {
    pub path: PathBuf,
    pub range: TextReadRange,
    #[serde(default = "default_text_read_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_true")]
    pub include_line_endings: bool,
    #[serde(default)]
    pub expected_version: Option<String>,
    #[serde(default)]
    pub budget: TextReadBudget,
}

const fn default_text_read_max_bytes() -> usize {
    256 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadRangeResult {
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub start_byte: Option<u64>,
    pub end_byte: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadResultV2 {
    pub path: PathBuf,
    pub content: String,
    pub range: TextReadRangeResult,
    pub next_start_line: Option<usize>,
    pub next_byte_offset: Option<u64>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub bytes_read: u64,
    pub size_bytes: u64,
    pub version_token: String,
    pub encoding: String,
    pub bom: bool,
    pub line_ending: String,
    pub line_ending_detection: String,
    pub total_lines: Option<usize>,
    pub total_lines_known: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsBatchReadRequest {
    pub requests: Vec<TextReadRequestV2>,
    #[serde(default = "default_batch_read_max_items")]
    pub max_items: usize,
    #[serde(default = "default_batch_read_output_bytes")]
    pub max_total_output_bytes: usize,
    #[serde(default = "default_batch_read_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub budget: TextReadBudget,
}

const fn default_batch_read_max_items() -> usize {
    50
}
const fn default_batch_read_output_bytes() -> usize {
    1024 * 1024
}
const fn default_batch_read_concurrency() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchReadItem {
    pub path: PathBuf,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TextReadResultV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BatchItemError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchReadResult {
    pub items: Vec<FsBatchReadItem>,
    pub usage: FsBatchUsage,
    pub output_bytes: usize,
    pub truncated: bool,
}
