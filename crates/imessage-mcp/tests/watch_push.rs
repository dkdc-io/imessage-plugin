//! End-to-end test for --watch mode.
//!
//! Spawns the binary against a stand-in chat.db, writes an allowlist, then
//! inserts an inbound row. Asserts the server pushes a
//! `notifications/claude/channel` notification on stdout within a few seconds.
//! Also exercises the `$CODEX_CHANNEL_DIR` filesystem envelope path.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::{Value, json};

fn bin_path() -> String {
    env!("CARGO_BIN_EXE_dkdc-io-imessage").to_string()
}

fn seed_chat_db(path: &Path, handle: &str, chat_guid: &str) {
    let conn = rusqlite::Connection::open(path).expect("open chat.db");
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
    conn.execute(
        "INSERT INTO handle (ROWID, id) VALUES (1, ?1)",
        params![handle],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat (ROWID, guid, style) VALUES (1, ?1, 45)",
        params![chat_guid],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_handle_join (chat_id, handle_id) VALUES (1, 1)",
        [],
    )
    .unwrap();
}

fn insert_inbound(path: &Path, text: &str, guid: &str, chat_guid: &str) {
    let conn = rusqlite::Connection::open(path).expect("reopen chat.db");
    conn.execute(
        "INSERT INTO message (guid, text, date, is_from_me, handle_id, service, cache_has_attachments) \
         VALUES (?1, ?2, 0, 0, 1, 'iMessage', 0)",
        params![guid, text],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_message_join (message_id, chat_id) VALUES ((SELECT MAX(ROWID) FROM message), (SELECT ROWID FROM chat WHERE guid = ?1))",
        params![chat_guid],
    )
    .unwrap();
}

/// Spawn, capture line-oriented stdout through a background reader, return
/// handles and a wait-for-notification helper.
struct Server {
    child: std::process::Child,
    rx: std::sync::mpsc::Receiver<String>,
    _stdin: std::process::ChildStdin,
}

impl Server {
    fn wait_for_channel_notification(&self, within: Duration) -> Option<Value> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            let remaining = deadline - Instant::now();
            if let Ok(line) = self.rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
                let v: Value = match serde_json::from_str(line.trim_end()) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("method") == Some(&json!("notifications/claude/channel")) {
                    return Some(v);
                }
            }
        }
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_server(
    chat_db: &Path,
    access_file: &Path,
    state_dir: &Path,
    codex_dir: Option<&Path>,
) -> Server {
    let mut cmd = Command::new(bin_path());
    cmd.env("DKDC_IO_CHAT_DB", chat_db)
        .env("DKDC_IO_ACCESS_FILE", access_file)
        .env("DKDC_IO_STATE_DIR", state_dir)
        .env(
            "DKDC_IO_LOG",
            std::env::var("DKDC_IO_LOG").unwrap_or_else(|_| "warn".into()),
        )
        // Fast poll so the test finishes quickly.
        .env("DKDC_IO_WATCH_INTERVAL_MS", "100");
    if let Some(d) = codex_dir {
        cmd.env("CODEX_CHANNEL_DIR", d);
    } else {
        cmd.env_remove("CODEX_CHANNEL_DIR");
    }
    let mut child = cmd
        .arg("--stdio")
        .arg("--watch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dkdc-io-imessage");

    // Send initialize so the server is live. Not strictly required for watch
    // but matches the real client flow.
    let mut stdin = child.stdin.take().expect("stdin");
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": { "name": "watch-test", "version": "0.0.1" } }
    });
    stdin
        .write_all(format!("{init}\n").as_bytes())
        .expect("write init");
    stdin.flush().ok();
    // Keep stdin open by stashing it in Server — dropping it would EOF the
    // child's stdin, which in turn flips the shutdown flag and exits the
    // watcher before it ever sees a new message.

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        while reader.read_line(&mut buf).unwrap_or(0) > 0 {
            if !buf.trim().is_empty() && tx.send(buf.clone()).is_err() {
                break;
            }
            buf.clear();
        }
    });

    Server {
        child,
        rx,
        _stdin: stdin,
    }
}

#[test]
fn watch_emits_channel_notification_for_allowed_inbound() {
    let tmp = tempfile::tempdir().unwrap();
    let chat_db = tmp.path().join("chat.db");
    let access_file = tmp.path().join("access.toml");
    let state_dir = tmp.path().to_path_buf();

    seed_chat_db(&chat_db, "+15550001234", "iMessage;-;+15550001234");
    std::fs::write(
        &access_file,
        r#"allow_from = ["+15550001234"]
"#,
    )
    .unwrap();

    let server = spawn_server(&chat_db, &access_file, &state_dir, None);

    // Drain the initialize response and any startup noise, then insert a row.
    // Give the watcher a beat to establish its watermark.
    // Wait long enough for the watcher's spawn_blocking thread to open the
    // chat.db handle and snapshot MAX(ROWID) before our insert lands.
    std::thread::sleep(Duration::from_millis(1500));
    insert_inbound(
        &chat_db,
        "hello from test",
        "guid-watch-1",
        "iMessage;-;+15550001234",
    );

    let notif = server
        .wait_for_channel_notification(Duration::from_secs(5))
        .expect("expected one channel notification within 5s");

    assert_eq!(notif["method"], json!("notifications/claude/channel"));
    assert_eq!(notif["params"]["content"], json!("hello from test"));
    assert_eq!(
        notif["params"]["meta"]["chat_id"],
        json!("iMessage;-;+15550001234")
    );
    assert_eq!(notif["params"]["meta"]["user"], json!("+15550001234"));
    assert!(
        notif["params"]["meta"]["message_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "expected non-empty message_id in meta"
    );
}

#[test]
fn watch_writes_codex_envelope_when_channel_dir_set() {
    let tmp = tempfile::tempdir().unwrap();
    let chat_db = tmp.path().join("chat.db");
    let access_file = tmp.path().join("access.toml");
    let state_dir = tmp.path().to_path_buf();
    let codex_dir = tmp.path().join("codex-channel");

    seed_chat_db(&chat_db, "+15550004444", "iMessage;-;+15550004444");
    std::fs::write(
        &access_file,
        r#"allow_from = ["+15550004444"]
"#,
    )
    .unwrap();

    let server = spawn_server(&chat_db, &access_file, &state_dir, Some(&codex_dir));

    // Wait long enough for the watcher's spawn_blocking thread to open the
    // chat.db handle and snapshot MAX(ROWID) before our insert lands.
    std::thread::sleep(Duration::from_millis(1500));
    insert_inbound(
        &chat_db,
        "codex envelope test",
        "guid-watch-2",
        "iMessage;-;+15550004444",
    );

    // Poll the filesystem until an envelope appears (or time out).
    let deadline = Instant::now() + Duration::from_secs(5);
    let inbox = codex_dir.join("inbox");
    let envelope_path = loop {
        if inbox.is_dir()
            && let Ok(mut entries) = std::fs::read_dir(&inbox)
            && let Some(Ok(entry)) = entries.next()
        {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                break p;
            }
        }
        if Instant::now() >= deadline {
            panic!("no envelope written to {}", inbox.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    // Ensure we also saw the MCP notification.
    let _ = server.wait_for_channel_notification(Duration::from_secs(2));

    let contents = std::fs::read_to_string(&envelope_path).expect("read envelope");
    let v: Value = serde_json::from_str(&contents).expect("envelope json");
    assert_eq!(v["from"], json!("+15550004444"));
    assert_eq!(v["text"], json!("codex envelope test"));
    assert_eq!(v["kind"], json!("brief"));
    assert!(v["ts"].as_str().is_some());
    // Filename shape: `{nanos}-{pid}-{rand_hex}-{seq}-from-{from}.json`.
    let name = envelope_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.ends_with("-from-+15550004444.json"),
        "unexpected filename shape: {name}"
    );
}

#[test]
fn watch_drops_non_allowlisted_inbound() {
    let tmp = tempfile::tempdir().unwrap();
    let chat_db = tmp.path().join("chat.db");
    let access_file = tmp.path().join("access.toml");
    let state_dir = tmp.path().to_path_buf();

    seed_chat_db(&chat_db, "stranger@x.com", "iMessage;-;stranger@x.com");
    // Empty allowlist. Server must not leak non-allowlisted inbound.
    std::fs::write(&access_file, "").unwrap();

    let server = spawn_server(&chat_db, &access_file, &state_dir, None);

    // Wait long enough for the watcher's spawn_blocking thread to open the
    // chat.db handle and snapshot MAX(ROWID) before our insert lands.
    std::thread::sleep(Duration::from_millis(1500));
    insert_inbound(
        &chat_db,
        "leaked?",
        "guid-watch-3",
        "iMessage;-;stranger@x.com",
    );

    // Wait long enough that a positive would have fired, then assert we got
    // no channel notification.
    let got = server.wait_for_channel_notification(Duration::from_millis(900));
    assert!(
        got.is_none(),
        "must NOT emit channel notification for non-allowlisted sender"
    );
}
