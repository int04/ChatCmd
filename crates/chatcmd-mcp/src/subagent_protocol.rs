use chatcmd_runtime::{RuntimeError, RuntimeResult};
use serde_json::{Map, Value};

pub(super) enum TextAction {
    Tool {
        name: String,
        arguments: Map<String, Value>,
    },
    Final(String),
}

pub(super) fn parse_text_action(text: &str) -> RuntimeResult<TextAction> {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let value = serde_json::from_str::<Value>(trimmed)
        .or_else(|_| {
            let start = trimmed.find('{').ok_or_else(|| {
                serde_json::Error::io(std::io::Error::other("missing JSON object"))
            })?;
            let end = trimmed.rfind('}').ok_or_else(|| {
                serde_json::Error::io(std::io::Error::other("missing JSON object"))
            })?;
            serde_json::from_str(&trimmed[start..=end])
        })
        .map_err(|_| {
            RuntimeError::new(
                "subagent_protocol_error",
                "child model did not return the required JSON action",
            )
        })?;
    match value.get("action").and_then(Value::as_str) {
        Some("final") => Ok(TextAction::Final(
            value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        )),
        Some("tool") => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if name.is_empty() {
                return Err(RuntimeError::new(
                    "subagent_protocol_error",
                    "child tool action is missing name",
                ));
            }
            Ok(TextAction::Tool { name, arguments })
        }
        _ => Err(RuntimeError::new(
            "subagent_protocol_error",
            "child model action must be tool or final",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{TextAction, parse_text_action};
    use serde_json::json;

    #[test]
    fn parses_manual_tool_action() {
        let action = parse_text_action(
            r#"{"action":"tool","name":"fs_read_text","arguments":{"path":"a.rs"}}"#,
        )
        .expect("action");
        match action {
            TextAction::Tool { name, arguments } => {
                assert_eq!(name, "fs_read_text");
                assert_eq!(arguments.get("path"), Some(&json!("a.rs")));
            }
            TextAction::Final(_) => panic!("expected tool"),
        }
    }
}
