//! Exercises the compiled `capscan-mcp` binary over real stdio, the same
//! way any MCP client would, rather than reaching into rmcp's lower-level
//! client-side request API.
//!
//! `call_server` keeps stdin open until every expected response has
//! arrived, then closes it. That's deliberate: real end-to-end testing
//! against the audit tool found that closing stdin right after writing the
//! request (the "write everything, then wait_with_output" pattern this file
//! used to use) silently drops the response to any request still in flight
//! -- no error either side, just nothing written. Fast calls like `diff`
//! happened to finish before that race triggered, which is exactly why it
//! went unnoticed until a ~47s `audit` call exposed it. See the README's
//! stdin-lifecycle note.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// Sends each of `requests` as one JSON-RPC line, then reads back exactly
/// one response line per request that has an "id" (notifications get none)
/// before closing stdin -- see the module docs for why that order matters.
fn call_server(requests: &[&str]) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_capscan-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start capscan-mcp");

    let expected_responses = requests
        .iter()
        .filter(|req| {
            serde_json::from_str::<serde_json::Value>(req)
                .expect("test wrote invalid JSON")
                .get("id")
                .is_some()
        })
        .count();

    let mut stdin = child.stdin.take().expect("stdin was piped");
    for req in requests {
        writeln!(stdin, "{req}").expect("failed to write request");
    }
    // `stdin` stays alive (not dropped) across the read loop below -- closing
    // it before every expected response has arrived is the bug this harness
    // exists to not reproduce.

    let mut reader = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let mut responses = Vec::new();
    let mut line = String::new();
    while responses.len() < expected_responses {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .expect("failed to read a response line");
        assert!(
            bytes_read > 0,
            "server closed stdout after {} of {expected_responses} expected responses",
            responses.len()
        );
        responses.push(
            serde_json::from_str(line.trim())
                .unwrap_or_else(|e| panic!("bad JSON line {line:?}: {e}")),
        );
    }

    drop(stdin); // safe now: everything we asked for has already arrived

    let output = child
        .wait_with_output()
        .expect("server did not exit cleanly");
    assert!(
        output.status.success(),
        "server exited with {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    responses
}

#[test]
fn initialize_reports_instructions() {
    let responses = call_server(&[
        INITIALIZE,
        INITIALIZED,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ]);
    let instructions = responses[0]["result"]["instructions"]
        .as_str()
        .expect("initialize response should include instructions");
    assert!(instructions.contains("capability surface"));
}

#[test]
fn lists_all_three_tools() {
    let responses = call_server(&[
        INITIALIZE,
        INITIALIZED,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ]);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools/list should return a tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"scan"));
    assert!(names.contains(&"diff"));
    assert!(names.contains(&"audit"));
}

#[test]
#[ignore = "requires network access to crates.io"]
fn diff_tool_call_matches_known_result() {
    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"diff","arguments":{"name":"anyhow","old_version":"1.0.70","new_version":"1.0.104"}}}"#;
    let responses = call_server(&[INITIALIZE, INITIALIZED, call]);

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("tool call should return text content");
    let diff: serde_json::Value = serde_json::from_str(text).expect("tool text should be JSON");

    let added: Vec<&str> = diff["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["detail"].as_str().unwrap())
        .collect();
    assert!(added.contains(&"object_reallocate_boxed"));
    assert_eq!(
        diff["removed_dependencies"].as_array().unwrap(),
        &[serde_json::json!("backtrace")]
    );
}

#[test]
#[ignore = "requires network access to crates.io; audits this repo's own ~118-dependency \
            Cargo.lock over the real protocol, takes roughly 30-90s"]
fn audit_tool_call_on_own_lockfile_respects_min_severity() {
    // This is the exact scenario that caught the stdin-lifecycle bug: a
    // long-running `audit` call over the real compiled binary. Its
    // assertions depend on live crates.io state (whatever's out of date on
    // this lockfile right now) rather than a fixed fixture, so if this ever
    // starts failing because everything's been updated to latest, that's
    // an environment change, not a regression -- see the README's
    // real-world example for what it found as of this writing.
    let lockfile_path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock");
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"audit","arguments":{{"lockfile_path":"{lockfile_path}","min_severity":"medium"}}}}}}"#
    );
    let responses = call_server(&[INITIALIZE, INITIALIZED, &call]);

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("tool call should return text content");
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(text).expect("tool text should be a JSON array");

    assert!(
        !entries.is_empty(),
        "expected at least one medium+ finding on this repo's own lockfile \
         (toml/serde_spanned/wasi updates as of this writing) -- if this repo's \
         dependencies have since been fully updated, this assertion is stale, not a bug"
    );
    for e in &entries {
        assert!(
            e["diff"].is_object(),
            "min_severity filter should have excluded any up-to-date (diff: null) entry, found: {e}"
        );
    }
}
