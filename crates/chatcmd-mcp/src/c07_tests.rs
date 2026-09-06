use super::*;

#[test]
fn metadata_separates_structural_and_behavior_contract_hashes() {
    let metadata = catalog_metadata();
    assert_eq!(metadata.catalog_hash, catalog_hash());
    assert_eq!(metadata.instructions_hash, instructions_hash());
    assert_eq!(
        metadata.instructions_version,
        server_contract::instructions::INSTRUCTIONS_VERSION
    );
    assert!(metadata.instructions_hash.starts_with("sha256:"));
    assert_ne!(metadata.instructions_hash, metadata.catalog_hash);
}

#[test]
fn behavior_wording_changes_only_the_instruction_hash_channel() {
    let first = serde_json::json!({"description": "first", "version": 1});
    let second = serde_json::json!({"description": "second", "version": 1});
    assert_ne!(
        tool_catalog::hash_instruction_value(&first),
        tool_catalog::hash_instruction_value(&second)
    );
    assert_eq!(
        tool_catalog::hash_manifest_value(&first),
        tool_catalog::hash_manifest_value(&second)
    );
}

#[test]
fn consent_and_git_preview_fields_are_additive_and_safe_by_default() {
    let question: PlanQuestionArgs = serde_json::from_value(serde_json::json!({
        "question": "Proceed?", "options": ["yes", "no"]
    }))
    .expect("legacy clarification args");
    assert!(matches!(
        question.question_kind,
        PlanQuestionKindArgs::Clarification
    ));
    let commit: GitCommitArgs = serde_json::from_value(serde_json::json!({
        "message": "scoped", "paths": ["selected.txt"]
    }))
    .expect("scoped commit args");
    assert!(!commit.all);
    assert!(!commit.preview_only);
    assert!(commit.expected_preview.is_none());
    let schema =
        serde_json::to_value(schemars::schema_for!(GitCommitArgs)).expect("git commit schema");
    assert!(schema["properties"].get("previewOnly").is_some());
    assert!(schema["properties"].get("expectedPreview").is_some());
}

#[test]
fn command_run_schema_and_authorization_metadata_are_explicit() {
    assert!(TOOL_NAMES.iter().any(|name| name == "command_run"));
    let capabilities = tool_capabilities("command_run");
    assert_eq!(
        capabilities.operation_class,
        ToolOperationClass::ProcessExecution
    );
    assert_eq!(capabilities.risk_class, ToolRiskClass::ProcessExecution);
    assert!(capabilities.approval_required);
    assert!(capabilities.mutating);
    assert_eq!(capabilities.path_fields, vec![PathFieldRole::Cwd]);
    let schema = serde_json::to_value(schemars::schema_for!(CommandRunArgs))
        .expect("command_run input schema");
    assert!(schema["required"].as_array().is_some_and(|required| {
        required.iter().any(|field| field == "executable")
            && required.iter().any(|field| field == "cwd")
    }));
    assert!(schema["properties"].get("arguments").is_some());
    assert!(schema["properties"].get("timeoutMs").is_some());
    assert!(canonical_manifest().to_string().contains("terminalState"));
}

#[test]
fn executable_contract_examples_deserialize_and_match_advertised_schemas() {
    let examples: Value = serde_json::from_str(include_str!(
        "../tests/coding_fixtures/contract_examples.json"
    ))
    .expect("contract examples JSON");
    serde_json::from_value::<CommandRunArgs>(examples["commandRun"].clone())
        .expect("command_run example");
    serde_json::from_value::<ProjectContextArgs>(examples["projectContext"].clone())
        .expect("project_context example");
    serde_json::from_value::<GitCommitArgs>(examples["gitPreview"].clone())
        .expect("git preview example");
    serde_json::from_value::<PlanQuestionArgs>(examples["executionConsent"].clone())
        .expect("execution consent example");
    serde_json::from_value::<CompleteArgs>(examples["completion"].clone())
        .expect("completion example");
    serde_json::from_value::<chatcmd_runtime::GitCommitPreview>(
        examples["gitPreviewResult"].clone(),
    )
    .expect("git preview result");
    serde_json::from_value::<chatcmd_runtime::ProjectContextBundle>(
        examples["projectContextResult"].clone(),
    )
    .expect("project context result");

    let error = server_contract::error_value(&RuntimeError::new(
        "execution_not_found",
        "execution evidence was not found",
    ));
    assert_eq!(error, examples["completionError"]);
    assert!(
        canonical_manifest()["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|tool| !tool["resultSchema"].is_null())
    );
}

#[tokio::test]
async fn project_rule_digest_changes_without_requiring_catalog_reconnect() {
    let fixture = tempfile::tempdir().expect("fixture");
    std::fs::write(fixture.path().join("AGENTS.md"), "first").expect("first rule");
    let catalog_before = catalog_hash();
    let service = chatcmd_runtime::ProjectContextService::default();
    let first = service
        .load(fixture.path(), &[])
        .await
        .expect("first context");
    std::fs::write(fixture.path().join("AGENTS.md"), "second").expect("second rule");
    let second = service
        .load(fixture.path(), &[])
        .await
        .expect("second context");
    assert_ne!(first.effective_hash, second.effective_hash);
    assert_eq!(catalog_before, catalog_hash());
}

#[test]
fn catalog_v8_mismatch_has_reconnect_metadata_and_bounded_recovery() {
    let arguments = ToolArguments {
        client_catalog_hash: Some("sha256:v7-cached".to_owned()),
        ..ToolArguments::default()
    };
    let result = catalog_mismatch(&arguments).expect("cached v7 must mismatch");
    let value = result.structured_content.expect("structured mismatch");
    assert_eq!(value["catalogVersion"], 8);
    assert_eq!(value["error"]["recovery"], "refreshAndRetry");
    assert_eq!(value["reconnect"]["maxAttempts"], 1);
}
