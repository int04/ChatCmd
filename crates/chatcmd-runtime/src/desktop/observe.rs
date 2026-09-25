use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

impl DesktopControlService {
    pub async fn observe(
        &self,
        context: &OperationContext,
        request: DesktopObserveRequest,
    ) -> RuntimeResult<DesktopObservation> {
        let record = self.window(context, &request.window_id).await?;
        let target = self.session_target(&record).await?;
        self.observe_target(
            context,
            target,
            request.include_screenshot,
            request.include_elements,
        )
        .await
    }

    pub async fn element_act(
        &self,
        context: &OperationContext,
        request: DesktopElementActRequest,
    ) -> RuntimeResult<DesktopActionResult> {
        validate_element_action(&request.action)?;
        let (target, element) = {
            let mut state = self.state.lock().await;
            let observation = state
                .observations
                .get(&request.observation_id)
                .ok_or_else(|| {
                    RuntimeError::new(
                        "desktop_observation_not_found",
                        "observation is stale or was not found",
                    )
                })?;
            ensure_owner(&observation.owner, context, "desktop_observation_not_found")?;
            if observation.created_at.elapsed() > OBSERVATION_TTL {
                return Err(RuntimeError::new(
                    "desktop_observation_stale",
                    "observation expired; observe the window again",
                ));
            }
            let element = observation
                .elements
                .get(&request.element_id)
                .cloned()
                .ok_or_else(|| {
                    RuntimeError::new(
                        "desktop_element_not_found",
                        "element was not found in that observation",
                    )
                })?;
            let target = observation.target.clone();
            state.observations.remove(&request.observation_id);
            (target, element)
        };
        let session = target.session.clone();
        let capture_after = request.observe_after && request.include_screenshot;
        let action = request.action;
        let capture_sequence = tokio::task::spawn_blocking(move || {
            let sequence = capture_after
                .then(|| session.current_capture_sequence())
                .transpose()?;
            session.element_act(&element, &action)?;
            Ok::<_, RuntimeError>(sequence)
        })
        .await
        .map_err(join_error)??;
        let (observation, verification_warning) = if request.observe_after {
            match self
                .observe_target_after(
                    context,
                    target,
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
        } else {
            (None, None)
        };
        Ok(DesktopActionResult {
            completed: true,
            observation_invalidated: true,
            observation,
            verification_warning,
        })
    }

    pub(super) async fn observe_target(
        &self,
        context: &OperationContext,
        target: SessionTarget,
        include_screenshot: bool,
        include_elements: bool,
    ) -> RuntimeResult<DesktopObservation> {
        self.observe_target_after(context, target, include_screenshot, include_elements, None)
            .await
    }

    pub(super) async fn observe_target_after(
        &self,
        context: &OperationContext,
        target: SessionTarget,
        include_screenshot: bool,
        include_elements: bool,
        after_sequence: Option<u64>,
    ) -> RuntimeResult<DesktopObservation> {
        let session = target.session.clone();
        let observed = tokio::task::spawn_blocking(move || match after_sequence {
            Some(sequence) => {
                session.observe_after(include_screenshot, include_elements, Some(sequence))
            }
            None => session.observe(include_screenshot, include_elements),
        })
        .await
        .map_err(join_error)??;
        let observation_id = uuid::Uuid::new_v4().to_string();
        let mut public_elements = Vec::with_capacity(observed.elements.len());
        let mut native_elements = HashMap::with_capacity(observed.elements.len());
        for (element, native) in observed.elements {
            native_elements.insert(element.element_id.clone(), native);
            public_elements.push(element);
        }
        let screenshot_base64 = observed.screenshot_png.map(|bytes| STANDARD.encode(bytes));
        let mut state = self.state.lock().await;
        prune_observations(&mut state);
        state.observations.insert(
            observation_id.clone(),
            ObservationRecord {
                owner: owner(context),
                target: target.clone(),
                created_at: Instant::now(),
                elements: native_elements,
            },
        );
        Ok(DesktopObservation {
            observation_id,
            window: target.info,
            elements: public_elements,
            elements_truncated: observed.elements_truncated,
            screenshot_base64,
        })
    }
}
