# capscan-mcp

An [MCP](https://modelcontextprotocol.io) server that exposes
[capscan](https://crates.io/crates/capscan)'s crate capability scanning as
tools any AI coding agent can call directly — so an agent can check what a
dependency actually does (unsafe, FFI, process/network/filesystem access,
build scripts) before recommending or applying a version bump, instead of
trusting a `Cargo.toml` edit blind.

## Install

Prebuilt binary, no Rust toolchain needed -- macOS, Linux, and Windows,
from the [latest release](https://github.com/poglesbyg/capscan-mcp/releases/latest):

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/poglesbyg/capscan-mcp/releases/latest/download/capscan-mcp-installer.sh | sh
```

(PowerShell on Windows: see the install command on the
[release page](https://github.com/poglesbyg/capscan-mcp/releases/latest).)

Or build from source:

```
cargo install capscan-mcp
```

or, from a checkout of this repo: `cargo install --path .`. Either way this
builds the `capscan-mcp` binary, which speaks MCP over stdio.

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

## Claude Code skill

Registering the server gets you the tools; [`skills/capscan-audit`](skills/capscan-audit)
gets an agent to actually *reach for them* -- before bumping a dependency
version, adding a new one, or running `cargo update`, instead of only when
you happen to ask. Install it once:

```bash
# available in every project on this machine
cp -r skills/capscan-audit ~/.claude/skills/

# or scoped to just this project
mkdir -p .claude/skills && cp -r skills/capscan-audit .claude/skills/
```

It works even without the MCP server connected -- it falls back to the
`cargo capscan` CLI (`cargo install capscan` if it isn't already) -- but
prefers the MCP tools when they're available so results come back
structured rather than as text to re-parse.

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `scan` | `name`, `version` | Capability signals for one published crate version. |
| `diff` | `name`, `old_version`, `new_version` | What capabilities/dependencies a version bump would add or remove. Use this before recommending or applying one. |
| `audit` | `lockfile_path` (absolute path), `min_severity` (optional: `"low"`/`"medium"`/`"high"`) | Checks every crates.io dependency in a `Cargo.lock` against its latest published version; can take tens of seconds to minutes on large lockfiles since it fetches real crate sources via `cargo`. Pass `min_severity` to only get back dependencies that found something at or above that severity, instead of every up-to-date one too -- a 117-dependency lockfile is a 28KB response otherwise, almost all of it "nothing to report." Reports MCP progress notifications as it works if your request includes a `progressToken` -- see below. |

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

## Progress notifications

`audit`'s ~47s runtime is a long time for a client to sit in silence, so it
reports real MCP progress notifications as it works -- if you include a
`progressToken` in your request's `_meta`, you'll get one notification per
dependency as its latest version resolves, then one per out-of-date
dependency as it gets diffed, e.g.:

```
notifications/progress  1/116  resolved latest versions: 1/116
notifications/progress  2/116  resolved latest versions: 2/116
...
notifications/progress  1/8    diffed out-of-date dependencies: 1/8 (r-efi)
...
```

If your request has no `progressToken`, `audit` behaves exactly as before
-- silent until the final result. This is opt-in, not a behavior change.

The concurrency behind this was tuned by watching it happen, not by
guessing. The first version fired one lookup per dependency at once (116
concurrent `spawn_blocking` tasks for this repo's own lockfile) and the
progress notifications immediately showed why that's wrong: progress sat
at 1-2/116 for about 47 seconds while every lookup piled onto cargo's own
registry-index lock simultaneously, then jumped to 116/116 in under 2
seconds once the contention cleared -- a curve that looks stalled for
almost the entire call. Capping concurrency at 16 (same cap and rationale
as `capscan`'s own `MAX_VERSION_LOOKUP_WORKERS`) turned that into a steady
cadence of ~16 completions every 5-6 seconds and cut total time from
~49s to ~42s in the same test. Bounded concurrency isn't just gentler on
cargo's lock, it's the difference between a progress bar that looks broken
and one that looks like it's actually doing something.

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

Filtered to `min_severity: "medium"`, that same audit drops the 109
up-to-date dependencies entirely and returns only the handful actually
worth an agent's attention.

## How it's built

Three `#[tool]`-annotated methods on a [`rmcp`](https://crates.io/crates/rmcp)
`ServerHandler`. `scan` and `diff` call straight into the `capscan` library
crate (`locate_or_fetch`, `scan_dir`, `diff_reports`) inside
`tokio::task::spawn_blocking`, since those do real subprocess/filesystem I/O
and shouldn't block the async runtime. `audit` reimplements that same
orchestration itself -- using capscan's individual public functions
(`parse_lockfile`, `latest_version`, `locate_or_fetch`, `scan_dir`,
`diff_reports`) instead of calling capscan's own all-in-one
`audit_project` -- specifically so it has a point to hook progress
notifications into; `audit_project`'s parallelism is internal
`std::thread::scope` with no way to observe it from outside as it runs.
All the actual scanning/diffing logic still lives in `capscan` either way —
this crate is protocol glue plus, for `audit`, its own progress-aware
scheduling on top.

## Tests

```
cargo test              # unit tests (min_severity filtering) + protocol tests, no network
cargo test -- --ignored # also calls `diff` and `audit` for real against crates.io
```

`tests/protocol.rs` spawns the actual compiled `capscan-mcp` binary and talks
real JSON-RPC to it over stdio (initialize, `tools/list`, `tools/call`) —
the same wire format any MCP client uses — rather than reaching into rmcp's
internal client API. Its `call_server` helper deliberately keeps stdin open
until every expected response has arrived before closing it, per the
stdin-lifecycle note above; the ignored `audit` test is the same
~30-90s-long-call scenario that caught that bug in the first place, now
covered as a regression test instead of a one-off manual finding.

## License

MIT OR Apache-2.0
