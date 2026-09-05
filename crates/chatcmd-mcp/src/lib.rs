//! Official `rmcp` server surface for direct local ChatCMD execution.

// RuntimeError is a shared structured API error; keep its established representation.
#![allow(clippy::result_large_err)]

mod request_identity;
mod server_contract;
mod subagent_protocol;
mod subagent_worker;
mod tool_catalog;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use chatcmd_runtime::{
    BoxFuture, DeviceDescriptor, OperationContext, RuntimeError, RuntimeResult, ShellInputKind,
};
use rmcp::{
    Peer, RoleServer,
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars,
    service::RequestContext,
    tool, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tower::ServiceExt;

use server_contract::error_value;

pub use tool_catalog::{
    CATALOG_VERSION, CatalogMetadata, PROTOCOL_VERSION, PathFieldRole, TOOL_NAMES,
    ToolCapabilityFlags, ToolOperationClass, ToolRiskClass, canonical_manifest, catalog_hash,
    catalog_metadata, instructions_hash, tool_capabilities,
};

/// Path-token authentication dependency injected by the HTTP host.
pub trait AuthProvider: Send + Sync {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>>;
}

/// Origin allow-list dependency injected by the HTTP host.
pub trait OriginPolicy: Send + Sync {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>>;
}

/// Runtime dispatch boundary. Implementations inject policy, task, device, and local runtime services.
pub trait RuntimeApi: Send + Sync {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>>;

    fn local_device(&self) -> DeviceDescriptor;

    fn project_folder<'a>(
        &'a self,
        _task_id: Option<&'a str>,
    ) -> BoxFuture<'a, RuntimeResult<Option<String>>> {
        Box::pin(async { Ok(None) })
    }

    fn fail_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>>;

    fn heartbeat_subagent<'a>(
        &'a self,
        _child_task_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn request_subagent_fallback<'a>(
        &'a self,
        parent_context: &'a OperationContext,
        registration: &'a Value,
        delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>>;
}

include!("tool_args/common.rs");
include!("tool_args/basic.rs");
include!("tool_args/filesystem.rs");
include!("tool_args/git_agent.rs");
include!("mcp_server.rs");
include!("tool_methods.rs");
include!("http_transport.rs");

#[cfg(test)]
mod lib_tests;

#[cfg(test)]
mod c07_tests;
