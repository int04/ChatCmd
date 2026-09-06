use std::path::{Component, Path};

use chatcmd_runtime::{RuntimeError, RuntimeResult};

use super::inputs::SubagentStartInput;

const MAX_ITEMS: usize = 64;
const MAX_TEXT_CHARS: usize = 2_000;

pub(super) fn validate_delegation_contract(input: &SubagentStartInput) -> RuntimeResult<()> {
    validate_list("allowedFiles", input.allowed_files.as_deref())?;
    validate_list("allowedEffects", input.allowed_effects.as_deref())?;
    validate_list("dependencies", input.dependencies.as_deref())?;
    validate_list("acceptance", input.acceptance.as_deref())?;
    validate_optional("projectContextRef", input.project_context_ref.as_deref())?;
    validate_optional("instructionsVersion", input.instructions_version.as_deref())?;

    let Some(grant) = input.approval_grant.as_ref() else {
        return Ok(());
    };
    if let Some(effects) = input.allowed_effects.as_deref()
        && effects
            .iter()
            .any(|effect| !matches!(effect.trim(), "read" | "sample"))
    {
        return Err(RuntimeError::new(
            "delegation_scope_widening",
            "approvalGrant is read-only and cannot authorize a modifying delegated effect",
        ));
    }
    if let Some(files) = input.allowed_files.as_deref() {
        for scope in &grant.path_scopes {
            if !files.iter().any(|allowed| path_contains(allowed, scope)) {
                return Err(RuntimeError::new(
                    "delegation_scope_widening",
                    "approvalGrant pathScopes must be contained by allowedFiles",
                ));
            }
        }
    }
    Ok(())
}

fn validate_list(field: &str, values: Option<&[String]>) -> RuntimeResult<()> {
    let Some(values) = values else { return Ok(()) };
    if values.len() > MAX_ITEMS {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} exceeds {MAX_ITEMS} items"),
        ));
    }
    for value in values {
        validate_text(field, value)?;
    }
    Ok(())
}

fn validate_optional(field: &str, value: Option<&str>) -> RuntimeResult<()> {
    if let Some(value) = value {
        validate_text(field, value)?;
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> RuntimeResult<()> {
    if value.trim().is_empty() || value.chars().count() > MAX_TEXT_CHARS {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} contains an empty or oversized value"),
        ));
    }
    Ok(())
}

fn path_contains(allowed: &str, requested: &str) -> bool {
    let allowed = Path::new(allowed);
    let requested = Path::new(requested);
    if allowed
        .components()
        .any(|part| part == Component::ParentDir)
        || requested
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return false;
    }
    requested.starts_with(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::inputs::{SubagentApprovalGrantInput, SubagentStartInput};

    fn input() -> SubagentStartInput {
        SubagentStartInput {
            name: "reader".into(),
            request: "inspect".into(),
            allowed_files: Some(vec!["src".into()]),
            allowed_effects: Some(vec!["read".into()]),
            dependencies: None,
            acceptance: None,
            project_context_ref: None,
            instructions_version: None,
            approval_grant: Some(SubagentApprovalGrantInput {
                allowed_tools: vec!["fs_read_text".into()],
                path_scopes: vec!["src/lib.rs".into()],
                max_calls: 1,
                max_files_scanned: 1,
                max_bytes_read: 100,
            }),
        }
    }

    #[test]
    fn read_only_manifest_may_only_narrow_the_c01_grant() {
        assert!(validate_delegation_contract(&input()).is_ok());
        let mut widened = input();
        widened.approval_grant.as_mut().expect("grant").path_scopes = vec!["tests".into()];
        assert_eq!(
            validate_delegation_contract(&widened)
                .expect_err("widened")
                .code,
            "delegation_scope_widening"
        );
        let mut modifying = input();
        modifying.allowed_effects = Some(vec!["write".into()]);
        assert_eq!(
            validate_delegation_contract(&modifying)
                .expect_err("modify")
                .code,
            "delegation_scope_widening"
        );
    }
}
