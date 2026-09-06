use super::{CompactCheckpoint, MAX_DETAIL_CHARS, MAX_HANDOFF_BYTES, invalid};
use chatcmd_core::StorageError;
use uuid::Uuid;

/// Identical to the OpenAI scope used by the HTTP bridge and MCP identity layer.
pub fn openai_scope(conversation_id: &str) -> String {
    let material = format!("openai\0{}", conversation_id.trim());
    format!(
        "openai:{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

/// Accept only real ChatGPT conversation paths, not provisional or share URLs.
pub fn validate_conversation(id: &str, url: &str) -> Result<(), StorageError> {
    if !valid_token(id)
        || id.len() > 200
        || url.len() > 2_000
        || url
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'\\')
    {
        return Err(invalid("invalid ChatGPT conversation id or URL"));
    }
    let rest = url
        .strip_prefix("https://chatgpt.com/")
        .ok_or_else(|| invalid("a trusted HTTPS ChatGPT URL is required"))?;
    let path = rest
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    let segments = path.split('/').collect::<Vec<_>>();
    let matches = match segments.as_slice() {
        ["c", value] => *value == id,
        ["g", project, "c", value] => valid_token(project) && *value == id,
        _ => false,
    };
    if !matches {
        return Err(invalid(
            "ChatGPT URL must identify the supplied conversation id",
        ));
    }
    Ok(())
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn validate_operation(operation_id: &str) -> Result<(), StorageError> {
    if operation_id.is_empty()
        || operation_id.len() > 200
        || operation_id.chars().any(char::is_control)
    {
        return Err(invalid("a bounded operation id is required"));
    }
    Ok(())
}

pub(super) fn validate_checkpoint(input: &CompactCheckpoint) -> Result<(), StorageError> {
    if input.expected_revision < 0 || input.expected_revision >= 9_007_199_254_740_991 {
        return Err(invalid(
            "expectedRevision must be a nonnegative safe integer",
        ));
    }
    if input
        .handoff_text
        .as_ref()
        .is_some_and(|text| text.trim().is_empty() || text.len() > MAX_HANDOFF_BYTES)
    {
        return Err(invalid("handoffText must be nonempty and at most 512 KiB"));
    }
    if input
        .detail
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|detail| detail.chars().count() > MAX_DETAIL_CHARS)
    {
        return Err(invalid("detail must be at most 2,000 characters"));
    }
    Ok(())
}
