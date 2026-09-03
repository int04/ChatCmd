use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{RuntimeError, RuntimeResult};

pub const TOOL_RESULT_SCHEMA_VERSION: u16 = 1;
pub const CURSOR_VERSION: u16 = 1;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEnvelope<T> {
    pub schema_version: u16,
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ToolUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ToolWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<ContentRef>,
}

impl<T> ToolResultEnvelope<T> {
    #[must_use]
    pub fn complete(data: T) -> Self {
        Self {
            schema_version: TOOL_RESULT_SCHEMA_VERSION,
            data,
            page: None,
            truncation: None,
            usage: None,
            warnings: Vec::new(),
            content_ref: None,
        }
    }

    #[must_use]
    pub fn paged(data: T, next_cursor: Option<String>, has_more: bool) -> Self {
        let mut result = Self::complete(data);
        result.page = Some(PageInfo {
            next_cursor,
            has_more,
        });
        result
    }

    #[must_use]
    pub fn truncated(
        data: T,
        reason: TruncationReason,
        returned_items: u64,
        omitted_items: Option<u64>,
    ) -> Self {
        let mut result = Self::complete(data);
        result.truncation = Some(TruncationInfo {
            truncated: true,
            reason: Some(reason),
            returned_items,
            omitted_items,
        });
        result
    }

    #[must_use]
    pub fn externalized(data: T, content_ref: ContentRef, returned_items: u64) -> Self {
        let mut result = Self::truncated(
            data,
            TruncationReason::ContentExternalized,
            returned_items,
            None,
        );
        result.content_ref = Some(content_ref);
        result
    }

    #[must_use]
    pub fn with_warning(mut self, warning: ToolWarning) -> Self {
        self.warnings.push(warning);
        self
    }

    #[must_use]
    pub fn with_usage(mut self, usage: ToolUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}

impl<T: Serialize> ToolResultEnvelope<T> {
    /// Updates usage.outputBytes until serialized length stabilizes.
    pub fn measure_output_bytes(&mut self) -> RuntimeResult<()> {
        if self.usage.is_none() {
            self.usage = Some(ToolUsage::default());
        }
        for _ in 0..4 {
            let bytes = serde_json::to_vec(self).map_err(|error| {
                RuntimeError::new("result_serialization_failed", error.to_string())
            })?;
            let measured = bytes.len() as u64;
            let usage = self.usage.as_mut().expect("usage initialized above");
            if usage.output_bytes == measured {
                return Ok(());
            }
            usage.output_bytes = measured;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TruncationInfo {
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<TruncationReason>,
    pub returned_items: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_items: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TruncationReason {
    OutputLimit,
    ItemLimit,
    TimeBudget,
    FileBudget,
    ByteBudget,
    Cancelled,
    ReplayEvicted,
    BinaryContent,
    ContentExternalized,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolUsage {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_scanned: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_read: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_written: Option<u64>,
    pub output_bytes: u64,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContentRef {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CursorCodec {
    key: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload {
    version: u16,
    tool_kind: String,
    scope_hash: String,
    state: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_unix_ms: Option<u64>,
}

impl CursorCodec {
    #[must_use]
    pub fn ephemeral() -> Self {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut hash = Sha256::new();
        hash.update(first.as_bytes());
        hash.update(second.as_bytes());
        Self {
            key: hash.finalize().into(),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn from_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn encode<T: Serialize>(
        &self,
        tool_kind: &str,
        normalized_scope: &str,
        state: &T,
        expires_at_unix_ms: Option<u64>,
    ) -> RuntimeResult<String> {
        let payload = CursorPayload {
            version: CURSOR_VERSION,
            tool_kind: tool_kind.to_owned(),
            scope_hash: scope_hash(normalized_scope),
            state: serde_json::to_value(state).map_err(cursor_serde_error)?,
            expires_at_unix_ms,
        };
        let payload_bytes = serde_json::to_vec(&payload).map_err(cursor_serde_error)?;
        let signature = self.sign(&payload_bytes)?;
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload_bytes),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    pub fn decode<T: DeserializeOwned>(
        &self,
        cursor: &str,
        expected_tool_kind: &str,
        normalized_scope: &str,
    ) -> RuntimeResult<T> {
        let (payload_part, signature_part) = cursor
            .split_once('.')
            .ok_or_else(|| cursor_error("invalid_cursor", "cursor is malformed"))?;
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(payload_part)
            .map_err(|_| cursor_error("invalid_cursor", "cursor payload is malformed"))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_part)
            .map_err(|_| cursor_error("invalid_cursor", "cursor signature is malformed"))?;
        self.verify(&payload_bytes, &signature)?;
        let payload: CursorPayload = serde_json::from_slice(&payload_bytes)
            .map_err(|_| cursor_error("invalid_cursor", "cursor payload is invalid"))?;
        if payload.version != CURSOR_VERSION {
            return Err(cursor_error(
                "cursor_version_unsupported",
                "cursor version is unsupported",
            ));
        }
        if payload.tool_kind != expected_tool_kind
            || payload.scope_hash != scope_hash(normalized_scope)
        {
            return Err(cursor_error(
                "cursor_scope_mismatch",
                "cursor does not belong to this tool or scope",
            ));
        }
        if payload
            .expires_at_unix_ms
            .is_some_and(|expires_at| now_unix_ms() > expires_at)
        {
            return Err(cursor_error("cursor_expired", "cursor has expired"));
        }
        serde_json::from_value(payload.state)
            .map_err(|_| cursor_error("invalid_cursor", "cursor state is invalid"))
    }

    fn sign(&self, bytes: &[u8]) -> RuntimeResult<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|_| cursor_error("invalid_cursor_key", "cursor signing key is invalid"))?;
        mac.update(bytes);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    fn verify(&self, bytes: &[u8], signature: &[u8]) -> RuntimeResult<()> {
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|_| cursor_error("invalid_cursor_key", "cursor signing key is invalid"))?;
        mac.update(bytes);
        mac.verify_slice(signature)
            .map_err(|_| cursor_error("invalid_cursor", "cursor signature is invalid"))
    }
}

#[must_use]
pub fn scope_hash(normalized_scope: &str) -> String {
    let digest = Sha256::digest(normalized_scope.as_bytes());
    format!("sha256:{digest:x}")
}

fn cursor_serde_error(error: serde_json::Error) -> RuntimeError {
    cursor_error("invalid_cursor", error.to_string())
}

fn cursor_error(code: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(code, message)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codec() -> CursorCodec {
        CursorCodec::from_key([7; 32])
    }

    #[test]
    fn optional_fields_are_omitted() {
        let value = serde_json::to_value(ToolResultEnvelope::complete(vec![1, 2]))
            .expect("serialize envelope");
        assert_eq!(value, json!({"schemaVersion": 1, "data": [1, 2]}));
    }

    #[test]
    fn truncation_reasons_are_camel_case() {
        let reasons = [
            (TruncationReason::OutputLimit, "outputLimit"),
            (TruncationReason::ItemLimit, "itemLimit"),
            (TruncationReason::TimeBudget, "timeBudget"),
            (TruncationReason::FileBudget, "fileBudget"),
            (TruncationReason::ByteBudget, "byteBudget"),
            (TruncationReason::Cancelled, "cancelled"),
            (TruncationReason::ReplayEvicted, "replayEvicted"),
            (TruncationReason::BinaryContent, "binaryContent"),
            (TruncationReason::ContentExternalized, "contentExternalized"),
        ];
        for (reason, expected) in reasons {
            assert_eq!(serde_json::to_value(reason).unwrap(), json!(expected));
        }
    }

    #[test]
    fn envelope_round_trips_and_measures_output() {
        let mut envelope = ToolResultEnvelope::paged(
            vec!["a".to_owned(), "b".to_owned()],
            Some("next".into()),
            true,
        )
        .with_usage(ToolUsage::default());
        envelope.measure_output_bytes().expect("measure output");
        let bytes = serde_json::to_vec(&envelope).expect("serialize");
        let decoded: ToolResultEnvelope<Vec<String>> =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(decoded, envelope);
        assert_eq!(envelope.usage.unwrap().output_bytes, bytes.len() as u64);
    }

    #[test]
    fn result_schema_snapshot_has_stable_top_level_fields() {
        let schema = serde_json::to_value(schemars::schema_for!(
            ToolResultEnvelope<Vec<crate::FsEntry>>
        ))
        .expect("serialize result schema");
        let mut properties = schema["properties"]
            .as_object()
            .expect("schema properties")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        properties.sort();
        assert_eq!(
            properties,
            vec![
                "contentRef",
                "data",
                "page",
                "schemaVersion",
                "truncation",
                "usage",
                "warnings",
            ]
        );
        let mut required = schema["required"]
            .as_array()
            .expect("required fields")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        required.sort();
        assert_eq!(required, vec!["data", "schemaVersion"]);
    }

    #[test]
    fn legacy_fs_list_shape_remains_a_plain_array() {
        let entries = vec![crate::FsEntry {
            path: std::path::PathBuf::from("src/lib.rs"),
            name: "lib.rs".into(),
            entry_type: "file".into(),
            size: 12,
            readonly: false,
        }];
        let value = serde_json::to_value(entries).expect("serialize legacy entries");
        assert!(value.is_array());
        assert!(value.get("schemaVersion").is_none());
    }

    #[test]
    fn externalized_result_marks_truncation_and_content_reference() {
        let result = ToolResultEnvelope::externalized(
            json!({"preview": "bounded"}),
            ContentRef {
                id: "artifact-1".into(),
                media_type: Some("text/plain".into()),
                size_bytes: Some(2048),
            },
            1,
        );
        let value = serde_json::to_value(result).expect("serialize externalized result");
        assert_eq!(value["truncation"]["truncated"], true);
        assert_eq!(value["truncation"]["reason"], "contentExternalized");
        assert_eq!(value["contentRef"]["id"], "artifact-1");
    }

    #[test]
    fn cursor_is_scope_bound_and_tamper_evident() {
        let cursor = codec()
            .encode("fs_list_v2", "c:/repo/src", &json!({"offset": 10}), None)
            .unwrap();
        let state: Value = codec()
            .decode(&cursor, "fs_list_v2", "c:/repo/src")
            .unwrap();
        assert_eq!(state, json!({"offset": 10}));
        assert_eq!(
            codec()
                .decode::<Value>(&cursor, "fs_list_v2", "c:/repo/other")
                .unwrap_err()
                .code,
            "cursor_scope_mismatch"
        );
        assert_eq!(
            codec()
                .decode::<Value>(&cursor, "fs_search_v2", "c:/repo/src")
                .unwrap_err()
                .code,
            "cursor_scope_mismatch"
        );
        let tampered = format!("{}A", cursor);
        assert_eq!(
            codec()
                .decode::<Value>(&tampered, "fs_list_v2", "c:/repo/src")
                .unwrap_err()
                .code,
            "invalid_cursor"
        );
    }

    #[test]
    fn malformed_expired_and_unsupported_cursors_have_stable_codes() {
        assert_eq!(
            codec()
                .decode::<Value>("nope", "fs_list_v2", "scope")
                .unwrap_err()
                .code,
            "invalid_cursor"
        );
        let expired = codec()
            .encode("fs_list_v2", "scope", &json!({"offset": 1}), Some(1))
            .unwrap();
        assert_eq!(
            codec()
                .decode::<Value>(&expired, "fs_list_v2", "scope")
                .unwrap_err()
                .code,
            "cursor_expired"
        );

        let payload = CursorPayload {
            version: CURSOR_VERSION + 1,
            tool_kind: "fs_list_v2".into(),
            scope_hash: scope_hash("scope"),
            state: json!({"offset": 1}),
            expires_at_unix_ms: None,
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let signature = codec().sign(&payload_bytes).unwrap();
        let cursor = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload_bytes),
            URL_SAFE_NO_PAD.encode(signature)
        );
        assert_eq!(
            codec()
                .decode::<Value>(&cursor, "fs_list_v2", "scope")
                .unwrap_err()
                .code,
            "cursor_version_unsupported"
        );
    }

    #[test]
    fn cursor_does_not_expose_signing_material_or_scope() {
        let cursor = codec()
            .encode(
                "fs_list_v2",
                "secret/internal/path",
                &json!({"offset": 3}),
                None,
            )
            .unwrap();
        assert!(!cursor.contains("secret/internal/path"));
        assert!(!cursor.contains("07070707"));
    }
}
