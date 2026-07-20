//! MCP tool wrapper around the `capscan` library: lets an AI coding agent
//! check a crate's capability surface (unsafe, FFI, process/network/fs
//! access, build scripts) before recommending or applying a dependency
//! change, instead of trusting a version bump blind.

use std::path::Path;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

fn to_mcp_err(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value).map_err(to_mcp_err)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScanRequest {
    #[schemars(description = "Crate name on crates.io, e.g. \"anyhow\"")]
    pub name: String,
    #[schemars(description = "Exact published version, e.g. \"1.0.104\"")]
    pub version: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DiffRequest {
    #[schemars(description = "Crate name on crates.io, e.g. \"anyhow\"")]
    pub name: String,
    #[schemars(description = "Version currently in use / locked in Cargo.lock")]
    pub old_version: String,
    #[schemars(description = "Version being considered, e.g. the latest release")]
    pub new_version: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AuditRequest {
    #[schemars(description = "Absolute path to the Cargo.lock file to audit")]
    pub lockfile_path: String,
}

#[derive(Clone)]
pub struct CapscanTools {
    // Read by the #[tool_handler]-generated ServerHandler methods (list_tools/
    // call_tool) through macro glue that rustc's dead-code pass doesn't trace
    // back to this field -- verified functionally: a live tools/list and
    // tools/call over stdio both correctly reflect the router built here.
    #[allow(dead_code)]
    tool_router: ToolRouter<CapscanTools>,
}

impl Default for CapscanTools {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl CapscanTools {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Scan a single published crate version and report its capability surface: unsafe code, FFI, process spawns, network/filesystem access, build scripts, proc-macro crates, and native linkage."
    )]
    async fn scan(
        &self,
        Parameters(ScanRequest { name, version }): Parameters<ScanRequest>,
    ) -> Result<CallToolResult, McpError> {
        let report =
            tokio::task::spawn_blocking(move || -> anyhow::Result<capscan::CrateReport> {
                let path = capscan::locate_or_fetch(&name, &version)?;
                capscan::scan_dir(&name, &version, &path)
            })
            .await
            .map_err(to_mcp_err)?
            .map_err(to_mcp_err)?;

        json_result(&report)
    }

    #[tool(
        description = "Diff a crate's capability surface between two published versions -- what new unsafe/FFI/process/network/build-script capabilities (and new transitive dependencies) updating from old_version to new_version would introduce. Use this before recommending or applying a dependency version bump."
    )]
    async fn diff(
        &self,
        Parameters(DiffRequest {
            name,
            old_version,
            new_version,
        }): Parameters<DiffRequest>,
    ) -> Result<CallToolResult, McpError> {
        let diff = tokio::task::spawn_blocking(move || -> anyhow::Result<capscan::Diff> {
            let old_path = capscan::locate_or_fetch(&name, &old_version)?;
            let new_path = capscan::locate_or_fetch(&name, &new_version)?;
            let old_report = capscan::scan_dir(&name, &old_version, &old_path)?;
            let new_report = capscan::scan_dir(&name, &new_version, &new_path)?;
            Ok(capscan::diff_reports(&old_report, &new_report))
        })
        .await
        .map_err(to_mcp_err)?
        .map_err(to_mcp_err)?;

        json_result(&diff)
    }

    #[tool(
        description = "Audit every crates.io dependency in a Cargo.lock against its latest published version, and report which ones would gain new capabilities if updated. Can take tens of seconds on large lockfiles -- it resolves and fetches real crate sources via cargo."
    )]
    async fn audit(
        &self,
        Parameters(AuditRequest { lockfile_path }): Parameters<AuditRequest>,
    ) -> Result<CallToolResult, McpError> {
        let entries =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<capscan::AuditEntry>> {
                capscan::audit_project(Path::new(&lockfile_path))
            })
            .await
            .map_err(to_mcp_err)?
            .map_err(to_mcp_err)?;

        json_result(&entries)
    }
}

#[tool_handler]
impl ServerHandler for CapscanTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "Tools for checking a Rust crate's capability surface (unsafe, FFI, \
                 process/network/filesystem access, build scripts, proc-macros) before \
                 trusting or applying a dependency change. Use `diff` before recommending \
                 a version bump for one crate. Use `audit` to check an entire project's \
                 Cargo.lock against latest published versions at once. Use `scan` to \
                 inspect a single version on its own."
                    .to_string(),
            )
    }
}
