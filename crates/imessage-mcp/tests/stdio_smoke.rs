//! End-to-end stdio smoke: spawn the binary, initialize, list tools, ping.
//!
//! Does NOT send any live iMessage. The test isolates state via DKDC_IO_*
//! env vars so it can run on any macOS dev box (and no-ops on non-macOS with
//! an in-memory chat.db stand-in via `DKDC_IO_CHAT_DB`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

/// Path to the cargo-built binary.
fn bin_path() -> String {
    // Cargo sets CARGO_BIN_EXE_<name> for binary targets at integration test compile time.
    env!("CARGO_BIN_EXE_dkdc-io-imessage").to_string()
}

fn isolated_env(tmp: &tempfile::TempDir) -> Vec<(String, String)> {
    let chat_db = tmp.path().join("chat.db");
    seed_empty_chat_db(&chat_db);
    vec![
        (
            "DKDC_IO_STATE_DIR".into(),
            tmp.path().to_string_lossy().into_owned(),
        ),
        (
            "DKDC_IO_ACCESS_FILE".into(),
            tmp.path()
                .join("access.toml")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "DKDC_IO_CHAT_DB".into(),
            chat_db.to_string_lossy().into_owned(),
        ),
        ("DKDC_IO_LOG".into(), "warn".into()),
    ]
}

fn seed_empty_chat_db(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).expect("open stand-in chat.db");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS message (
            ROWID INTEGER PRIMARY KEY,
            guid TEXT, text TEXT, attributedBody BLOB,
            date INTEGER, is_from_me INTEGER, account TEXT, handle_id INTEGER,
            service TEXT, cache_has_attachments INTEGER
        );
        CREATE TABLE IF NOT EXISTS handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE IF NOT EXISTS chat (ROWID INTEGER PRIMARY KEY, guid TEXT, style INTEGER);
        CREATE TABLE IF NOT EXISTS chat_handle_join (chat_id INTEGER, handle_id INTEGER);
        CREATE TABLE IF NOT EXISTS chat_message_join (message_id INTEGER, chat_id INTEGER);
        "#,
    )
    .unwrap();
}

fn send_and_receive(requests: &[Value]) -> Vec<Value> {
    let tmp = tempfile::tempdir().unwrap();
    let env = isolated_env(&tmp);
    let mut cmd = Command::new(bin_path());
    for (k, v) in &env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dkdc-io-imessage");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    for r in requests {
        let line = serde_json::to_string(r).unwrap();
        stdin.write_all(line.as_bytes()).expect("write request");
        stdin.write_all(b"\n").expect("write newline");
    }
    stdin.flush().ok();
    drop(stdin);

    // Read one response per request.
    let mut responses = Vec::new();
    let mut reader = BufReader::new(stdout);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = String::new();
        while reader.read_line(&mut buf).unwrap_or(0) > 0 {
            if !buf.trim().is_empty() && tx.send(buf.clone()).is_err() {
                break;
            }
            buf.clear();
        }
    });
    for _ in 0..requests.len() {
        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("response within 5s");
        responses.push(serde_json::from_str(line.trim_end()).expect("valid json response"));
    }

    let _ = child.kill();
    let _ = child.wait();
    responses
}

#[test]
fn initialize_then_tools_list_returns_three_tools() {
    let responses = send_and_receive(&[
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "smoke", "version": "0.0.1" } }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    ]);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], json!(1));
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        json!("dkdc-io-imessage")
    );

    assert_eq!(responses[1]["id"], json!(2));
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["reply", "list_messages", "read_message"],
        "expected exactly these three tools in this order",
    );
}

#[test]
fn ping_returns_empty_result() {
    let responses = send_and_receive(&[json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" })]);
    assert_eq!(responses[0]["id"], json!(7));
    assert_eq!(responses[0]["result"], json!({}));
}

#[test]
fn empty_allowlist_reply_returns_is_error() {
    // No access.toml written — fail-closed path through tools/call.
    let responses = send_and_receive(&[json!({
        "jsonrpc": "2.0", "id": 10, "method": "tools/call",
        "params": { "name": "reply", "arguments": { "chat_id": "x", "text": "hi" } }
    })]);
    assert_eq!(responses[0]["id"], json!(10));
    assert_eq!(responses[0]["result"]["isError"], json!(true));
    let txt = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        txt.contains("fail-closed"),
        "expected fail-closed explanation, got: {txt}"
    );
}
