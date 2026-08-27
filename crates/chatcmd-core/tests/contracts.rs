use chatcmd_core::{DeviceId, GeneratedSecret, SecretHash, TaskId};

#[test]
fn identifiers_preserve_unsafe_json_integers_as_strings() {
    let raw = "9223372036854775807";
    let id = DeviceId::new(raw).expect("valid ID");
    assert_eq!(id.as_str(), raw);
    assert_eq!(
        serde_json::to_string(&id).expect("serialize"),
        format!("\"{raw}\"")
    );

    let task = TaskId::new("task_9007199254740993").expect("valid task ID");
    assert_eq!(task.as_str(), "task_9007199254740993");
}

#[test]
fn secret_hash_comparison_and_one_time_exposure_work() {
    let first = SecretHash::from_token("secret-one");
    let same = SecretHash::from_token("secret-one");
    let other = SecretHash::from_token("secret-two");
    assert!(first.constant_time_eq(&same));
    assert!(!first.constant_time_eq(&other));

    let generated = GeneratedSecret::new("abcdefghijklmnopqrstuvwxyz1234".to_owned());
    assert_eq!(generated.last4(), "1234");
    assert_eq!(generated.expose_once(), "abcdefghijklmnopqrstuvwxyz1234");
}
