//! Exercises the compiled `capscan-mcp` binary over real stdio, the same
//! way any MCP client would, rather than reaching into rmcp's lower-level
//! client-side request API. Manually verified once against the actual
//! protocol (initialize/tools-list/tools-call all round-tripped correctly)
//! before writing these as the corresponding regression tests.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_server_with_input(input: &str) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_capscan-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start capscan-mcp");

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input.as_bytes())
        .expect("failed to write to server stdin");
    // stdin's temporary handle above is dropped here, closing the pipe so
    // the server sees EOF once it's processed everything we sent it.

    let output = child
        .wait_with_output()
        .expect("server did not exit cleanly");
    assert!(
        output.status.success(),
        "server exited with {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("non-utf8 stdout")
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON line {line:?}: {e}"))
        })
        .collect()
}

fn initialize_and(next: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        next,
    )
}

#[test]
fn initialize_reports_instructions() {
    let responses = run_server_with_input(&initialize_and(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ));
    let instructions = responses[0]["result"]["instructions"]
        .as_str()
        .expect("initialize response should include instructions");
    assert!(instructions.contains("capability surface"));
}

#[test]
fn lists_all_three_tools() {
    let responses = run_server_with_input(&initialize_and(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ));
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
    let responses = run_server_with_input(&initialize_and(call));

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
