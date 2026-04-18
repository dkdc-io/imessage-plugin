//! Watch `~/Library/Messages/chat.db` and push new inbound allowlisted
//! messages to the LLM session. Runs alongside the normal MCP server.
//!
//! Two emission routes, both active when a target is available:
//!
//! 1. MCP `notifications/claude/channel` over stdio. Claude Code and the
//!    codex fork both consume this notification shape, so a single push
//!    works regardless of which client registered the server.
//! 2. `$CODEX_CHANNEL_DIR/inbox/<filename>.json` filesystem envelope,
//!    when `CODEX_CHANNEL_DIR` is set (codex filesystem channel mode).
//!
//! Polling uses a `MAX(ROWID)` watermark. No FSEvents / SQLite trigger is
//! reliable across macOS versions; a 750ms tick with a prepared statement
//! is the same approach the `dkdc-io-imessage` demo scripts already use
//! and costs well under 1ms per tick on cold chat.db.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::access::Access;
use crate::attributed;

pub const JSONRPC: &str = "2.0";
pub const CHANNEL_METHOD: &str = "notifications/claude/channel";
pub const DEFAULT_INTERVAL_MS: u64 = 750;
pub const CODEX_CHANNEL_ENV: &str = "CODEX_CHANNEL_DIR";

/// A chat.db row as the watcher sees it. Richer than [`crate::db::Message`]
/// because the watcher needs `service`, `chat_style`, and attachment flags
/// to filter inbound traffic correctly.
#[derive(Debug, Clone)]
pub struct Row {
    pub rowid: i64,
    pub guid: String,
    pub text: Option<String>,
    pub attributed_body: Option<Vec<u8>>,
    pub date: i64,
    pub is_from_me: bool,
    pub cache_has_attachments: bool,
    pub service: Option<String>,
    pub handle_id: Option<String>,
    pub chat_guid: String,
    pub chat_style: Option<i64>,
}

const POLL_SQL: &str = "\
SELECT m.ROWID, m.guid, m.text, m.attributedBody, m.date, m.is_from_me, \
       m.cache_has_attachments, m.service, h.id, c.guid, c.style \
FROM message m \
JOIN chat_message_join cmj ON cmj.message_id = m.ROWID \
JOIN chat c ON c.ROWID = cmj.chat_id \
LEFT JOIN handle h ON h.ROWID = m.handle_id \
WHERE m.ROWID > ?1 \
ORDER BY m.ROWID ASC";

pub fn open_watch_conn() -> Result<Connection> {
    let path = crate::db::chat_db_path()?;
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open chat.db for watch at {}", path.display()))?;
    Ok(conn)
}

pub fn watermark(conn: &Connection) -> Result<i64> {
    let w: Option<i64> = conn
        .query_row("SELECT MAX(ROWID) FROM message", [], |r| r.get(0))
        .ok()
        .flatten();
    Ok(w.unwrap_or(0))
}

pub fn poll(conn: &Connection, after: i64) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare_cached(POLL_SQL)?;
    let rows = stmt
        .query_map(params![after], |r| {
            Ok(Row {
                rowid: r.get(0)?,
                guid: r.get(1)?,
                text: r.get(2)?,
                attributed_body: r.get(3)?,
                date: r.get(4)?,
                is_from_me: r.get::<_, i64>(5)? != 0,
                cache_has_attachments: r.get::<_, Option<i64>>(6).unwrap_or(Some(0)).unwrap_or(0)
                    != 0,
                service: r.get(7)?,
                handle_id: r.get(8)?,
                chat_guid: r.get(9)?,
                chat_style: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Clone)]
pub struct Config {
    pub interval: Duration,
    pub codex_channel_dir: Option<PathBuf>,
    pub self_handles: HashSet<String>,
}

impl Config {
    pub fn from_env(self_handles: HashSet<String>) -> Self {
        let interval_ms = std::env::var("DKDC_IO_WATCH_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_MS)
            .max(100);
        let codex_channel_dir = std::env::var_os(CODEX_CHANNEL_ENV)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self {
            interval: Duration::from_millis(interval_ms),
            codex_channel_dir,
            self_handles,
        }
    }
}

/// Start the watcher on a blocking thread. Returns immediately. The watcher
/// exits when `shutdown` flips true (reader saw stdin EOF). When the watcher
/// exits, it drops its `out_tx` clone, which in turn lets the stdio writer
/// task see its rx close and exit cleanly.
pub fn spawn(out_tx: UnboundedSender<Value>, cfg: Config, shutdown: Arc<AtomicBool>) {
    tokio::task::spawn_blocking(move || {
        if let Err(err) = run(out_tx, cfg, shutdown) {
            tracing::warn!(error = %err, "watch loop exited");
        }
    });
}

fn run(out_tx: UnboundedSender<Value>, cfg: Config, shutdown: Arc<AtomicBool>) -> Result<()> {
    let conn = open_watch_conn()?;
    let mut mark = watermark(&conn)?;
    tracing::info!(
        watermark = mark,
        interval_ms = cfg.interval.as_millis() as u64,
        codex_fs = cfg.codex_channel_dir.is_some(),
        "imessage watch loop starting"
    );

    let state = Arc::new(State { cfg });

    loop {
        if shutdown.load(Ordering::Acquire) {
            tracing::debug!("shutdown signaled; exiting watch loop");
            return Ok(());
        }
        if out_tx.is_closed() {
            tracing::debug!("out_tx closed; exiting watch loop");
            return Ok(());
        }

        let rows = match poll(&conn, mark) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "poll failed");
                Vec::new()
            }
        };

        if !rows.is_empty() {
            // Recompute the allowlisted chat set each tick; cheap, and picks
            // up new DM chats as soon as they exist in chat.db.
            let access = crate::access::load();
            let allowed = match resolve_allowed(&conn, &access, &state.cfg.self_handles) {
                Ok(a) => a,
                Err(err) => {
                    tracing::warn!(error = %err, "resolve_allowed failed");
                    HashSet::new()
                }
            };
            for r in rows {
                mark = r.rowid;
                if let Err(err) = handle_inbound(&out_tx, &state, &allowed, &access, &r) {
                    tracing::warn!(rowid = r.rowid, error = %err, "handle_inbound failed");
                }
            }
        }

        sleep_interruptible(state.cfg.interval, &shutdown);
    }
}

/// Sleep up to `total`, waking every ~50ms to check shutdown. Keeps stdin-EOF
/// shutdown latency bounded below the poll interval.
fn sleep_interruptible(total: Duration, shutdown: &Arc<AtomicBool>) {
    let step = Duration::from_millis(50);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let s = if remaining < step { remaining } else { step };
        std::thread::sleep(s);
        remaining = remaining.saturating_sub(s);
    }
}

struct State {
    cfg: Config,
}

fn resolve_allowed(
    conn: &Connection,
    access: &Access,
    self_handles: &HashSet<String>,
) -> Result<HashSet<String>> {
    let mut access = access.clone();
    for h in self_handles {
        if !access.self_handles.iter().any(|existing| existing == h) {
            access.self_handles.push(h.clone());
        }
    }
    crate::db::allowed_chat_guids(conn, &access)
}

fn handle_inbound(
    out_tx: &UnboundedSender<Value>,
    state: &Arc<State>,
    allowed: &HashSet<String>,
    _access: &Access,
    r: &Row,
) -> Result<()> {
    if r.is_from_me {
        return Ok(());
    }
    // iMessage only — SMS sender IDs are spoofable.
    if r.service.as_deref() != Some("iMessage") {
        return Ok(());
    }
    // style 45 = DM. Drop groups and undefined for v0.2.
    let Some(style) = r.chat_style else {
        return Ok(());
    };
    if style != 45 {
        return Ok(());
    }
    if !allowed.contains(&r.chat_guid) {
        return Ok(());
    }
    let Some(sender) = r.handle_id.clone() else {
        return Ok(());
    };

    let text = message_text(r);
    if text.trim().is_empty() && !r.cache_has_attachments {
        return Ok(());
    }
    let content = if text.is_empty() && r.cache_has_attachments {
        "(image)".to_string()
    } else {
        text
    };

    let ts = crate::db::apple_date_to_utc(r.date).to_rfc3339();
    let meta = json!({
        "chat_id": r.chat_guid,
        "message_id": r.guid,
        "user": sender,
        "ts": ts,
    });

    // 1. MCP stdio push (claude + codex fork both consume this method).
    let notif = json!({
        "jsonrpc": JSONRPC,
        "method": CHANNEL_METHOD,
        "params": { "content": content, "meta": meta },
    });
    if let Err(err) = out_tx.send(notif) {
        tracing::debug!(error = %err, "mcp out_tx send failed (client likely gone)");
    }

    // 2. Filesystem envelope — only if configured.
    if let Some(dir) = &state.cfg.codex_channel_dir
        && let Err(err) = write_codex_envelope(dir, &sender, &content, &ts)
    {
        tracing::warn!(error = %err, dir = %dir.display(), "codex envelope write failed");
    }
    Ok(())
}

fn message_text(r: &Row) -> String {
    if let Some(t) = &r.text
        && !t.is_empty()
    {
        return t.clone();
    }
    attributed::parse_attributed_body(r.attributed_body.as_deref()).unwrap_or_default()
}

/// Write a codex channel envelope to `<dir>/inbox/<name>.json` using the
/// hardlink-atomic pattern the fork expects. See
/// gh-org/lostmygithubaccount/codex/codex-rs/tui/src/channel.rs.
pub fn write_codex_envelope(dir: &Path, from: &str, text: &str, ts: &str) -> Result<PathBuf> {
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox)
        .with_context(|| format!("create inbox dir {}", inbox.display()))?;
    let env = json!({
        "from": from,
        "to": "codex",
        "kind": "brief",
        "text": text,
        "ts": ts,
        "thread": "imessage",
    });
    let bytes = serde_json::to_vec(&env)?;
    for attempt in 0..8u32 {
        let name = build_filename(from);
        let final_path = inbox.join(&name);
        let tmp_path = inbox.join(format!(".{name}.{attempt}.tmp"));
        std::fs::write(&tmp_path, &bytes)
            .with_context(|| format!("write tmp {}", tmp_path.display()))?;
        match std::fs::hard_link(&tmp_path, &final_path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Ok(final_path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(&tmp_path);
                continue;
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(err.into());
            }
        }
    }
    anyhow::bail!("exhausted 8 attempts to write unique codex envelope filename")
}

fn build_filename(from: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let pid = std::process::id();
    // rand_hex slot: low 32 bits of nanos XOR pid. Deterministic but unique
    // enough across writers without adding a `rand` dep.
    let rand_hex = (nanos as u32) ^ pid;
    let safe_from = sanitize_from(from);
    format!("{nanos:020}-{pid:010}-{rand_hex:08x}-{seq:010}-from-{safe_from}.json")
}

fn sanitize_from(from: &str) -> String {
    from.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '-' | '_' | '.' | '@' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed_mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
            CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, style INTEGER);
            CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
            CREATE TABLE message (
                ROWID INTEGER PRIMARY KEY,
                guid TEXT, text TEXT, attributedBody BLOB, date INTEGER,
                is_from_me INTEGER, account TEXT, handle_id INTEGER,
                service TEXT, cache_has_attachments INTEGER
            );
            CREATE TABLE chat_message_join (message_id INTEGER, chat_id INTEGER);
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn build_filename_shape_is_stable() {
        let n = build_filename("+15550001234");
        assert!(n.ends_with("-from-+15550001234.json"), "name was {n}");
        let mid: Vec<&str> = n.split('-').collect();
        assert_eq!(mid[0].len(), 20, "nanos slot is 20 chars");
        assert_eq!(mid[1].len(), 10, "pid slot is 10 chars");
        assert_eq!(mid[2].len(), 8, "rand_hex slot is 8 chars");
        assert_eq!(mid[3].len(), 10, "seq slot is 10 chars");
    }

    #[test]
    fn sanitize_from_preserves_phone_and_email() {
        assert_eq!(sanitize_from("+15551234567"), "+15551234567");
        assert_eq!(sanitize_from("a.b@example.com"), "a.b@example.com");
        assert_eq!(sanitize_from("weird/path"), "weird_path");
    }

    #[test]
    fn write_codex_envelope_creates_inbox_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let p = write_codex_envelope(dir, "+15550001234", "hi", "2026-04-18T00:00:00+00:00")
            .expect("write envelope");
        let inbox = dir.join("inbox");
        assert!(inbox.is_dir(), "inbox dir must exist");
        assert!(p.starts_with(&inbox));
        let contents = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(v["from"], json!("+15550001234"));
        assert_eq!(v["text"], json!("hi"));
        assert_eq!(v["ts"], json!("2026-04-18T00:00:00+00:00"));
        assert_eq!(v["kind"], json!("brief"));
        // No leftover tmp files.
        let leftover: Vec<_> = std::fs::read_dir(&inbox)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "no .tmp files should remain");
    }

    fn seed_msg(conn: &Connection, rowid: i64, chat_guid: &str, from_me: i64) {
        conn.execute(
            "INSERT INTO message (ROWID, guid, text, date, is_from_me, handle_id, service, cache_has_attachments) \
             VALUES (?1, ?2, ?3, 0, ?4, (SELECT ROWID FROM handle LIMIT 1), 'iMessage', 0)",
            params![rowid, format!("guid-{rowid}"), "hello world", from_me],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (?1, (SELECT ROWID FROM chat WHERE guid = ?2))",
            params![rowid, chat_guid],
        )
        .unwrap();
    }

    fn seed_dm(conn: &Connection, handle: &str, chat_guid: &str) {
        conn.execute(
            "INSERT INTO handle (ROWID, id) VALUES ((SELECT COALESCE(MAX(ROWID),0)+1 FROM handle), ?1)",
            params![handle],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat (ROWID, guid, style) VALUES ((SELECT COALESCE(MAX(ROWID),0)+1 FROM chat), ?1, 45)",
            params![chat_guid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_handle_join (chat_id, handle_id) VALUES (\
              (SELECT ROWID FROM chat WHERE guid = ?1), \
              (SELECT ROWID FROM handle WHERE id = ?2))",
            params![chat_guid, handle],
        )
        .unwrap();
    }

    #[test]
    fn poll_returns_only_rows_above_watermark() {
        let conn = seed_mem_db();
        seed_dm(&conn, "+15550001234", "iMessage;-;+15550001234");
        seed_msg(&conn, 1, "iMessage;-;+15550001234", 0);
        seed_msg(&conn, 2, "iMessage;-;+15550001234", 0);
        seed_msg(&conn, 3, "iMessage;-;+15550001234", 0);
        let rows = poll(&conn, 1).unwrap();
        let ids: Vec<i64> = rows.iter().map(|r| r.rowid).collect();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(rows[0].service.as_deref(), Some("iMessage"));
        assert_eq!(rows[0].chat_style, Some(45));
    }

    #[tokio::test]
    async fn handle_inbound_emits_channel_notification_for_allowed_dm() {
        let conn = seed_mem_db();
        seed_dm(&conn, "+15550001234", "iMessage;-;+15550001234");
        seed_msg(&conn, 1, "iMessage;-;+15550001234", 0);
        let rows = poll(&conn, 0).unwrap();
        assert_eq!(rows.len(), 1);

        let access = Access {
            self_chat_id: None,
            self_handles: Vec::new(),
            allow_from: vec!["+15550001234".into()],
        };
        let allowed = resolve_allowed(&conn, &access, &HashSet::new()).unwrap();
        assert!(allowed.contains("iMessage;-;+15550001234"));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let state = Arc::new(State {
            cfg: Config {
                interval: Duration::from_millis(1),
                codex_channel_dir: None,
                self_handles: HashSet::new(),
            },
        });
        handle_inbound(&tx, &state, &allowed, &access, &rows[0]).unwrap();
        let notif = rx.try_recv().expect("one notification emitted");
        assert_eq!(notif["method"], json!(CHANNEL_METHOD));
        assert_eq!(notif["params"]["content"], json!("hello world"));
        assert_eq!(
            notif["params"]["meta"]["chat_id"],
            json!("iMessage;-;+15550001234")
        );
        assert_eq!(notif["params"]["meta"]["user"], json!("+15550001234"));
    }

    #[tokio::test]
    async fn handle_inbound_drops_non_allowlisted() {
        let conn = seed_mem_db();
        seed_dm(&conn, "stranger@x.com", "iMessage;-;stranger@x.com");
        seed_msg(&conn, 1, "iMessage;-;stranger@x.com", 0);
        let rows = poll(&conn, 0).unwrap();
        let access = Access::default();
        let allowed = resolve_allowed(&conn, &access, &HashSet::new()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let state = Arc::new(State {
            cfg: Config {
                interval: Duration::from_millis(1),
                codex_channel_dir: None,
                self_handles: HashSet::new(),
            },
        });
        handle_inbound(&tx, &state, &allowed, &access, &rows[0]).unwrap();
        assert!(rx.try_recv().is_err(), "must not emit for non-allowlisted");
    }

    #[tokio::test]
    async fn handle_inbound_drops_own_messages() {
        let conn = seed_mem_db();
        seed_dm(&conn, "+15550001234", "iMessage;-;+15550001234");
        seed_msg(&conn, 1, "iMessage;-;+15550001234", 1);
        let rows = poll(&conn, 0).unwrap();
        let access = Access {
            self_chat_id: None,
            self_handles: Vec::new(),
            allow_from: vec!["+15550001234".into()],
        };
        let allowed = resolve_allowed(&conn, &access, &HashSet::new()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let state = Arc::new(State {
            cfg: Config {
                interval: Duration::from_millis(1),
                codex_channel_dir: None,
                self_handles: HashSet::new(),
            },
        });
        handle_inbound(&tx, &state, &allowed, &access, &rows[0]).unwrap();
        assert!(rx.try_recv().is_err(), "must not emit is_from_me");
    }
}
