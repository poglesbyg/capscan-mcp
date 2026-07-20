# capscan-mcp

An [MCP](https://modelcontextprotocol.io) server that exposes
[capscan](https://crates.io/crates/capscan)'s crate capability scanning as
tools any AI coding agent can call directly — so an agent can check what a
dependency actually does (unsafe, FFI, process/network/filesystem access,
build scripts) before recommending or applying a version bump, instead of
trusting a `Cargo.toml` edit blind.

## Install

```
cargo install --path .
```

This builds the `capscan-mcp` binary, which speaks MCP over stdio.

## Register it with an MCP client

Generic `mcpServers` config (Claude Desktop, Cursor, etc.):

```json
{
  "mcpServers": {
    "capscan": {
      "command": "capscan-mcp"
    }
  }
}
```

Claude Code CLI:

```
claude mcp add capscan -- capscan-mcp
```

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `scan` | `name`, `version` | Capability signals for one published crate version. |
| `diff` | `name`, `old_version`, `new_version` | What capabilities/dependencies a version bump would add or remove. Use this before recommending or applying one. |
| `audit` | `lockfile_path` (absolute path) | Checks every crates.io dependency in a `Cargo.lock` against its latest published version; can take tens of seconds on large lockfiles since it fetches real crate sources via `cargo`. |

All three return the same JSON shapes as `capscan`'s own `--json` CLI output
(`CrateReport`, `Diff`, `Vec<AuditEntry>`), rendered as text content in the
tool result.

## How it's built

A thin wrapper: three `#[tool]`-annotated methods on a
[`rmcp`](https://crates.io/crates/rmcp) `ServerHandler`, each calling straight
into the `capscan` library crate (`locate_or_fetch`, `scan_dir`,
`diff_reports`, `audit_project`) inside `tokio::task::spawn_blocking`, since
those do real subprocess/filesystem I/O and shouldn't block the async
runtime. All the actual scanning/diffing logic lives in `capscan` — this
crate is just protocol glue.

## Tests

```
cargo test              # protocol-level tests against the real compiled binary, no network
cargo test -- --ignored # also calls `diff` for real against anyhow on crates.io
```

`tests/protocol.rs` spawns the actual compiled `capscan-mcp` binary and talks
real JSON-RPC to it over stdio (initialize, `tools/list`, `tools/call`) —
the same wire format any MCP client uses — rather than reaching into rmcp's
internal client API.

## License

MIT OR Apache-2.0
