/// Shared typed argument envelope. Unknown fields remain structured and never become shell text.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolArguments {
    /// Caller-generated idempotency key.
    #[serde(default)]
    pub request_id: String,
    /// Deprecated caller identity accepted only for compatibility and discarded.
    #[serde(default)]
    #[schemars(skip)]
    pub agent_id: String,
    /// Task correlation identifier.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Turn correlation identifier.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Catalog hash last observed by the caller. When present it must match this server build.
    #[serde(default)]
    pub client_catalog_hash: Option<String>,
    /// Deprecated private caller field accepted only for compatibility and discarded.
    #[serde(default, rename = "__chatcmdMcpSessionId")]
    #[schemars(skip)]
    pub(crate) authenticated_session_id: Option<String>,
    /// Deprecated private caller field accepted only for compatibility and discarded.
    #[serde(default, rename = "__chatcmdConversationScopeId")]
    #[schemars(skip)]
    pub(crate) conversation_scope_id: Option<String>,
    /// Tool-specific typed JSON fields.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CommonToolArgs {
    /// Caller-generated idempotency key. Usually omit and let ChatCMD generate one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    request_id: String,
    /// Deprecated caller identity accepted only for compatibility and discarded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schemars(skip)]
    agent_id: String,
    /// Task correlation identifier returned by agent_user_message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// Turn correlation identifier reused for every call in the current user turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    /// Catalog hash last observed by the caller. Send it to detect stale cached schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_catalog_hash: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdMcpSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    authenticated_session_id: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdConversationScopeId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    conversation_scope_id: Option<String>,
}

macro_rules! tool_args {
    ($name:ident {}) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
        }
    };
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),+ $(,)? }) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
            $(
                $(#[$meta])*
                $field: $ty,
            )+
        }
    };
}

const fn default_true() -> bool {
    true
}

const fn default_quarantine_retention_seconds() -> u64 {
    7 * 24 * 60 * 60
}

const fn default_quarantine_max_total_bytes() -> u64 {
    10 * 1024 * 1024 * 1024
}

const fn default_quarantine_max_items() -> u64 {
    10_000
}
