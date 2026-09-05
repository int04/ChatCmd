#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_policy_is_independent_from_lifecycle_and_cleanup() {
        assert!(tool_capabilities("shell_create").approval_required);
        assert!(tool_capabilities("shell_write").approval_required);
        assert!(tool_capabilities("fs_read_text").approval_required);
        assert!(tool_capabilities("git_status").approval_required);
        assert!(tool_capabilities("process_kill").approval_required);
        assert!(!tool_capabilities("agent_progress").approval_required);
        assert!(!tool_capabilities("task_get").approval_required);
        assert!(tool_capabilities("task_set_execution_mode").is_permission_change());
    }

    #[test]
    fn every_advertised_tool_has_an_explicit_operation_class() {
        for tool in TOOL_NAMES.iter() {
            assert!(
                !tool_capabilities(tool).is_permission_change()
                    || tool == "task_set_execution_mode",
                "tool {tool} fell through the fail-closed permission-change classification"
            );
        }
    }

    #[test]
    fn subtree_matching_does_not_allow_prefix_siblings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("repo");
        let sibling = directory.path().join("repository");
        std::fs::create_dir_all(root.join("src")).expect("project tree");
        std::fs::create_dir_all(&sibling).expect("sibling tree");
        let root = std::fs::canonicalize(root).expect("canonical root");
        let scopes = vec![GrantPathScope {
            path: normalized_path(&root),
            kind: GrantPathScopeKind::Subtree,
            identity: path_identity(&root),
        }];
        assert!(path_allowed(&root.join("src"), &scopes));
        assert!(!path_allowed(&sibling, &scopes));
    }

    #[test]
    fn mutation_summary_redacts_content_and_digest_binds_it() {
        let first = json!({"path": ".", "content": "top-secret", "expectedVersion": "v1"});
        let second = json!({"path": ".", "content": "changed", "expectedVersion": "v1"});
        let summary = approval_summary("fs_write_text", ToolRiskClass::Modify, &first);
        assert!(!summary.to_string().contains("top-secret"));
        assert_eq!(summary["contentBytesEstimate"], 10);
        assert_ne!(
            operation_digest("fs_write_text", &first),
            operation_digest("fs_write_text", &second)
        );
    }

    #[test]
    fn apply_edits_summary_exposes_counts_without_replacement_payload() {
        let replacement = "sensitive replacement payload";
        let arguments = json!({
            "path": ".",
            "expectedVersion": "v1-test",
            "edits": [
                {"startByte": 1, "endByte": 2, "text": replacement},
                {"startByte": 4, "endByte": 4, "text": "xy"}
            ],
            "dryRun": false,
            "budget": {"maxBytesRead": 4096, "maxBytesWritten": 4096}
        });
        let summary = approval_summary("fs_apply_edits", ToolRiskClass::Modify, &arguments);
        let serialized = summary.to_string();

        assert_eq!(summary["expectedVersion"], "v1-test");
        assert_eq!(summary["editCount"], 2);
        assert_eq!(summary["contentBytesEstimate"], replacement.len() + 2);
        assert_eq!(summary["contentRedacted"], true);
        assert!(!serialized.contains(replacement));
        assert!(serialized.contains("fs_apply_edits"));
    }

    #[test]
    fn read_and_mutation_risk_classes_do_not_overlap() {
        assert!(tool_capabilities("fs_search").risk_class.is_safe_read());
        assert!(!tool_capabilities("fs_delete").risk_class.is_safe_read());
        assert!(!tool_capabilities("git_status").risk_class.is_safe_read());
    }

    #[test]
    fn reusable_safe_read_grant_covers_read_family_only() {
        let tools = TOOL_NAMES
            .iter()
            .filter(|name| {
                let capabilities = tool_capabilities(name);
                capabilities.approval_required && capabilities.risk_class.is_safe_read()
            })
            .cloned()
            .collect::<Vec<_>>();
        assert!(tools.iter().any(|name| name == "fs_stat"));
        assert!(tools.iter().any(|name| name == "fs_read_text"));
        assert!(tools.iter().any(|name| name == "fs_search"));
        assert!(!tools.iter().any(|name| name == "fs_write_text"));
        assert!(!tools.iter().any(|name| name == "git_status"));
    }

    #[test]
    fn requested_charge_never_clips_explicit_budget_to_grant_cap() {
        let arguments = json!({
            "budget": {
                "maxFilesScanned": SAFE_READ_MAX_FILES + 1,
                "maxBytesRead": SAFE_READ_MAX_BYTES + 1
            }
        });
        let charge = requested_charge("fs_search", &arguments);
        assert_eq!(charge.files, SAFE_READ_MAX_FILES + 1);
        assert_eq!(charge.bytes_read, SAFE_READ_MAX_BYTES + 1);

        let batch = requested_charge("fs_batch_read", &json!({"maxItems": 500}));
        assert_eq!(batch.files, 500);
    }

    #[test]
    fn operation_digest_is_canonical_for_object_key_order() {
        let first = serde_json::from_str::<Value>(
            r#"{"path":"src","budget":{"maxBytesRead":4096,"timeoutMs":10}}"#,
        )
        .expect("first JSON");
        let second = serde_json::from_str::<Value>(
            r#"{"budget":{"timeoutMs":10,"maxBytesRead":4096},"path":"src"}"#,
        )
        .expect("second JSON");
        assert_eq!(
            operation_digest("fs_read_text", &first),
            operation_digest("fs_read_text", &second)
        );
        assert_ne!(
            operation_digest("fs_read_text", &first),
            operation_digest(
                "fs_read_text",
                &json!({"path":"other","budget":{"maxBytesRead":4096,"timeoutMs":10}})
            )
        );
    }

    #[test]
    fn option_constraints_fail_closed_and_match_safe_defaults() {
        let constraints = r#"{"includeIgnored":false,"includeHidden":false}"#;
        assert!(option_constraints_match(&json!({}), constraints));
        assert!(option_constraints_match(
            &json!({"includeIgnored":false,"includeHidden":false}),
            constraints
        ));
        assert!(!option_constraints_match(
            &json!({"includeIgnored":true}),
            constraints
        ));
        assert!(!option_constraints_match(
            &json!({}),
            r#"{"futureConstraint":false}"#
        ));
    }

    async fn allow_only_tool(host: &RuntimeHost, agent_id: &str, tool: &str) {
        use chatcmd_core::{AgentId, ToolCatalogStore as _};

        let tool_id = host
            .repository
            .list_tools()
            .await
            .expect("list tools")
            .into_iter()
            .find(|candidate| candidate.key == tool)
            .unwrap_or_else(|| panic!("seeded tool {tool}"))
            .id;
        host.repository
            .set_agent_allowed_tools(&AgentId::new(agent_id).expect("agent ID"), &[tool_id])
            .await
            .expect("set allowed tool");
    }

    async fn task_context(
        host: &RuntimeHost,
        agent_id: &str,
        scope: &str,
        turn: &str,
    ) -> OperationContext {
        let accepted = host
            .call_persisted(
                "agent_user_message",
                super::super::user_message_tests::turn_context(
                    &format!("{scope}-user"),
                    agent_id,
                    "agent_user_message",
                    turn,
                    scope,
                ),
                json!({"content":"Run the isolated authorization regression"}),
            )
            .await
            .expect("sync user message");
        let mut context = OperationContext::new(format!("{scope}-call"), agent_id, "shell_create");
        context.task_id = accepted["taskId"].as_str().map(str::to_owned);
        context.turn_id = accepted["turnId"].as_str().map(str::to_owned);
        context.mcp_session_id = accepted["sessionId"].as_str().map(str::to_owned);
        context
    }

    async fn wait_for_pending_approval(host: &RuntimeHost, request_id: &str) {
        for _ in 0..100 {
            let state = sqlx::query_scalar::<_, String>("SELECT state FROM approvals WHERE id=?")
                .bind(request_id)
                .fetch_optional(host.repository.pool())
                .await
                .expect("read approval state");
            if state.as_deref() == Some("pending") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("approval {request_id} did not become pending");
    }

    fn sentinel_command(path: &Path) -> (String, Vec<String>) {
        #[cfg(windows)]
        {
            let escaped = path.to_string_lossy().replace('\'', "''");
            (
                "powershell.exe".to_owned(),
                vec![
                    "-NoProfile".to_owned(),
                    "-NonInteractive".to_owned(),
                    "-Command".to_owned(),
                    format!("[IO.File]::WriteAllText('{escaped}','once')"),
                ],
            )
        }
        #[cfg(not(windows))]
        {
            let escaped = path.to_string_lossy().replace('\'', "'\\''");
            (
                "/bin/sh".to_owned(),
                vec!["-c".to_owned(), format!("printf once > '{escaped}'")],
            )
        }
    }

    #[tokio::test]
    async fn allowlisted_shell_create_cannot_spawn_in_deny_mode() {
        use chatcmd_core::{ExecutionMode, TaskExecutionMode, TaskStore as _};

        let (host, agent_id, directory) = super::super::user_message_tests::test_host().await;
        allow_only_tool(&host, &agent_id, "shell_create").await;
        let context = task_context(&host, &agent_id, "deny-shell", "deny-shell-turn").await;
        let task_id =
            TaskId::new(context.task_id.clone().expect("task ID")).expect("valid task ID");
        host.repository
            .set_execution_mode(&TaskExecutionMode {
                task_id,
                mode: ExecutionMode::Deny,
                updated_at_ms: now_ms(),
            })
            .await
            .expect("deny execution");
        let sentinel = directory.path().join("deny-sentinel.txt");

        let error = <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
            &host,
            "shell_create",
            context,
            json!({
                "workingDirectory": directory.path(),
                "executable": "authorization-test-must-not-start",
                "arguments": [sentinel.to_string_lossy()]
            }),
        )
        .await
        .expect_err("deny mode must reject before spawn");

        assert_eq!(error.code, "policy_denied");
        assert!(!sentinel.exists());
        assert!(host.shell.list().await.expect("list shells").is_empty());
    }

    #[tokio::test]
    async fn mcp_tool_cannot_elevate_execution_mode() {
        use chatcmd_core::{ExecutionMode, TaskExecutionMode, TaskStore as _};

        let (host, agent_id, _directory) = super::super::user_message_tests::test_host().await;
        sqlx::query("INSERT INTO tools(id,key,group_id,title,description,input_schema_json,capabilities_json,enabled) VALUES('tool-task-mode-test','task_set_execution_mode','group-device','Task mode','Task mode','{}','[]',1)")
            .execute(host.repository.pool())
            .await
            .expect("seed task mode tool");
        allow_only_tool(&host, &agent_id, "task_set_execution_mode").await;
        let mut context =
            task_context(&host, &agent_id, "deny-elevation", "deny-elevation-turn").await;
        context.tool_name = "task_set_execution_mode".to_owned();
        let task_id =
            TaskId::new(context.task_id.clone().expect("task ID")).expect("valid task ID");
        host.repository
            .set_execution_mode(&TaskExecutionMode {
                task_id: task_id.clone(),
                mode: ExecutionMode::Deny,
                updated_at_ms: now_ms(),
            })
            .await
            .expect("deny execution");

        let error = <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
            &host,
            "task_set_execution_mode",
            context,
            json!({"mode":"allow"}),
        )
        .await
        .expect_err("MCP permission change must be rejected");

        assert_eq!(error.code, "permission_change_requires_user");
        assert_eq!(
            host.repository
                .execution_mode(Some(&task_id))
                .await
                .expect("read execution mode"),
            ExecutionMode::Deny
        );
    }

    #[tokio::test]
    async fn deny_mode_keeps_owned_cleanup_control_available() {
        use chatcmd_core::{ExecutionMode, TaskExecutionMode, TaskStore as _};

        let (host, agent_id, _directory) = super::super::user_message_tests::test_host().await;
        sqlx::query("INSERT INTO tools(id,key,group_id,title,description,input_schema_json,capabilities_json,enabled) VALUES('tool-shell-close-test','shell_close','group-terminal','Close shell','Close shell','{}','[]',1)")
            .execute(host.repository.pool())
            .await
            .expect("seed shell close tool");
        allow_only_tool(&host, &agent_id, "shell_close").await;
        let mut context = task_context(&host, &agent_id, "deny-cleanup", "deny-cleanup-turn").await;
        context.tool_name = "shell_close".to_owned();
        let task_id =
            TaskId::new(context.task_id.clone().expect("task ID")).expect("valid task ID");
        host.repository
            .set_execution_mode(&TaskExecutionMode {
                task_id,
                mode: ExecutionMode::Deny,
                updated_at_ms: now_ms(),
            })
            .await
            .expect("deny execution");

        let error = <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
            &host,
            "shell_close",
            context,
            json!({"sessionId":"owned-session-that-already-ended"}),
        )
        .await
        .expect_err("missing cleanup target should reach shell runtime");

        assert_ne!(error.code, "policy_denied");
        assert_eq!(error.code, "session_not_found");
    }

    #[tokio::test]
    async fn approval_mode_does_not_spawn_until_exact_request_is_approved() {
        let (host, agent_id, directory) = super::super::user_message_tests::test_host().await;
        allow_only_tool(&host, &agent_id, "shell_create").await;
        let context = task_context(&host, &agent_id, "approve-shell", "approve-shell-turn").await;
        let request_id = context.request_id.clone();
        let replay_context = context.clone();
        let sentinel = directory.path().join("approved-sentinel.txt");
        let (executable, arguments) = sentinel_command(&sentinel);
        let call_host = host.clone();
        let workdir = directory.path().to_path_buf();
        let pending = tokio::spawn(async move {
            <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
                &call_host,
                "shell_create",
                context,
                json!({
                    "workingDirectory": workdir,
                    "executable": executable,
                    "arguments": arguments
                }),
            )
            .await
        });

        wait_for_pending_approval(&host, &request_id).await;
        assert!(!sentinel.exists(), "command ran before approval");
        assert!(host.shell.list().await.expect("list shells").is_empty());
        sqlx::query("UPDATE approvals SET state='approved',decision_json='{}',resolved_at_ms=? WHERE id=? AND state='pending'")
            .bind(now_ms())
            .bind(&request_id)
            .execute(host.repository.pool())
            .await
            .expect("approve request");
        let output = pending
            .await
            .expect("approval task join")
            .expect("approved shell create");

        for _ in 0..100 {
            if sentinel.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&sentinel).expect("sentinel"),
            "once"
        );
        let replay = <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
            &host,
            "shell_create",
            replay_context,
            json!({
                "workingDirectory": directory.path(),
                "executable": "authorization-replay-must-not-start",
                "arguments": []
            }),
        )
        .await
        .expect_err("approval request IDs are single-use");
        assert_eq!(replay.code, "approval_replayed");
        if let Some(session_id) = output.get("sessionId").and_then(Value::as_str) {
            let _ = host
                .shell
                .close(
                    &OperationContext::new("approve-cleanup", &agent_id, "shell_close"),
                    session_id,
                    true,
                )
                .await;
        }
    }

    #[tokio::test]
    async fn rejected_and_cancelled_approvals_never_spawn() {
        let (host, agent_id, directory) = super::super::user_message_tests::test_host().await;
        allow_only_tool(&host, &agent_id, "shell_create").await;

        for (suffix, resolution) in [("rejected", "rejected"), ("cancelled", "cancelled")] {
            let mut context = task_context(
                &host,
                &agent_id,
                &format!("{suffix}-shell"),
                &format!("{suffix}-shell-turn"),
            )
            .await;
            context.request_id = format!("{suffix}-shell-call");
            let request_id = context.request_id.clone();
            let sentinel = directory.path().join(format!("{suffix}-sentinel.txt"));
            let call_host = host.clone();
            let workdir = directory.path().to_path_buf();
            let pending = tokio::spawn(async move {
                <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
                    &call_host,
                    "shell_create",
                    context,
                    json!({
                        "workingDirectory": workdir,
                        "executable": "authorization-test-must-not-start",
                        "arguments": []
                    }),
                )
                .await
            });
            wait_for_pending_approval(&host, &request_id).await;
            sqlx::query("UPDATE approvals SET state=?,decision_json='{}',resolved_at_ms=? WHERE id=? AND state='pending'")
                .bind(resolution)
                .bind(now_ms())
                .bind(&request_id)
                .execute(host.repository.pool())
                .await
                .expect("resolve approval");
            let error = pending
                .await
                .expect("approval task join")
                .expect_err("resolution must reject execution");
            assert!(matches!(
                error.code.as_str(),
                "command_rejected_by_user" | "approval_cancelled"
            ));
            assert!(!sentinel.exists());
            assert!(host.shell.list().await.expect("list shells").is_empty());
        }
    }
}
