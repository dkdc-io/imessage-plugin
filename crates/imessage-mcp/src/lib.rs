//! iMessage MCP server.
//!
//! A standalone MCP server that exposes three tools to an LLM CLI (Codex CLI,
//! Claude Code, or any JSON-RPC stdio MCP client):
//!
//! - `reply(chat_id, text)` — send an iMessage via `osascript` → Messages.app.
//! - `list_messages(query, limit)` — search `~/Library/Messages/chat.db`.
//! - `read_message(id)` — fetch the full body of one message by GUID.
//!
//! All three are gated by an allowlist at `~/.config/dkdc-io/imessage/access.toml`.
//! Empty allowlist = fail closed. See README for setup.
//!
//! # Prior art
//!
//! Anthropic shipped the original TypeScript/Bun iMessage MCP server for Claude
//! Code:
//! <https://github.com/anthropics/claude-plugins-official/tree/main/external_plugins/imessage>
//! We first ported that shape, then hit two correctness bugs: typedstream
//! truncation on messages above roughly 130 bytes, and echo-tracker replay of
//! outbound replies as inbound messages. Those bugs were fixed, then the
//! project was rewritten in Rust for correctness, not speed. The current
//! server keeps the same LLM-CLI-agnostic surface.

pub mod access;
pub mod attributed;
pub mod cli;
pub mod db;
pub mod mcp;
pub mod send;
pub mod tools;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
