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

> [!IMPORTANT]
> `audit` can legitimately take tens of seconds to minutes on a large
> `Cargo.lock` (confirmed: ~47s for 117 dependencies). **Keep the connection
> open until you've read the response.** A client that closes stdin right
> after sending the request -- rather than keeping the pipe alive while it
> waits -- will silently lose the response with no error on either side.
> This was caught by real end-to-end testing: piping input then immediately
> closing stdin dropped a completed `audit` response every time, while
> keeping the pipe open past the call's runtime delivered it correctly. This
> is stdio-transport behavior, not something `capscan-mcp` controls, but it's
> sharp enough to catch someone writing their own client, so it's called out
> here explicitly.

## Real-world example

Running `audit` against this repo's own `Cargo.lock` (117 dependencies) via
the real MCP protocol turned up a genuine, non-obvious result: the `wasi`
crate's pending update removes 48 capability signals outright --

```
wasi   0.11.1+wasi-snapshot-preview1 -> 0.14.7+wasi-0.2.4   (-48 signals, +1 new dep)
```

`diff`-ing those two versions shows why: 0.11's `src/lib_generated.rs`
exposed 46 raw `unsafe fn` WASI Preview 1 syscall bindings (`args_get`,
`environ_get`, ...) plus an FFI block. The Preview 2 rewrite drops that
entire raw-unsafe surface from `wasi` itself in favor of the new `wasip2`
dependency. Exactly the kind of thing that's invisible in a normal
`cargo update` diff and easy for an agent to surface with one tool call.

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
