use super::*;

pub(super) fn prune_observations(state: &mut DesktopState) {
    state
        .observations
        .retain(|_, value| value.created_at.elapsed() <= OBSERVATION_TTL);
    if state.observations.len() >= MAX_OBSERVATIONS
        && let Some(oldest) = state
            .observations
            .iter()
            .min_by_key(|(_, value)| value.created_at)
            .map(|(id, _)| id.clone())
    {
        state.observations.remove(&oldest);
    }
}

pub(super) fn validate_element_action(action: &DesktopElementAction) -> RuntimeResult<()> {
    if matches!(action, DesktopElementAction::SetValue { text } if text.len() > 32 * 1024) {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "text exceeds the 32 KiB action limit",
        ));
    }
    Ok(())
}

pub(super) fn validate_input_actions(actions: &[DesktopInputAction]) -> RuntimeResult<()> {
    if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "actions must contain between 1 and 32 items",
        ));
    }
    for action in actions {
        match action {
            DesktopInputAction::Drag { duration_ms, .. } if !(1..=5_000).contains(duration_ms) => {
                return Err(RuntimeError::new(
                    "invalid_arguments",
                    "drag durationMs must be between 1 and 5000",
                ));
            }
            DesktopInputAction::Wait { duration_ms } if !(1..=10_000).contains(duration_ms) => {
                return Err(RuntimeError::new(
                    "invalid_arguments",
                    "wait durationMs must be between 1 and 10000",
                ));
            }
            DesktopInputAction::Keypress { keys } if keys.is_empty() || keys.len() > 8 => {
                return Err(RuntimeError::new(
                    "invalid_arguments",
                    "keypress requires between 1 and 8 keys",
                ));
            }
            DesktopInputAction::Keypress { keys } if keys.iter().any(|key| key.len() > 32) => {
                return Err(RuntimeError::new(
                    "invalid_arguments",
                    "key names are limited to 32 bytes",
                ));
            }
            DesktopInputAction::Type { text } if text.len() > 32 * 1024 => {
                return Err(RuntimeError::new(
                    "invalid_arguments",
                    "typed text exceeds the 32 KiB action limit",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn join_error(error: tokio::task::JoinError) -> RuntimeError {
    RuntimeError::new(
        "desktop_backend_error",
        format!("desktop worker failed: {error}"),
    )
}
