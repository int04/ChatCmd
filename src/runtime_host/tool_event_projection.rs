use serde::Serialize;
use serde_json::{Map, Value, json};

const EVENT_SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy)]
pub(super) struct EventLimits {
    pub max_event_bytes: usize,
    pub max_preview_bytes: usize,
    pub max_array_items: usize,
    pub max_object_fields: usize,
    pub max_depth: usize,
}

impl Default for EventLimits {
    fn default() -> Self {
        Self {
            // One projection is shared by SQLite and realtime, so this is also the
            // (smaller) realtime limit.
            max_event_bytes: 64 * 1024,
            max_preview_bytes: 8 * 1024,
            max_array_items: 50,
            max_object_fields: 64,
            max_depth: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum RedactionKind {
    Content,
    Binary,
    Credential,
    Environment,
    PrivateScope,
    Collection,
    EventLimit,
}

pub(super) struct ToolEventProjection {
    pub public_summary: Value,
    pub redactions: Vec<RedactionKind>,
    pub received_bytes: usize,
    pub projected_bytes: usize,
    pub truncated: bool,
}

pub(super) fn project(tool: &str, value: &Value, limits: EventLimits) -> ToolEventProjection {
    let received_bytes = estimated_json_bytes(value);
    let mut redactions = Vec::new();
    let mut truncated = false;
    let mut public_summary = project_value(
        tool,
        None,
        value,
        0,
        limits,
        &mut redactions,
        &mut truncated,
    );
    let mut projected_bytes = serialized_len(&public_summary);
    if projected_bytes > limits.max_event_bytes {
        redactions.push(RedactionKind::EventLimit);
        truncated = true;
        public_summary = emergency_summary(tool, value, received_bytes, limits.max_event_bytes);
        projected_bytes = serialized_len(&public_summary);
    }
    ToolEventProjection {
        public_summary,
        redactions,
        received_bytes,
        projected_bytes,
        truncated,
    }
}

pub(super) fn bounded_error_message(message: &str, limits: EventLimits) -> (String, bool) {
    let mut value = Value::String(message.to_owned());
    let mut redactions = Vec::new();
    let mut truncated = false;
    value = project_value(
        "error",
        Some("errorMessage"),
        &value,
        0,
        limits,
        &mut redactions,
        &mut truncated,
    );
    (
        value.as_str().unwrap_or("tool execution failed").to_owned(),
        truncated,
    )
}

fn project_value(
    tool: &str,
    key: Option<&str>,
    value: &Value,
    depth: usize,
    limits: EventLimits,
    redactions: &mut Vec<RedactionKind>,
    truncated: &mut bool,
) -> Value {
    if let Some(kind) = key.and_then(|key| redaction_for_key(tool, key)) {
        redactions.push(kind);
        return redacted_value(value, kind);
    }
    if depth >= limits.max_depth {
        *truncated = true;
        redactions.push(RedactionKind::Collection);
        return json!({"truncated": true, "reason": "depthLimit"});
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(text) => bounded_string(text, limits.max_preview_bytes, truncated),
        Value::Array(items) => {
            let take = items.len().min(limits.max_array_items);
            if take < items.len() {
                *truncated = true;
                redactions.push(RedactionKind::Collection);
            }
            let mut result = Vec::with_capacity(take + usize::from(take < items.len()));
            result.extend(items[..take].iter().map(|item| {
                project_value(tool, key, item, depth + 1, limits, redactions, truncated)
            }));
            if take < items.len() {
                result.push(json!({"truncated": true, "omittedItems": items.len() - take}));
            }
            Value::Array(result)
        }
        Value::Object(object) => {
            let mut result = Map::new();
            for (field, item) in object.iter().take(limits.max_object_fields) {
                result.insert(
                    field.clone(),
                    project_value(
                        tool,
                        Some(field),
                        item,
                        depth + 1,
                        limits,
                        redactions,
                        truncated,
                    ),
                );
            }
            if object.len() > limits.max_object_fields {
                *truncated = true;
                redactions.push(RedactionKind::Collection);
                result.insert("fieldsTruncated".to_owned(), Value::Bool(true));
            }
            Value::Object(result)
        }
    }
}

fn redaction_for_key(tool: &str, key: &str) -> Option<RedactionKind> {
    let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
    if (tool == "shell_write" && normalized == "text")
        || (tool == "shell_read" && normalized == "data")
        || (tool.starts_with("agent_")
            && matches!(normalized.as_str(), "content" | "message" | "request"))
    {
        return Some(RedactionKind::Content);
    }
    match normalized.as_str() {
        "content" | "database64" | "replacement" | "before" | "after" | "diff" | "patch"
        | "submittedcontent" => Some(RedactionKind::Content),
        "base64" | "bytes" | "binary" => Some(RedactionKind::Binary),
        "token" | "accesstoken" | "refreshtoken" | "authorization" | "bearertoken" | "password"
        | "secret" | "apikey" | "pathtoken" | "encryptionkey" => Some(RedactionKind::Credential),
        "environment" | "env" => Some(RedactionKind::Environment),
        "conversationscope" | "conversationscopeid" | "rawscope" => {
            Some(RedactionKind::PrivateScope)
        }
        _ => None,
    }
}

fn redacted_value(value: &Value, kind: RedactionKind) -> Value {
    json!({
        "redacted": true,
        "kind": kind,
        "originalBytes": estimated_json_bytes(value)
    })
}

fn bounded_string(text: &str, max_bytes: usize, truncated: &mut bool) -> Value {
    if text.len() <= max_bytes {
        return Value::String(text.to_owned());
    }
    *truncated = true;
    let end = floor_char_boundary(text, max_bytes);
    json!({
        "preview": &text[..end],
        "truncated": true,
        "originalBytes": text.len()
    })
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn emergency_summary(
    tool: &str,
    value: &Value,
    received_bytes: usize,
    max_event_bytes: usize,
) -> Value {
    let object = value.as_object();
    let preview_bytes = max_event_bytes.saturating_div(4).clamp(16, 1_024);
    let path = object
        .and_then(|value| value.get("path"))
        .and_then(Value::as_str)
        .map(|value| &value[..floor_char_boundary(value, preview_bytes.min(value.len()))]);
    let content_ref = object
        .and_then(|value| value.get("contentRef"))
        .and_then(Value::as_str)
        .map(|value| &value[..floor_char_boundary(value, preview_bytes.min(value.len()))]);
    json!({
        "tool": tool,
        "truncated": true,
        "reason": "eventLimit",
        "originalBytes": received_bytes,
        "path": path,
        "contentRef": content_ref,
        "schemaVersion": EVENT_SCHEMA_VERSION
    })
}

fn estimated_json_bytes(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(value) => {
            if *value {
                4
            } else {
                5
            }
        }
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len().saturating_add(2),
        Value::Array(items) => items.iter().fold(2_usize, |total, item| {
            total
                .saturating_add(estimated_json_bytes(item))
                .saturating_add(1)
        }),
        Value::Object(object) => object.iter().fold(2_usize, |total, (key, value)| {
            total
                .saturating_add(key.len())
                .saturating_add(3)
                .saturating_add(estimated_json_bytes(value))
                .saturating_add(1)
        }),
    }
}

fn serialized_len(value: &Value) -> usize {
    let mut counter = ByteCounter::default();
    if serde_json::to_writer(&mut counter, value).is_err() {
        return usize::MAX;
    }
    counter.len()
}

#[derive(Default)]
struct ByteCounter(usize);

impl ByteCounter {
    const fn len(&self) -> usize {
        self.0
    }
}

impl std::io::Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_sensitive_fields_and_caps_collections() {
        let marker = "PRIVATE-MARKER";
        let value = json!({
            "path": "safe.txt",
            "nested": {
                "content": marker,
                "authorization": marker,
                "environment": marker,
                "conversationScopeId": marker
            },
            "items": (0..100).collect::<Vec<_>>()
        });
        let projection = project("fs_write_text", &value, EventLimits::default());
        let encoded = projection.public_summary.to_string();
        assert!(!encoded.contains(marker));
        assert!(projection.truncated);
        assert!(projection.redactions.contains(&RedactionKind::Content));
        assert!(projection.redactions.contains(&RedactionKind::Credential));
        assert!(projection.redactions.contains(&RedactionKind::Environment));
        assert!(projection.redactions.contains(&RedactionKind::PrivateScope));
    }

    #[test]
    fn hard_cap_falls_back_to_small_metadata_summary() {
        let value = json!({"path":"large.txt", "values": (0..10_000).collect::<Vec<_>>()});
        let limits = EventLimits {
            max_event_bytes: 256,
            max_array_items: 10_000,
            ..EventLimits::default()
        };
        let projection = project("fs_list", &value, limits);
        assert!(projection.projected_bytes <= limits.max_event_bytes);
        assert_eq!(projection.public_summary["reason"], "eventLimit");
    }

    #[test]
    fn utf8_preview_ends_on_character_boundary() {
        let limits = EventLimits {
            max_preview_bytes: 5,
            ..EventLimits::default()
        };
        let projection = project("git_show", &json!({"message":"éééé"}), limits);
        assert_eq!(projection.public_summary["message"]["preview"], "éé");
    }

    #[test]
    fn shell_payload_is_not_duplicated_into_timeline_projection() {
        let marker = "TERMINAL-PRIVATE-MARKER";
        let input = project(
            "shell_write",
            &json!({"sessionId":"session", "text":marker}),
            EventLimits::default(),
        );
        let output = project(
            "shell_read",
            &json!({"events":[{"sequence":1, "data":marker}]}),
            EventLimits::default(),
        );
        assert!(!input.public_summary.to_string().contains(marker));
        assert!(!output.public_summary.to_string().contains(marker));
    }
}
