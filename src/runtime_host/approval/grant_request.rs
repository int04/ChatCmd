/// Eligibility is not authorization: callers must still hold a matching approved parent grant.
pub(super) fn subagent_grant_tools() -> Vec<String> {
    TOOL_NAMES
        .iter()
        .filter(|name| {
            let flags = tool_capabilities(name);
            flags.approval_required && flags.risk_class.is_safe_read()
        })
        .cloned()
        .collect()
}

/// Validate before registration, and again when claiming persisted/legacy requests.
/// Never reclassify Git/process/lifecycle tools as safe reads to make a request pass.
pub(super) fn validate_subagent_grant_request(
    request: &SubagentApprovalGrantInput,
) -> RuntimeResult<Vec<String>> {
    if request.allowed_tools.is_empty() || request.path_scopes.is_empty() {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "approvalGrant requires non-empty allowedTools and pathScopes",
        ));
    }
    if request.allowed_tools.len() > 64 || request.path_scopes.len() > 64 {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "approvalGrant exceeds the maximum number of tool/path scopes",
        ));
    }
    for (field, value) in [
        ("maxCalls", request.max_calls),
        ("maxFilesScanned", request.max_files_scanned),
        ("maxBytesRead", request.max_bytes_read),
    ] {
        if value == 0 || i64::try_from(value).is_err() {
            return Err(RuntimeError::new(
                "invalid_arguments",
                format!("approvalGrant {field} must be between 1 and {}", i64::MAX),
            ));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &request.allowed_tools {
        let flags = tool_capabilities(name);
        let duplicate = !seen.insert(name.clone());
        if duplicate || !flags.approval_required || !flags.risk_class.is_safe_read() {
            let preview: String = name.chars().take(120).collect();
            return Err(RuntimeError::new(
                "approval_grant_inheritance_denied",
                format!(
                    "approvalGrant.allowedTools contains {} tool '{preview}'. Use only distinct names from subagentPolicy.approvalGrant.allowedTools. This optional grant is not the child's tool allowlist; Git/process and agent_* lifecycle tools are not eligible. Omit approvalGrant when no approved parent grant is available; normal execution policy still applies.",
                    if duplicate { "duplicate" } else { "ineligible" }
                ),
            ));
        }
    }
    Ok(seen.into_iter().collect())
}
