/// Fail-closed HTTP security state.
#[derive(Clone)]
pub struct HttpSecurity {
    auth: Arc<dyn AuthProvider>,
    origins: Arc<dyn OriginPolicy>,
}

impl HttpSecurity {
    #[must_use]
    pub fn new(auth: Arc<dyn AuthProvider>, origins: Arc<dyn OriginPolicy>) -> Self {
        Self { auth, origins }
    }

    async fn authorize(
        &self,
        token: &str,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> RuntimeResult<String> {
        if query.is_some_and(has_query_token) {
            return Err(RuntimeError::new(
                "query_token_rejected",
                "authentication token in query is forbidden",
            ));
        }
        if token.is_empty() || token.len() > 512 {
            return Err(RuntimeError::new(
                "unauthorized",
                "valid MCP path token is required",
            ));
        }
        let agent_id = self.auth.authorize(token).await?;
        self.origins
            .authorize(
                headers
                    .get("origin")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
            )
            .await?;
        Ok(agent_id)
    }
}

fn has_query_token(query: &str) -> bool {
    query.split('&').any(|pair| {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        matches!(
            name.to_ascii_lowercase().as_str(),
            "token" | "access_token" | "bearer_token"
        )
    })
}

#[derive(Clone)]
struct McpHttpState {
    security: HttpSecurity,
    service: StreamableHttpService<McpServer, LocalSessionManager>,
    session_manager: Arc<LocalSessionManager>,
    session_owners: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
}

async fn mcp_handler(
    State(state): State<McpHttpState>,
    Path(token): Path<String>,
    mut request: Request<Body>,
) -> Response {
    let agent_id = match state
        .security
        .authorize(&token, request.headers(), request.uri().query())
        .await
    {
        Ok(agent_id) => agent_id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if request.method() == http::Method::POST
        && let Some(content_length) = request
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
        && content_length > request_identity::MCP_CONTROL_BODY_BYTES as u64
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }

    let terminate_session = request.method() == http::Method::DELETE;
    let header_session = request
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Some(remote_session) = header_session.as_deref() {
        let session_exists = state
            .session_manager
            .sessions
            .read()
            .await
            .contains_key(remote_session);
        if !session_exists {
            state.session_owners.write().await.remove(remote_session);
        }
        if state
            .session_owners
            .read()
            .await
            .get(remote_session)
            .is_some_and(|owner| owner != &agent_id)
        {
            return StatusCode::NOT_FOUND.into_response();
        }
    }
    let session_id = local_mcp_session_id(&agent_id, header_session.as_deref());
    let metadata = catalog_metadata();
    tracing::info!(
        target: "chatcmd_mcp_catalog",
        app_version = %metadata.app_version,
        protocol_version = metadata.protocol_version,
        catalog_version = metadata.catalog_version,
        catalog_hash = %metadata.catalog_hash,
        build_id = %metadata.build_id,
        transport = "streamable_http",
        agent_id = %agent_id,
        mcp_session_id = %session_id,
        "mcp_session_catalog"
    );

    // rmcp forwards the original HTTP parts through RequestContext, including this
    // server-owned extension. The request body remains untouched and is parsed once.
    request_identity::bind_authenticated_context(
        &mut request,
        request_identity::AuthenticatedMcpContext::new(agent_id.clone(), session_id),
    );

    // Keep the credential at the HTTP boundary. Downstream rmcp handlers receive
    // a stable credential-free URI and the authenticated agent identity only.
    *request.uri_mut() = Uri::from_static("/mcp");
    match state.service.clone().oneshot(request).await {
        Ok(response) => {
            if let Some(remote_session) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
            {
                state
                    .session_owners
                    .write()
                    .await
                    .insert(remote_session.to_owned(), agent_id.clone());
            }
            if terminate_session
                && response.status().is_success()
                && let Some(remote_session) = header_session.as_deref()
            {
                state.session_owners.write().await.remove(remote_session);
            }
            response.into_response()
        }
        Err(infallible) => match infallible {},
    }
}

async fn catalog_handler(
    State(state): State<McpHttpState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if state
        .security
        .authorize(&token, &headers, uri.query())
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(serde_json::json!({
        "metadata": catalog_metadata(),
        "manifest": canonical_manifest()
    }))
    .into_response()
}

fn local_mcp_session_id(agent_id: &str, header_session: Option<&str>) -> String {
    let scope = header_session.unwrap_or("agent-fallback");
    let material = format!("agent:{agent_id}\0session:{scope}");
    format!(
        "mcp-session-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

/// Build reusable Streamable HTTP service with local rmcp session management.
pub fn streamable_http_service(
    server: McpServer,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    streamable_http_service_with_config_and_manager(
        server,
        StreamableHttpServerConfig::default(),
        Arc::new(LocalSessionManager::default()),
    )
}

fn streamable_http_service_with_config_and_manager(
    server: McpServer,
    config: StreamableHttpServerConfig,
    session_manager: Arc<LocalSessionManager>,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    let config = config.with_max_request_body_bytes(request_identity::MCP_CONTROL_BODY_BYTES);
    StreamableHttpService::new(move || Ok(server.clone()), session_manager, config)
}

/// Build an Axum router protected by a token path segment and Origin checks.
pub fn axum_router(server: McpServer, security: HttpSecurity) -> Router {
    axum_router_with_host_validation(server, security, true)
}

/// Build an Axum router while optionally disabling rmcp Host validation.
///
/// Host validation should only be disabled when the listener itself is loopback-only
/// and an external reverse proxy is the sole public ingress. Token and Origin checks
/// remain active at the ChatCmdClient boundary.
pub fn axum_router_with_host_validation(
    server: McpServer,
    security: HttpSecurity,
    validate_host: bool,
) -> Router {
    let config = if validate_host {
        StreamableHttpServerConfig::default()
    } else {
        StreamableHttpServerConfig::default().disable_allowed_hosts()
    };
    let session_manager = Arc::new(LocalSessionManager::default());
    let state = McpHttpState {
        security,
        service: streamable_http_service_with_config_and_manager(
            server,
            config,
            session_manager.clone(),
        ),
        session_manager,
        session_owners: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    };
    Router::new()
        .route("/mcp/{token}", any(mcp_handler))
        .route("/mcp/{token}/catalog", get(catalog_handler))
        .with_state(state)
}
