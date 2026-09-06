use chatcmd_mcp::{McpServer, RuntimeApi};
use chatcmd_runtime::{BoxFuture, DeviceDescriptor, OperationContext, RuntimeError, RuntimeResult};
use rmcp::ServiceExt as _;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
struct SmokeRuntime;

impl RuntimeApi for SmokeRuntime {
    fn call<'a>(
        &'a self,
        _tool: &'a str,
        _context: OperationContext,
        _arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async {
            Err(RuntimeError::new(
                "smoke_only",
                "catalog smoke server does not execute tools",
            ))
        })
    }

    fn local_device(&self) -> DeviceDescriptor {
        DeviceDescriptor {
            device_id: "catalog-smoke".to_owned(),
            machine_id: None,
            name: "Catalog Smoke".to_owned(),
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
                "subagents are disabled in catalog smoke",
            ))
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = McpServer::new(Arc::new(SmokeRuntime));
    server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?
        .waiting()
        .await?;
    Ok(())
}
