use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub process_id: u32,
    pub name: String,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillReadResult {
    pub id: String,
    pub name: String,
    pub source: String,
    pub instructions: String,
    pub truncated: bool,
}

pub trait TaskRuntime: Send + Sync {
    fn task_get<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn task_list(&self) -> BoxFuture<'_, RuntimeResult<serde_json::Value>>;
    fn set_execution_mode<'a>(
        &'a self,
        task_id: &'a str,
        mode: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn artifact_list<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn artifact_read<'a>(
        &'a self,
        task_id: &'a str,
        artifact_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
}

pub trait ProgressSink: Send + Sync {
    fn progress<'a>(
        &'a self,
        task_id: &'a str,
        turn_id: &'a str,
        message: &'a str,
        suggested_title: Option<&'a str>,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn turn_complete<'a>(
        &'a self,
        task_id: &'a str,
        turn_id: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
}

pub type SharedEventSink = Arc<dyn EventSink>;
