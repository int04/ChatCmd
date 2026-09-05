use super::*;

#[tokio::test]
async fn resolves_option_using_server_owned_option_text() {
    let registry = PlanPromptRegistry::default();
    let (view, receiver, _guard) = registry
        .register(
            "task-1".to_owned(),
            "turn-1".to_owned(),
            "agent-1".to_owned(),
            "Choose".to_owned(),
            ["PHP".to_owned(), "C#".to_owned()],
            PlanQuestionKind::Clarification,
            10,
            120_000,
        )
        .expect("register prompt");
    registry
        .resolve(
            &view.id,
            Some("task-1"),
            Some("turn-1"),
            PlanPromptResolution::Option(2),
            20,
        )
        .expect("resolve option");
    let answer = receiver.await.expect("receive answer");
    assert_eq!(answer.option_index, Some(2));
    assert_eq!(answer.text, "C#");
    assert!(!answer.custom);
    assert!(registry.pending().expect("pending prompts").is_empty());
}

#[test]
fn pending_prompts_are_fifo_and_invalid_answer_keeps_prompt() {
    let registry = PlanPromptRegistry::default();
    let (second, _receiver, _guard) = registry
        .register(
            "task-2".to_owned(),
            "turn-2".to_owned(),
            "agent-2".to_owned(),
            "Second".to_owned(),
            ["A".to_owned(), "B".to_owned()],
            PlanQuestionKind::Clarification,
            20,
            120_000,
        )
        .expect("register second prompt");
    let (first, _receiver, _guard) = registry
        .register(
            "task-1".to_owned(),
            "turn-1".to_owned(),
            "agent-1".to_owned(),
            "First".to_owned(),
            ["A".to_owned(), "B".to_owned()],
            PlanQuestionKind::Clarification,
            10,
            120_000,
        )
        .expect("register first prompt");
    assert!(matches!(
        registry.resolve(
            &first.id,
            Some("task-1"),
            Some("turn-1"),
            PlanPromptResolution::Option(3),
            30
        ),
        Err(PlanPromptResolveError::InvalidOption)
    ));
    let pending = registry.pending().expect("pending prompts");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].id, first.id);
    assert_eq!(pending[1].id, second.id);
}

#[tokio::test]
async fn consent_semantics_ignore_ai_option_order_and_custom_text() {
    let registry = PlanPromptRegistry::default();
    let (view, receiver, _guard) = registry
        .register(
            "task-consent".to_owned(),
            "turn-consent".to_owned(),
            "agent-consent".to_owned(),
            "Proceed?".to_owned(),
            ["No".to_owned(), "Yes".to_owned()],
            PlanQuestionKind::ExecutionConsent,
            100,
            1_000,
        )
        .expect("register consent");
    assert!(matches!(
        registry.resolve(
            &view.id,
            Some("task-consent"),
            Some("turn-consent"),
            PlanPromptResolution::Custom("yes".to_owned()),
            200,
        ),
        Err(PlanPromptResolveError::InvalidResolution)
    ));
    registry
        .resolve(
            &view.id,
            Some("task-consent"),
            Some("turn-consent"),
            PlanPromptResolution::ApproveExecution,
            200,
        )
        .expect("approve consent");
    assert_eq!(
        receiver.await.expect("answer").consent_state,
        Some(ConsentState::Approved)
    );
}

#[test]
fn scoped_late_and_replayed_answers_fail_closed() {
    let registry = PlanPromptRegistry::default();
    let (view, _receiver, _guard) = registry
        .register(
            "task-a".to_owned(),
            "turn-a".to_owned(),
            "agent-a".to_owned(),
            "Proceed?".to_owned(),
            ["Yes".to_owned(), "No".to_owned()],
            PlanQuestionKind::ExecutionConsent,
            10,
            10,
        )
        .expect("register consent");
    assert!(matches!(
        registry.resolve(
            &view.id,
            None,
            None,
            PlanPromptResolution::ApproveExecution,
            15
        ),
        Err(PlanPromptResolveError::ScopeMismatch)
    ));
    assert!(matches!(
        registry.resolve(
            &view.id,
            Some("task-b"),
            Some("turn-a"),
            PlanPromptResolution::ApproveExecution,
            15
        ),
        Err(PlanPromptResolveError::ScopeMismatch)
    ));
    assert!(matches!(
        registry.resolve(
            &view.id,
            Some("task-a"),
            Some("turn-a"),
            PlanPromptResolution::ApproveExecution,
            20
        ),
        Err(PlanPromptResolveError::Expired)
    ));
    assert!(matches!(
        registry.resolve(
            &view.id,
            Some("task-a"),
            Some("turn-a"),
            PlanPromptResolution::ApproveExecution,
            20
        ),
        Err(PlanPromptResolveError::NotFound)
    ));
}
