---
name: capscan-audit
version: 0.1.0
description: |
  Check a Rust crate's actual capability surface (unsafe code, FFI, process/
  network/filesystem access, build scripts, proc-macros) before recommending
  or applying a Cargo.toml dependency version bump, adding a new dependency,
  or running `cargo update`. Backed by capscan/capscan-mcp: scan a single
  version, diff two versions, or audit an entire Cargo.lock against latest
  published versions. Use when asked to "update a dependency", "bump this
  crate", "run cargo update", "add this crate", "is this dependency safe",
  "audit my Cargo.lock", or before committing any Cargo.toml/Cargo.lock
  change in a Rust project.
  Proactively invoke this skill (do NOT silently edit Cargo.toml/Cargo.lock)
  whenever you are about to change what version of a crate a Rust project
  depends on, add a brand new dependency, or a user asks whether a crate is
  safe to use or update.
allowed-tools:
  - Bash
  - Read
---

# capscan-audit

Before changing what a Rust project depends on -- bumping a version,
adding a crate, or running `cargo update` -- check what the change
actually does, not just what version number it is. A `Cargo.toml` diff
never shows you that an update added `unsafe`, FFI, a `build.rs`, or a
new transitive dependency; capscan does.

## Which interface to use

**Prefer the MCP tools if connected.** Look through your available tools
for ones named like `mcp__<server>__scan`, `mcp__<server>__diff`, and
`mcp__<server>__audit` whose descriptions mention capability surface /
unsafe / FFI / build scripts -- that's `capscan-mcp`, regardless of what
name the user registered it under. Use those directly; they return the
same JSON described below.

**Fall back to the CLI** if no such MCP tools are connected:

```bash
# is it installed?
cargo capscan --help

# not installed? offer to install it (ask first, per your normal rules
# around installing things) rather than doing it silently:
cargo install capscan
```

## When to actually call it

- **Bumping one dependency's version** (in `Cargo.toml`, or reviewing a
  Dependabot/Renovate PR): call `diff` for that crate, old version ->
  new version, before making or endorsing the change.
  ```bash
  cargo capscan diff SERDE 1.0.200 1.0.229
  ```
- **Adding a brand-new dependency**: call `scan` on the version you're
  about to pin, so anything it already does (build.rs, FFI, native
  linkage) is visible before it's ever a "new" finding to someone else.
  ```bash
  cargo capscan scan SOME_NEW_CRATE 1.4.0
  ```
- **Running (or about to run) `cargo update`, or asked to audit
  dependencies generally**: call `audit` against the project's
  `Cargo.lock`. This can take tens of seconds to minutes on large
  lockfiles -- it resolves and fetches real crate sources via `cargo`.
  ```bash
  cargo capscan audit --lockfile ./Cargo.lock --min-severity medium
  ```
  Pass `min_severity`/`--min-severity` (`"low"`/`"medium"`/`"high"`, both
  the MCP tool and the CLI support it) to skip up-to-date dependencies in
  the result -- use it, especially on large lockfiles, so you're not
  wading through noise to find the handful of findings that matter. It
  only filters what's shown/returned; the CLI's exit code still reflects
  the true worst severity across every dependency regardless of the
  filter, so it's safe to use in a CI gate.
  The MCP tool also emits progress notifications during the run if your
  client requested a `progressToken` -- surface those to the user on
  long audits instead of going silent.

## Reading the result and what to actually do with it

Every result carries a severity per signal: `high` (unsafe fn/impl, FFI,
process spawn, `build.rs`, proc-macro crate, native linkage,
`mem::transmute`, exported symbol), `medium` (`unsafe` block, network
access, filesystem write, env write, new transitive dependency), `low`
(env read, build-time macros).

- **New `high` severity signal**: don't apply or recommend the change
  silently. Tell the user specifically what appeared (kind, file, detail)
  and let them decide -- this is exactly the kind of thing a normal
  `cargo update` hides.
- **New `medium` severity signal, or new transitive dependencies**:
  mention it in your summary of the change; less urgent than `high`, but
  still worth a sentence.
- **Nothing new, or only `low`**: proceed normally, no need to call it
  out at length.

This is informational, not a gate you enforce unilaterally -- surface
what you found and let the user decide, the same way you would for any
other risk you noticed while doing the work they asked for.

## Limitations worth knowing

capscan's detection is heuristic AST matching, not real type resolution
(it can't see through re-exported aliases, and a user type also named
`Command` would false-positive as a process spawn). It's a fast triage
signal, not a proof of safety -- treat a clean result as "nothing obvious
showed up," not "this dependency is safe."
