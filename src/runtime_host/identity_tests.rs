use super::{
    select_task_identity, task_identity_from_first_message, unique_bridge_task_for_message,
};

#[test]
fn explicit_task_overrides_changed_conversation_scope() {
    assert_eq!(
        select_task_identity(
            "agent",
            Some("openai:chat-b"),
            Some("task-current"),
            None,
            "r1"
        ),
        "task-current"
    );
}

#[test]
fn bound_turn_overrides_changed_conversation_scope() {
    assert_eq!(
        select_task_identity(
            "agent",
            Some("openai:chat-b"),
            None,
            Some("task-bound"),
            "r1"
        ),
        "task-bound"
    );
}

#[test]
fn conversation_scope_has_stable_task_identity_across_messages() {
    let first = task_identity_from_first_message("agent", "openai:scope", "xin chào");
    assert_eq!(
        first,
        task_identity_from_first_message("agent", "openai:scope", "một tin nhắn khác")
    );
}

#[test]
fn explicit_task_and_turn_binding_are_safe_fallbacks_without_private_scope() {
    assert_eq!(
        select_task_identity("agent", None, Some("task-known"), Some("task-bound"), "r1"),
        "task-known"
    );
    assert_eq!(
        select_task_identity("agent", None, None, Some("task-bound"), "r1"),
        "task-bound"
    );
    assert_ne!(
        select_task_identity("agent", None, None, None, "r1"),
        select_task_identity("agent", None, None, None, "r2")
    );
}

#[test]
fn unicode_space_bridge_match_requires_one_unambiguous_task() {
    let rows = vec![("task-a".to_owned(), "Ví dụ abcd ".to_owned())];
    assert_eq!(
        unique_bridge_task_for_message(&rows, None, "Ví dụ abcd\u{00a0}"),
        Some("task-a".to_owned())
    );

    let ambiguous = vec![
        ("task-a".to_owned(), "Ví dụ abcd ".to_owned()),
        ("task-b".to_owned(), "Ví dụ abcd\u{202f}".to_owned()),
    ];
    assert_eq!(
        unique_bridge_task_for_message(&ambiguous, None, "Ví dụ abcd\u{00a0}"),
        None
    );
    assert_eq!(
        unique_bridge_task_for_message(&ambiguous, Some("task-b"), "Ví dụ abcd\u{00a0}"),
        Some("task-b".to_owned())
    );
    assert_eq!(
        unique_bridge_task_for_message(&ambiguous, Some("task-unrelated"), "Ví dụ abcd\u{00a0}"),
        None
    );
}
