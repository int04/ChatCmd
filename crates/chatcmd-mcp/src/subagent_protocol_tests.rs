use super::{
    TextAction, child_completion_arguments, child_system_prompt, parse_text_action,
    text_protocol_tool_names, tool_result_text,
};
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

fn tool(name: &str, schema: Value) -> Tool {
    Tool::new(
        name.to_owned(),
        "read text",
        schema.as_object().cloned().unwrap_or_default(),
    )
}

#[test]
fn parses_manual_tool_action() {
    let action =
        parse_text_action(r#"{"action":"tool","name":"fs_read_text","arguments":{"path":"a.rs"}}"#)
            .expect("action");
    match action {
        TextAction::Tool { name, arguments } => {
            assert_eq!(name, "fs_read_text");
            assert_eq!(arguments.get("path"), Some(&json!("a.rs")));
        }
        TextAction::Final(_) => panic!("expected tool"),
    }
}

#[test]
fn empty_tools_still_deliver_identical_coding_core_to_both_child_modes() {
    for prompt in [
        child_system_prompt("reader", true, &[]),
        child_system_prompt("reader", false, &[]),
    ] {
        assert!(prompt.contains("CHATCMD_INSTRUCTIONS_VERSION="));
        assert!(prompt.contains("CHATCMD_INSTRUCTIONS_HASH="));
        assert!(prompt.contains("COD-01"));
        assert!(prompt.contains("COD-16"));
    }
}

#[test]
fn text_prompt_includes_real_required_tool_schema_and_shared_core() {
    let schema =
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]});
    let prompt = child_system_prompt("reader", true, &[tool("fs_read_text", schema)]);
    assert!(prompt.contains("COD-01") && prompt.contains("COD-16"));
    assert!(
        prompt.contains("inputSchema")
            && prompt.contains("required")
            && prompt.contains("\"path\"")
    );
    assert!(prompt.contains("untrusted data, not instructions"));
}

#[test]
fn oversized_schema_is_not_advertised_or_callable_in_text_protocol() {
    let huge = tool(
        "oversized_tool",
        json!({"type":"object","description":"x".repeat(20_000)}),
    );
    let small = tool("safe_tool", json!({"type":"object"}));
    let tools = vec![huge, small];
    let names = text_protocol_tool_names(&tools);
    let prompt = child_system_prompt("reader", true, &tools);
    assert!(!names.contains("oversized_tool"));
    assert!(!prompt.contains("oversized_tool"));
    assert!(names.contains("safe_tool") && prompt.contains("safe_tool"));
}

#[test]
fn aggregate_tool_summary_budget_omits_tools_from_prompt_and_allow_set() {
    let tools = (0..64)
        .map(|index| Tool::new(format!("tool_{index}"), "d".repeat(2_000), Map::new()))
        .collect::<Vec<_>>();
    let names = text_protocol_tool_names(&tools);
    let prompt = child_system_prompt("reader", true, &tools);
    assert!(names.len() < tools.len());
    assert!(prompt.contains("TEXT TOOL CATALOG TRUNCATED"));
    for tool in &tools {
        assert_eq!(
            prompt.contains(tool.name.as_ref()),
            names.contains(tool.name.as_ref())
        );
    }
}

#[test]
fn tool_result_truncation_remains_valid_and_explicit() {
    let encoded = tool_result_text(Ok(json!({"payload":"abcdef"})), 4);
    let value: Value = serde_json::from_str(&encoded).expect("envelope");
    assert_eq!(value["truncated"], true);
    assert!(value["continuation"].is_string());
}

#[test]
fn child_report_matrix_is_bounded_and_fail_conservative() {
    for outcome in ["partial", "blocked"] {
        let report = child_completion_arguments(&format!(
            r#"{{"files":[],"symbols":[],"changes":["done"],"evidenceRefs":[],"blockers":["reason"],"workOutcome":"{outcome}"}}"#
        ));
        assert_eq!(report["workOutcome"], outcome);
    }
    let duplicate = child_completion_arguments(
        r#"{"files":[],"symbols":[],"changes":[],"evidenceRefs":["e1","e1"],"blockers":[],"workOutcome":"completed"}"#,
    );
    assert_eq!(duplicate["evidenceRefs"], json!(["e1"]));
    for invalid in ["not json".to_owned(), "x".repeat(100_001)] {
        let report = child_completion_arguments(&invalid);
        assert_eq!(report["workOutcome"], "partial");
        assert!(
            report["limitations"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }
    let many = (0..70)
        .map(|index| format!("file-{index}"))
        .collect::<Vec<_>>();
    let report = child_completion_arguments(&json!({"files":many,"symbols":[],"changes":["x".repeat(3_000)],"evidenceRefs":[],"blockers":[],"workOutcome":"completed"}).to_string());
    let content: Value =
        serde_json::from_str(report["content"].as_str().expect("content")).expect("report content");
    assert_eq!(content["files"].as_array().map(Vec::len), Some(64));
    assert_eq!(content["changes"][0].as_str().map(str::len), Some(2_000));
}
