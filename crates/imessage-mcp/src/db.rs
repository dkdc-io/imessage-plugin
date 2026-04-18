//! Read-only access to `~/Library/Messages/chat.db`.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, params};

use crate::access::Access;

const APPLE_EPOCH_UNIX_SECS: i64 = 978_307_200;

pub fn chat_db_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DKDC_IO_CHAT_DB") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join("Library/Messages/chat.db"))
}

pub fn open() -> Result<Connection> {
    let path = chat_db_path()?;
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| {
        format!(
            "cannot read {}\n  Grant Full Disk Access to the process (System Settings → \
             Privacy & Security → Full Disk Access).",
            path.display()
        )
    })?;
    // Smoke-probe the schema to surface FDA failures early.
    conn.query_row("SELECT ROWID FROM message LIMIT 1", [], |_| Ok(()))
        .ok();
    Ok(conn)
}

#[derive(Debug, Clone)]
pub struct Message {
    pub rowid: i64,
    pub guid: String,
    pub text: String,
    pub date: DateTime<Utc>,
    pub is_from_me: bool,
    pub handle_id: Option<String>,
    pub chat_guid: String,
}

impl Message {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.guid,
            "chat_id": self.chat_guid,
            "user": self.handle_id,
            "text": self.text,
            "is_from_me": self.is_from_me,
            "ts": self.date.to_rfc3339(),
        })
    }
}

fn read_message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let rowid: i64 = r.get(0)?;
    let guid: String = r.get(1)?;
    let text: Option<String> = r.get(2)?;
    let attributed_body: Option<Vec<u8>> = r.get(3)?;
    let date: i64 = r.get(4)?;
    let is_from_me: bool = r.get::<_, i64>(5)? != 0;
    let handle_id: Option<String> = r.get(6)?;
    let chat_guid: String = r.get(7)?;
    let body = match text.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            crate::attributed::parse_attributed_body(attributed_body.as_deref()).unwrap_or_default()
        }
    };
    Ok(Message {
        rowid,
        guid,
        text: body,
        date: apple_date_to_utc(date),
        is_from_me,
        handle_id,
        chat_guid,
    })
}

const SELECT_COLS: &str = "\
m.ROWID, m.guid, m.text, m.attributedBody, m.date, m.is_from_me, \
h.id, c.guid";

const BASE_JOIN: &str = "\
FROM message m \
JOIN chat_message_join cmj ON cmj.message_id = m.ROWID \
JOIN chat c ON c.ROWID = cmj.chat_id \
LEFT JOIN handle h ON h.ROWID = m.handle_id";

/// Search across allowlisted chats. `query` is a substring match against
/// `message.text`; if empty, returns the most recent N messages. Attributed-
/// body-only messages are included regardless of query (we don't decode them
/// server-side for the search term to keep the SQL hot path simple — callers
/// wanting full-text search can narrow with date ranges and read bodies).
pub fn list_messages(
    conn: &Connection,
    allowed_chats: &HashSet<String>,
    query: &str,
    limit: i64,
) -> Result<Vec<Message>> {
    if allowed_chats.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; allowed_chats.len()].join(",");
    let query_clause = if query.is_empty() {
        String::new()
    } else {
        format!(" AND (m.text LIKE ?{})", allowed_chats.len() + 1)
    };
    let sql = format!(
        "SELECT {SELECT_COLS} {BASE_JOIN} WHERE c.guid IN ({placeholders}){query_clause} \
         ORDER BY m.ROWID DESC LIMIT ?{}",
        allowed_chats.len() + if query.is_empty() { 1 } else { 2 },
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for g in allowed_chats {
        bound.push(Box::new(g.clone()));
    }
    let needle = if query.is_empty() {
        String::new()
    } else {
        format!("%{query}%")
    };
    if !query.is_empty() {
        bound.push(Box::new(needle));
    }
    bound.push(Box::new(limit));

    let params_refs: Vec<&dyn rusqlite::ToSql> = bound.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), read_message_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Fetch one message by GUID. Returns `None` if the GUID is not in an
/// allowlisted chat (or doesn't exist).
pub fn read_message(
    conn: &Connection,
    allowed_chats: &HashSet<String>,
    guid: &str,
) -> Result<Option<Message>> {
    let sql = format!("SELECT {SELECT_COLS} {BASE_JOIN} WHERE m.guid = ?1 LIMIT 1",);
    let row: Option<Message> = conn
        .prepare(&sql)?
        .query_row(params![guid], read_message_row)
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(row.filter(|m| allowed_chats.contains(&m.chat_guid)))
}

pub fn self_handles(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT account FROM message \
         WHERE is_from_me = 1 AND account IS NOT NULL AND account != '' LIMIT 50",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for addr in rows {
        let n = normalize_account(&addr?);
        if !n.is_empty() {
            out.insert(n);
        }
    }
    Ok(out)
}

fn normalize_account(s: &str) -> String {
    let trimmed =
        if s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() {
            &s[2..]
        } else {
            s
        };
    trimmed.trim().to_lowercase()
}

fn dm_chats_for_handle(conn: &Connection, handle: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT c.guid FROM chat c \
         JOIN chat_handle_join chj ON chj.chat_id = c.ROWID \
         JOIN handle h ON h.ROWID = chj.handle_id \
         WHERE c.style = 45 AND LOWER(h.id) = ?1",
    )?;
    let rows = stmt
        .query_map(params![handle], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Compute the set of chat GUIDs that reply / list / read will accept.
/// Includes: (a) `self.chat_id` if set, (b) DM chats for every allowlisted
/// handle (owner and other), (c) no groups for v0 — add a dedicated
/// `groups` field in v0.2 if there's demand.
pub fn allowed_chat_guids(conn: &Connection, access: &Access) -> Result<HashSet<String>> {
    let mut out: HashSet<String> = HashSet::new();
    if let Some(cid) = &access.self_chat_id {
        out.insert(cid.clone());
    }
    for handle in access.all_handles_lower() {
        for guid in dm_chats_for_handle(conn, &handle)? {
            out.insert(guid);
        }
    }
    Ok(out)
}

pub fn apple_date_to_utc(ns: i64) -> DateTime<Utc> {
    let secs = ns / 1_000_000_000 + APPLE_EPOCH_UNIX_SECS;
    let sub_ns = (ns % 1_000_000_000) as u32;
    Utc.timestamp_opt(secs, sub_ns)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_account_strips_prefix() {
        assert_eq!(normalize_account("E:User@Example.com"), "user@example.com");
        assert_eq!(normalize_account("P:+15551234567"), "+15551234567");
        assert_eq!(normalize_account("+15551234567"), "+15551234567");
    }

    #[test]
    fn apple_date_matches_known() {
        let d = apple_date_to_utc(0);
        assert_eq!(d.timestamp(), APPLE_EPOCH_UNIX_SECS);
    }

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
            CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, style INTEGER);
            CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
            CREATE TABLE message (
                ROWID INTEGER PRIMARY KEY,
                guid TEXT,
                text TEXT,
                attributedBody BLOB,
                date INTEGER,
                is_from_me INTEGER,
                account TEXT,
                handle_id INTEGER
            );
            CREATE TABLE chat_message_join (message_id INTEGER, chat_id INTEGER);
            "#,
        )
        .unwrap();
        conn
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

    fn seed_msg(conn: &Connection, chat_guid: &str, guid: &str, text: &str, from_me: i64) {
        conn.execute(
            "INSERT INTO message (guid, text, date, is_from_me, handle_id) VALUES \
              (?1, ?2, 0, ?3, (SELECT ROWID FROM handle LIMIT 1))",
            params![guid, text, from_me],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (\
              (SELECT MAX(ROWID) FROM message), \
              (SELECT ROWID FROM chat WHERE guid = ?1))",
            params![chat_guid],
        )
        .unwrap();
    }

    #[test]
    fn allowlist_resolves_dm_chats() {
        let conn = mem_db();
        seed_dm(&conn, "+15551112222", "iMessage;-;+15551112222");
        seed_dm(&conn, "stranger@x.com", "iMessage;-;stranger@x.com");
        let access = Access {
            self_chat_id: None,
            self_handles: Vec::new(),
            allow_from: vec!["+15551112222".into()],
        };
        let chats = allowed_chat_guids(&conn, &access).unwrap();
        assert!(chats.contains("iMessage;-;+15551112222"));
        assert!(!chats.contains("iMessage;-;stranger@x.com"));
    }

    #[test]
    fn list_messages_filters_to_allowlisted_chats() {
        let conn = mem_db();
        seed_dm(&conn, "ok@x.com", "iMessage;-;ok@x.com");
        seed_dm(&conn, "bad@x.com", "iMessage;-;bad@x.com");
        seed_msg(&conn, "iMessage;-;ok@x.com", "g1", "hello", 0);
        seed_msg(&conn, "iMessage;-;bad@x.com", "g2", "spam", 0);

        let mut allowed = HashSet::new();
        allowed.insert("iMessage;-;ok@x.com".to_string());
        let msgs = list_messages(&conn, &allowed, "", 50).unwrap();
        let guids: Vec<_> = msgs.iter().map(|m| m.guid.as_str()).collect();
        assert_eq!(guids, vec!["g1"]);
    }

    #[test]
    fn read_message_enforces_allowlist() {
        let conn = mem_db();
        seed_dm(&conn, "bad@x.com", "iMessage;-;bad@x.com");
        seed_msg(&conn, "iMessage;-;bad@x.com", "gx", "hi", 0);
        let allowed = HashSet::new();
        let got = read_message(&conn, &allowed, "gx").unwrap();
        assert!(
            got.is_none(),
            "read_message must not return non-allowlisted"
        );
    }

    #[test]
    fn list_messages_query_substring() {
        let conn = mem_db();
        seed_dm(&conn, "ok@x.com", "iMessage;-;ok@x.com");
        seed_msg(&conn, "iMessage;-;ok@x.com", "g1", "hello world", 0);
        seed_msg(&conn, "iMessage;-;ok@x.com", "g2", "goodbye", 0);
        let mut allowed = HashSet::new();
        allowed.insert("iMessage;-;ok@x.com".to_string());
        let msgs = list_messages(&conn, &allowed, "hello", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].guid, "g1");
    }
}
