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

fn parse_severity(s: &str) -> Result<capscan::Severity, McpError> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Ok(capscan::Severity::Low),
        "medium" => Ok(capscan::Severity::Medium),
        "high" => Ok(capscan::Severity::High),
        other => Err(McpError::invalid_params(
            format!("invalid min_severity '{other}' (expected 'low', 'medium', or 'high')"),
            None,
        )),
    }
}

/// Keep only entries whose worst new capability is at least `min_severity`
/// -- entries with no diff at all (already at latest) never pass a filter,
/// since there's nothing to report for them. `None` means no filtering.
fn filter_by_min_severity(
    entries: Vec<capscan::AuditEntry>,
    min_severity: Option<&str>,
) -> Result<Vec<capscan::AuditEntry>, McpError> {
    let Some(min_severity) = min_severity else {
        return Ok(entries);
    };
    let threshold = parse_severity(min_severity)?;
    Ok(entries
        .into_iter()
        .filter(|e| e.worst_severity().is_some_and(|sev| sev >= threshold))
        .collect())
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
    #[schemars(
        description = "Only include dependencies whose worst new capability is at least this severity: \"low\", \"medium\", or \"high\". Omit to include every dependency, including ones already at latest."
    )]
    #[serde(default)]
    pub min_severity: Option<String>,
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
        description = "Audit every crates.io dependency in a Cargo.lock against its latest published version, and report which ones would gain new capabilities if updated. Can take tens of seconds to minutes on large lockfiles -- it resolves and fetches real crate sources via cargo. Pass min_severity (\"low\"/\"medium\"/\"high\") to only get back dependencies that actually found something, instead of every up-to-date dependency too."
    )]
    async fn audit(
        &self,
        Parameters(AuditRequest {
            lockfile_path,
            min_severity,
        }): Parameters<AuditRequest>,
    ) -> Result<CallToolResult, McpError> {
        let entries =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<capscan::AuditEntry>> {
                capscan::audit_project(Path::new(&lockfile_path))
            })
            .await
            .map_err(to_mcp_err)?
            .map_err(to_mcp_err)?;

        let entries = filter_by_min_severity(entries, min_severity.as_deref())?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use capscan::{AuditEntry, Diff, Severity, Signal, SignalKind};

    fn entry(name: &str, worst: Option<Severity>) -> AuditEntry {
        let diff = worst.map(|sev| {
            let kind = match sev {
                Severity::Low => SignalKind::EnvRead,
                Severity::Medium => SignalKind::UnsafeBlock,
                Severity::High => SignalKind::UnsafeFn,
            };
            Diff {
                old: (name.to_string(), "1.0.0".to_string()),
                new: (name.to_string(), "2.0.0".to_string()),
                added: vec![Signal {
                    kind,
                    file: "src/lib.rs".to_string(),
                    line: 1,
                    detail: "x".to_string(),
                }],
                removed: vec![],
                added_dependencies: vec![],
                removed_dependencies: vec![],
            }
        });
        AuditEntry {
            name: name.to_string(),
            locked_version: "1.0.0".to_string(),
            latest_version: if worst.is_some() { "2.0.0" } else { "1.0.0" }.to_string(),
            diff,
        }
    }

    #[test]
    fn min_severity_filters_out_low_and_up_to_date() {
        let entries = vec![
            entry("up-to-date", None),
            entry("low-only", Some(Severity::Low)),
            entry("medium-hit", Some(Severity::Medium)),
            entry("high-hit", Some(Severity::High)),
        ];

        let filtered = filter_by_min_severity(entries, Some("medium")).unwrap();
        let names: Vec<&str> = filtered.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["medium-hit", "high-hit"]);
    }

    #[test]
    fn no_min_severity_returns_everything_unfiltered() {
        let entries = vec![entry("a", None), entry("b", Some(Severity::High))];
        let filtered = filter_by_min_severity(entries.clone(), None).unwrap();
        assert_eq!(filtered.len(), entries.len());
    }

    #[test]
    fn min_severity_is_case_insensitive() {
        let entries = vec![entry("a", Some(Severity::High))];
        let filtered = filter_by_min_severity(entries, Some("HIGH")).unwrap();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn invalid_min_severity_is_rejected_with_a_useful_message() {
        let entries = vec![entry("a", Some(Severity::High))];
        let err = filter_by_min_severity(entries, Some("critical")).unwrap_err();
        assert!(err.message.contains("critical"));
    }
}
