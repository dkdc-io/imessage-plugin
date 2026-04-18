//! Tool handlers, decoupled from the MCP transport. Each handler takes parsed
//! arguments plus shared state and returns a JSON value. The split lets us
//! unit-test the tool path without standing up a full stdio server.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::access::Access;
use crate::db;

/// Shared runtime state. Owns the chat.db connection behind a Mutex so the
/// MCP server can dispatch tools from different tokio tasks concurrently.
pub struct State {
    pub conn: Mutex<Connection>,
}

impl State {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }
}

/// Resolve the effective set of allowlisted chat GUIDs. Merges declared
/// `self.handles` with handles auto-detected from `message.account` rows.
fn resolve_allowed(conn: &Connection, access: &Access) -> Result<HashSet<String>> {
    let mut access = access.clone();
    let auto = db::self_handles(conn).unwrap_or_default();
    for h in auto {
        if !access.self_handles.iter().any(|existing| existing == &h) {
            access.self_handles.push(h);
        }
    }
    db::allowed_chat_guids(conn, &access)
}

fn fail_closed_if_empty(access: &Access) -> Result<()> {
    if access.is_empty() {
        let path = crate::access::access_file()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unknown path)".into());
        bail!(
            "allowlist is empty. dkdc-io-imessage is fail-closed by default. \
             Edit {path} to add `self.chat_id` and/or `allow_from` handles, then retry. \
             See https://github.com/dkdc-io/imessage-mcp for setup."
        );
    }
    Ok(())
}

pub fn reply(state: Arc<State>, access: &Access, args: &Value) -> Result<Value> {
    fail_closed_if_empty(access)?;
    let mut chat_id = args
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if chat_id.is_empty()
        && let Some(cid) = &access.self_chat_id
    {
        chat_id = cid.clone();
    }
    if chat_id.is_empty() {
        bail!("chat_id is required (no self.chat_id fallback configured)");
    }
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .context("text is required")?;
    if text.is_empty() {
        bail!("text must not be empty");
    }
    let allowed = {
        let conn = state.conn.lock().expect("conn mutex");
        resolve_allowed(&conn, access)?
    };
    if !allowed.contains(&chat_id) {
        bail!(
            "chat_id {chat_id} is not allowlisted. Add the recipient's handle to `allow_from` \
             (or set `self.chat_id` for the owner's own chat) in the access.toml."
        );
    }
    crate::send::send_text(&chat_id, text)?;
    Ok(json!({ "sent": true, "chat_id": chat_id }))
}

pub fn list_messages(state: Arc<State>, access: &Access, args: &Value) -> Result<Value> {
    fail_closed_if_empty(access)?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 200);
    let msgs = {
        let conn = state.conn.lock().expect("conn mutex");
        let allowed = resolve_allowed(&conn, access)?;
        db::list_messages(&conn, &allowed, &query, limit)?
    };
    let payload: Vec<Value> = msgs.iter().map(db::Message::to_json).collect();
    Ok(json!({ "messages": payload }))
}

pub fn read_message(state: Arc<State>, access: &Access, args: &Value) -> Result<Value> {
    fail_closed_if_empty(access)?;
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .context("id is required")?;
    let msg = {
        let conn = state.conn.lock().expect("conn mutex");
        let allowed = resolve_allowed(&conn, access)?;
        db::read_message(&conn, &allowed, id)?
    };
    match msg {
        Some(m) => Ok(m.to_json()),
        None => bail!("message {id} not found or not in an allowlisted chat"),
    }
}

/// Schemas for `tools/list`. Kept separate so tests can assert the surface
/// without importing the MCP server.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "reply",
            "description": "Send an iMessage to a chat. `chat_id` optional if `self.chat_id` is configured.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chat_id": { "type": "string", "description": "Chat GUID, e.g. 'iMessage;-;+15551234567'. Optional if a default self chat_id is set." },
                    "text": { "type": "string", "description": "Message body. Plain text." }
                },
                "required": ["text"]
            }
        }),
        json!({
            "name": "list_messages",
            "description": "Search allowlisted iMessage conversations. Returns the most recent matches first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Substring to match against message text. Empty = most recent." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20 }
                }
            }
        }),
        json!({
            "name": "read_message",
            "description": "Fetch one message by its GUID. Returns null if not in an allowlisted chat.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Message GUID." } },
                "required": ["id"]
            }
        }),
    ]
}
