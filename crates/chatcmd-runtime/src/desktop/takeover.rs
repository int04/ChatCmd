use super::*;

impl DesktopControlService {
    pub async fn input_begin(
        &self,
        context: &OperationContext,
        request: DesktopInputBeginRequest,
    ) -> RuntimeResult<DesktopInputSessionInfo> {
        let record = self.window(context, &request.window_id).await?;
        let target = self.session_target(&record).await?;
        ensure_not_cancelled(context, "desktop input start was cancelled")?;
        let _start_guard = self.input_start_gate.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let owner = owner(context);
        let stale_record = {
            let mut state = self.state.lock().await;
            match std::mem::take(&mut state.input) {
                InputSlot::Empty => None,
                InputSlot::Active(active)
                    if active.cancelled.load(std::sync::atomic::Ordering::Acquire) =>
                {
                    Some(active)
                }
                other => {
                    state.input = other;
                    return Err(RuntimeError::busy(
                        "another desktop input session is active",
                    ));
                }
            }
        };
        if let Some(stale) = stale_record {
            stale
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            let action_gate = stale.action_gate.clone();
            let _action_guard = action_gate.lock().await;
            let input_guard = stale.guard;
            tokio::task::spawn_blocking(move || input_guard.stop())
                .await
                .map_err(join_error)??;
        }
        ensure_not_cancelled(context, "desktop input start was cancelled")?;

        let capture_session = target.session.clone();
        tokio::task::spawn_blocking(move || capture_session.prewarm_capture())
            .await
            .map_err(join_error)??;
        let native = target.native.clone();
        let started = tokio::task::spawn_blocking(move || platform::start_input(&native))
            .await
            .map_err(join_error)?;
        let (guard, cancelled) = started?;
        if context.cancellation.is_cancelled()
            || cancelled.load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::task::spawn_blocking(move || guard.stop())
                .await
                .map_err(join_error)??;
            return Err(RuntimeError::new(
                "desktop_input_stopped",
                "desktop input was stopped during startup",
            ));
        }
        self.state.lock().await.input = InputSlot::Active(Box::new(InputRecord {
            id: id.clone(),
            owner,
            target: target.clone(),
            cancelled,
            action_gate: Arc::new(Mutex::new(())),
            guard,
        }));
        Ok(DesktopInputSessionInfo {
            input_session_id: id,
            window_id: target.window_id,
            active: true,
            escape_to_stop: true,
            overlay_visible: true,
            observation: None,
            verification_warning: None,
            completed_action_count: None,
            execution_warning: None,
        })
    }

    pub async fn input_act(
        &self,
        context: &OperationContext,
        request: DesktopInputActRequest,
    ) -> RuntimeResult<DesktopInputSessionInfo> {
        validate_input_actions(&request.actions)?;
        let (target, cancelled, action_gate) = self
            .input_access(context, &request.input_session_id)
            .await?;
        let _action_guard = action_gate.lock().await;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(RuntimeError::new(
                "desktop_input_stopped",
                "desktop input was stopped by the user",
            ));
        }
        let cancellation = context.cancellation.clone();
        let actions = request.actions;
        let input_cancelled = cancelled.clone();
        let capture_session = target.session.clone();
        let capture_after = request.observe_after && request.include_screenshot;
        let (execution, capture_sequence, capture_barrier_failed) =
            tokio::task::spawn_blocking(move || {
                let execution =
                    capture_session.input_act(&input_cancelled, &cancellation, &actions)?;
                let (sequence, barrier_failed) = if capture_after {
                    match capture_session.current_capture_sequence() {
                        Ok(sequence) => (Some(sequence), false),
                        Err(_) => (None, true),
                    }
                } else {
                    (None, false)
                };
                Ok::<_, RuntimeError>((execution, sequence, barrier_failed))
            })
            .await
            .map_err(join_error)??;
        let (observation, verification_warning) = if request.observe_after {
            if capture_barrier_failed {
                (
                    None,
                    Some(DesktopVerificationWarning::post_action_observation_failed()),
                )
            } else {
                match self
                    .observe_target_after(
                        context,
                        target.clone(),
                        request.include_screenshot,
                        request.include_elements,
                        capture_sequence,
                    )
                    .await
                {
                    Ok(observation) => (Some(observation), None),
                    Err(_) => (
                        None,
                        Some(DesktopVerificationWarning::post_action_observation_failed()),
                    ),
                }
            }
        } else {
            (None, None)
        };
        let active = !cancelled.load(std::sync::atomic::Ordering::Acquire);
        let execution_warning = execution
            .interrupted
            .then(DesktopInputExecutionWarning::partial_input_execution);
        Ok(DesktopInputSessionInfo {
            input_session_id: request.input_session_id,
            window_id: target.window_id,
            active,
            escape_to_stop: true,
            overlay_visible: active,
            observation,
            verification_warning,
            completed_action_count: Some(execution.completed_action_count),
            execution_warning,
        })
    }

    pub async fn input_end(
        &self,
        context: &OperationContext,
        request: DesktopInputEndRequest,
    ) -> RuntimeResult<()> {
        let _lifecycle_guard = self.input_start_gate.lock().await;
        let record = {
            let mut state = self.state.lock().await;
            let InputSlot::Active(active) = &state.input else {
                return Err(session_not_found());
            };
            if active.id != request.input_session_id {
                return Err(session_not_found());
            }
            ensure_owner(&active.owner, context, "desktop_input_session_not_found")?;
            let slot = std::mem::take(&mut state.input);
            match slot {
                InputSlot::Active(record) => {
                    record
                        .cancelled
                        .store(true, std::sync::atomic::Ordering::Release);
                    record
                }
                other => {
                    state.input = other;
                    return Err(session_not_found());
                }
            }
        };
        let action_gate = record.action_gate.clone();
        let _action_guard = action_gate.lock().await;
        let input_guard = record.guard;
        tokio::task::spawn_blocking(move || input_guard.stop())
            .await
            .map_err(join_error)??;
        Ok(())
    }
}

fn ensure_not_cancelled(context: &OperationContext, message: &str) -> RuntimeResult<()> {
    if context.cancellation.is_cancelled() {
        Err(RuntimeError::new("operationCancelled", message))
    } else {
        Ok(())
    }
}

fn session_not_found() -> RuntimeError {
    RuntimeError::new(
        "desktop_input_session_not_found",
        "input session was not found",
    )
}
