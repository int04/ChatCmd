use super::*;
use crate::runtime_host::user_message_tests::test_host;

#[tokio::test]
async fn subagent_settings_default_to_zero_and_round_trip_all_supported_values() {
    let (host, _agent, dir) = test_host().await;
    let state = Arc::new(host.test_app_state(dir.path().join("test.db").display().to_string()));
    assert_eq!(
        settings_value(&state).await.unwrap()["subagentConcurrency"],
        0
    );
    for limit in 0..=5 {
        let result = save_settings(
            State(state.clone()),
            Json(json!({"port":8080,"subagentConcurrency":limit})),
        )
        .await
        .unwrap();
        assert_eq!(result.0["subagentConcurrency"], limit);
    }
    for invalid in [json!(-1), json!(6), json!(1.5), json!("0")] {
        assert!(
            save_settings(
                State(state.clone()),
                Json(json!({"port":8080,"subagentConcurrency":invalid}))
            )
            .await
            .is_err()
        );
    }
}
