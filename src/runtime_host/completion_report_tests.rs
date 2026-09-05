use super::*;

fn input() -> CompleteInput {
    CompleteInput {
        content: "done".to_owned(),
        suggested_title: None,
        work_outcome: Some(WorkOutcome::Completed),
        verification_intent: None,
        verification_reason: None,
        verification_scope: Some("crate unit tests".to_owned()),
        criteria: vec![CompletionCriterionInput {
            criterion: "tests pass".to_owned(),
            evidence_refs: vec!["execution-1".to_owned()],
        }],
        evidence_refs: Vec::new(),
        blockers: Vec::new(),
        limitations: Vec::new(),
    }
}

fn record(status: &str) -> Value {
    json!({"executionId": "execution-1", "status": status})
}

#[test]
fn legacy_completion_is_not_run_instead_of_verified() {
    let mut legacy = input();
    legacy.verification_scope = None;
    legacy.criteria.clear();
    assert_eq!(
        aggregate_verification(&legacy, &[], &[], &[]),
        VerificationState::NotRun
    );
}

#[test]
fn docs_only_requires_a_reason_for_not_applicable() {
    let mut docs = input();
    docs.criteria.clear();
    docs.verification_scope = None;
    docs.verification_intent = Some(VerificationIntent::NotApplicable);
    assert_eq!(
        aggregate_verification(&docs, &[], &[], &[]),
        VerificationState::Unknown
    );
    docs.verification_reason = Some("documentation-only change".to_owned());
    assert_eq!(
        aggregate_verification(&docs, &[], &[], &[]),
        VerificationState::NotApplicable
    );
}

#[test]
fn missing_partial_or_failed_evidence_never_becomes_passed() {
    let report = input();
    let refs = vec!["execution-1".to_owned()];
    assert_eq!(
        aggregate_verification(&report, &refs, &[], &[json!({"code": "missing"})]),
        VerificationState::Unknown
    );
    assert_eq!(
        aggregate_verification(&report, &refs, &[record("failed")], &[]),
        VerificationState::Failed
    );
    assert_eq!(
        aggregate_verification(&report, &refs, &[record("stale")], &[]),
        VerificationState::Stale
    );
    let mut partial = input();
    partial.criteria.push(CompletionCriterionInput {
        criterion: "uncovered".to_owned(),
        evidence_refs: Vec::new(),
    });
    assert_eq!(
        aggregate_verification(&partial, &refs, &[record("passed")], &[]),
        VerificationState::Unknown
    );
}

#[test]
fn terminal_state_and_source_freshness_are_server_owned() {
    let fresh = json!({
        "exitCode": 0,
        "timedOut": false,
        "cancelled": false,
        "sourceStateBefore": {"digest": "state-a", "complete": true},
        "sourceStateAfter": {"digest": "state-a", "complete": true},
        "sourceStateCurrent": {"digest": "state-a", "complete": true}
    });
    assert_eq!(evidence_status(&fresh, true), VerificationState::Passed);
    assert_eq!(evidence_status(&fresh, false), VerificationState::Stale);
    let changed = json!({"exitCode": 0, "sourceStateBefore": {"digest": "a", "complete": true}, "sourceStateAfter": {"digest": "b", "complete": true}, "sourceStateCurrent": {"digest": "b", "complete": true}});
    assert_eq!(evidence_status(&changed, true), VerificationState::Stale);
    let unknown = json!({"exitCode": 0});
    assert_eq!(evidence_status(&unknown, true), VerificationState::Unknown);
    let null_states = json!({"exitCode": 0, "sourceStateBefore": null, "sourceStateAfter": null, "sourceStateCurrent": null});
    assert_eq!(
        evidence_status(&null_states, true),
        VerificationState::Unknown
    );
    let timeout = json!({"exitCode": null, "timedOut": true});
    assert_eq!(evidence_status(&timeout, true), VerificationState::Failed);
    let recovered = json!({"terminalState": "unknown", "exitCode": null});
    assert_eq!(
        evidence_status(&recovered, true),
        VerificationState::Unknown
    );
}

#[tokio::test]
async fn persisted_command_evidence_is_owner_bound_and_conservative() {
    use crate::runtime_host::user_message_tests::test_host;
    use chatcmd_core::{
        AgentId, ExecutionMode, TaskExecutionMode, TaskId, TaskStore as _, ToolCatalogStore as _,
    };
    use chatcmd_runtime::OperationContext;

    let (host, agent_id, directory) = test_host().await;
    let project = directory.path().join("quality-project");
    std::fs::create_dir(&project).expect("quality project");
    crate::catalog_seed::seed_catalog(&host.repository)
        .await
        .expect("seed current catalog");
    let allowed = host
        .repository
        .list_tools()
        .await
        .expect("list tools")
        .into_iter()
        .filter(|tool| matches!(tool.key.as_str(), "fs_read_text" | "command_run"))
        .map(|tool| tool.id)
        .collect::<Vec<_>>();
    host.repository
        .set_agent_allowed_tools(&AgentId::new(&agent_id).expect("agent id"), &allowed)
        .await
        .expect("allow command evidence fixture");
    let mut user = OperationContext::new("quality-user", &agent_id, "agent_user_message");
    user.turn_id = Some("quality-turn".to_owned());
    user.conversation_scope_id = Some("quality-conversation".to_owned());
    let accepted = host
        .call_persisted(
            "agent_user_message",
            user,
            json!({"content": "Verify the work"}),
        )
        .await
        .expect("persist user message");
    let mut command = OperationContext::new("quality-command", &agent_id, "command_run");
    command.task_id = accepted["taskId"].as_str().map(str::to_owned);
    command.turn_id = accepted["turnId"].as_str().map(str::to_owned);
    command.mcp_session_id = accepted["sessionId"].as_str().map(str::to_owned);
    host.repository
        .set_execution_mode(&TaskExecutionMode {
            task_id: TaskId::new(command.task_id.as_deref().expect("task id")).expect("task id"),
            mode: ExecutionMode::Allow,
            updated_at_ms: now_ms(),
        })
        .await
        .expect("allow task execution");
    #[cfg(windows)]
    let (executable, arguments) = ("cmd.exe", vec!["/C", "exit 0"]);
    #[cfg(not(windows))]
    let (executable, arguments) = ("sh", vec!["-c", "exit 0"]);
    let output = host
        .call_persisted(
            "command_run",
            command.clone(),
            json!({
                "executable": executable,
                "arguments": arguments,
                "cwd": &project,
                "timeoutMs": 5_000
            }),
        )
        .await
        .expect("run command");
    let execution_id = output["executionId"]
        .as_str()
        .expect("execution id")
        .to_owned();
    let mut report = input();
    report.evidence_refs = vec![execution_id.clone()];
    report.criteria[0].evidence_refs = vec![execution_id];
    let normalized = host.normalize_completion_report(&command, &report).await;
    assert_eq!(normalized["verification"], "passed");
    assert_eq!(normalized["evidence"][0]["exitCode"], 0);
    assert_eq!(
        normalized["evidence"][0]["reason"],
        "server-owned command evidence passed"
    );

    std::fs::write(project.join("late-untracked.txt"), "changed after test")
        .expect("late source edit");
    let stale = host.normalize_completion_report(&command, &report).await;
    assert_eq!(stale["verification"], "stale");
    assert_eq!(stale["evidence"][0]["status"], "stale");
    let mut wrong_agent = command.clone();
    wrong_agent.agent_id = "another-agent".to_owned();
    let rejected = host
        .normalize_completion_report(&wrong_agent, &report)
        .await;
    assert_eq!(rejected["verification"], "unknown");
    assert_eq!(rejected["evidence"].as_array().map(Vec::len), Some(0));
    assert_eq!(rejected["diagnostics"][0]["code"], "execution_not_found");

    let mut wrong_task = command.clone();
    wrong_task.task_id = Some("another-task".to_owned());
    let cross_task = host.normalize_completion_report(&wrong_task, &report).await;
    assert_eq!(cross_task["verification"], "unknown");
    assert_eq!(cross_task["evidence"].as_array().map(Vec::len), Some(0));

    let mut complete = OperationContext::new("quality-complete", &agent_id, "agent_turn_complete");
    complete.task_id = command.task_id;
    complete.turn_id = command.turn_id;
    complete.mcp_session_id = command.mcp_session_id;
    let finalized = host
        .call_persisted(
            "agent_turn_complete",
            complete,
            json!({
                "content": "Blocked by the test environment.",
                "workOutcome": "blocked",
                "evidenceRefs": ["forged-execution"],
                "blockers": ["required SDK is unavailable"]
            }),
        )
        .await
        .expect("honest blocked report still finalizes");
    assert_eq!(finalized["accepted"], true);
    assert_eq!(finalized["qualityReport"]["workOutcome"], "blocked");
    assert_eq!(finalized["qualityReport"]["verification"], "unknown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_child_evidence_requires_current_parent_integration_state() {
    use crate::runtime_host::user_message_tests::test_host;
    use chatcmd_core::{
        ActorKind, AgentId, EventId, EventKind, TaskId, TerminalEventStore as _, TimelineEvent,
        ToolCatalogStore as _, TurnId,
    };
    use chatcmd_runtime::{CommandRunRequest, OperationContext};

    let (host, agent_id, directory) = test_host().await;
    let project = directory.path().join("delegated-quality");
    std::fs::create_dir(&project).expect("project");
    crate::catalog_seed::seed_catalog(&host.repository)
        .await
        .expect("catalog");
    let tools = host.repository.list_tools().await.expect("tools");
    let allowed = tools
        .into_iter()
        .filter(|tool| matches!(tool.key.as_str(), "fs_read_text" | "command_run"))
        .map(|tool| tool.id)
        .collect::<Vec<_>>();
    host.repository
        .set_agent_allowed_tools(&AgentId::new(&agent_id).expect("agent"), &allowed)
        .await
        .expect("allow tools");

    let mut parent_user = OperationContext::new("e18-parent-user", &agent_id, "agent_user_message");
    parent_user.turn_id = Some("e18-parent-turn".into());
    parent_user.conversation_scope_id = Some("e18-parent-conversation".into());
    let parent = host
        .call_persisted(
            "agent_user_message",
            parent_user,
            json!({"content":"delegate verification"}),
        )
        .await
        .expect("parent message");
    let mut parent_context =
        OperationContext::new("e18-register", &agent_id, "agent_subagent_start");
    parent_context.task_id = parent["taskId"].as_str().map(str::to_owned);
    parent_context.turn_id = parent["turnId"].as_str().map(str::to_owned);
    parent_context.mcp_session_id = parent["sessionId"].as_str().map(str::to_owned);
    let registration = host
        .register_subagent(&parent_context, "verifier", "run tests", None)
        .await
        .expect("registration");
    let child_task = registration["childTaskId"]
        .as_str()
        .expect("child task")
        .to_owned();
    let mut child_command = OperationContext::new("e18-command", &agent_id, "command_run");
    child_command.task_id = Some(child_task.clone());
    child_command.turn_id = Some("e18-child-turn".into());
    #[cfg(windows)]
    let (executable, arguments) = ("cmd.exe", vec!["/C", "exit 0"]);
    #[cfg(not(windows))]
    let (executable, arguments) = ("sh", vec!["-c", "exit 0"]);
    let request: CommandRunRequest = serde_json::from_value(json!({
        "executable": executable, "arguments": arguments, "cwd": &project, "timeoutMs": 5_000
    }))
    .expect("request");
    let output = host
        .command
        .run(&child_command, request)
        .await
        .expect("child verification");
    let evidence = output.execution_id.clone();
    host.repository
        .append_timeline_events(&[TimelineEvent {
            id: EventId::new("event-e18-child-command").expect("event"),
            task_id: TaskId::new(&child_task).expect("child task"),
            turn_id: Some(TurnId::new("e18-child-turn").expect("child turn")),
            session_id: None,
            actor: ActorKind::Tool,
            kind: EventKind::ToolResult,
            idempotency_key: "e18-child-command-result".into(),
            payload_json: json!({"tool":"command_run","output":output}).to_string(),
            metadata_json: None,
            created_at_ms: now_ms(),
        }])
        .await
        .expect("persist child evidence");
    let mut report = input();
    report.evidence_refs = vec![evidence.clone()];
    report.criteria[0].evidence_refs = vec![evidence];
    let fresh = host
        .normalize_completion_report(&parent_context, &report)
        .await;
    assert_eq!(fresh["verification"], "passed");
    assert_eq!(fresh["evidence"][0]["delegatedChild"], true);
    std::fs::write(project.join("parent-integration-edit.rs"), "changed").expect("edit");
    let stale = host
        .normalize_completion_report(&parent_context, &report)
        .await;
    assert_eq!(stale["verification"], "stale");
}
