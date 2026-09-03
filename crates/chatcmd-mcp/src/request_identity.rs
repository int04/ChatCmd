use axum::{body::Body, extract::Request};
use rmcp::{RoleServer, service::RequestContext};

pub(crate) const MCP_CONTROL_BODY_BYTES: usize = 4 * 1024 * 1024;
const OPENAI_SESSION_META_KEY: &str = "openai/session";
const MAX_CONVERSATION_IDENTITY_BYTES: usize = 4096;

/// Server-owned identity propagated by rmcp from HTTP request extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthenticatedMcpContext {
    pub(crate) agent_id: String,
    pub(crate) local_session_id: String,
    pub(crate) conversation_scope_id: Option<String>,
}

impl AuthenticatedMcpContext {
    pub(crate) fn new(agent_id: String, local_session_id: String) -> Self {
        Self {
            agent_id,
            local_session_id,
            conversation_scope_id: None,
        }
    }
}

/// Attach trusted identity without reading, parsing, or replacing the body.
pub(crate) fn bind_authenticated_context(
    request: &mut Request<Body>,
    context: AuthenticatedMcpContext,
) {
    request.extensions_mut().insert(context);
}

/// Recover HTTP identity after rmcp moves the request into its session worker.
/// rmcp stores the original request parts in the tool request context.
pub(crate) fn authenticated_context(
    request_context: &RequestContext<RoleServer>,
) -> Option<AuthenticatedMcpContext> {
    let parts = request_context.extensions.get::<http::request::Parts>()?;
    let mut context = parts.extensions.get::<AuthenticatedMcpContext>()?.clone();
    context.conversation_scope_id = conversation_scope(&request_context.meta);
    Some(context)
}

/// Non-HTTP transports have no authentication middleware. Preserve the local
/// in-process transport contract without allowing this fallback on HTTP.
pub(crate) fn local_transport_context(
    request_context: &RequestContext<RoleServer>,
    agent_id: &str,
) -> Option<AuthenticatedMcpContext> {
    if request_context
        .extensions
        .get::<http::request::Parts>()
        .is_some()
        || agent_id.trim().is_empty()
    {
        return None;
    }
    Some(AuthenticatedMcpContext::new(
        agent_id.to_owned(),
        format!(
            "mcp-session-{}",
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("local-transport\0{agent_id}").as_bytes(),
            )
        ),
    ))
}

fn conversation_scope(meta: &rmcp::model::RequestMetaObject) -> Option<String> {
    let raw = meta.get(OPENAI_SESSION_META_KEY)?.as_str()?.trim();
    if raw.is_empty() || raw.len() > MAX_CONVERSATION_IDENTITY_BYTES {
        return None;
    }
    let material = format!("openai\0{raw}");
    Some(format!(
        "openai:{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, material.as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn binding_preserves_request_body_byte_for_byte() {
        let original =
            br#"{ "jsonrpc": "2.0", "params": { "arguments": { "agentId": "spoof" } } }"#;
        let mut request = Request::post("/mcp/token")
            .body(Body::from(original.as_slice()))
            .expect("request");

        bind_authenticated_context(
            &mut request,
            AuthenticatedMcpContext::new("agent-safe".to_owned(), "session-safe".to_owned()),
        );

        assert_eq!(
            to_bytes(request.into_body(), original.len())
                .await
                .expect("body"),
            original.as_slice()
        );
    }

    #[test]
    fn conversation_identity_is_bounded_and_fingerprinted() {
        let mut first = rmcp::model::RequestMetaObject::default();
        first.insert(
            OPENAI_SESSION_META_KEY.to_owned(),
            serde_json::Value::String("chat-a".to_owned()),
        );
        let mut second = first.clone();
        second.insert(
            OPENAI_SESSION_META_KEY.to_owned(),
            serde_json::Value::String("chat-b".to_owned()),
        );

        let first_scope = conversation_scope(&first).expect("first scope");
        assert!(first_scope.starts_with("openai:"));
        assert_ne!(
            first_scope,
            conversation_scope(&second).expect("second scope")
        );

        first.insert(
            OPENAI_SESSION_META_KEY.to_owned(),
            serde_json::Value::String("x".repeat(MAX_CONVERSATION_IDENTITY_BYTES + 1)),
        );
        assert!(conversation_scope(&first).is_none());
    }
}
