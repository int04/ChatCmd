use chatcmd_mcp::{
    AuthProvider, HttpSecurity, McpServer, OriginPolicy, RuntimeApi,
    axum_router_with_host_validation,
};
use chatcmd_runtime::{BoxFuture, DeviceDescriptor, OperationContext, RuntimeError, RuntimeResult};
use serde_json::Value;
use std::{io::Write as _, sync::Arc};

struct TokenAuth;

impl AuthProvider for TokenAuth {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move { Ok(token.to_owned()) })
    }
}

struct AllowOrigin;

impl OriginPolicy for AllowOrigin {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move {
            if origin == "https://allowed.example" {
                Ok(())
            } else {
                Err(RuntimeError::new("origin_denied", "origin is not allowed"))
            }
        })
    }
}

#[derive(Clone)]
struct IdentityRuntime;

impl RuntimeApi for IdentityRuntime {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            Ok(serde_json::json!({
                "tool": tool,
                "agentId": context.agent_id,
                "mcpSessionId": context.mcp_session_id,
                "conversationScopeId": context.conversation_scope_id,
                "arguments": arguments,
            }))
        })
    }

    fn local_device(&self) -> DeviceDescriptor {
        DeviceDescriptor {
            device_id: "http-identity-smoke".to_owned(),
            machine_id: None,
            name: "HTTP Identity Smoke".to_owned(),
            platform: std::env::consts::OS.to_owned(),
            os_version: String::new(),
            architecture: std::env::consts::ARCH.to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            online: true,
        }
    }

    fn fail_subagent<'a>(
        &'a self,
        _child_task_id: &'a str,
        _message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async { Ok(()) })
    }

    fn request_subagent_fallback<'a>(
        &'a self,
        _parent_context: &'a OperationContext,
        _registration: &'a Value,
        _delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async {
            Err(RuntimeError::new(
                "smoke_only",
                "subagents are disabled in HTTP identity smoke",
            ))
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(AllowOrigin));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(IdentityRuntime)),
        security,
        false,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("{}", listener.local_addr()?);
    std::io::stdout().flush()?;
    axum::serve(listener, router).await?;
    Ok(())
}
