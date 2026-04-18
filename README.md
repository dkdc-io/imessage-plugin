# imessage-plugin

An iMessage MCP plugin for any LLM CLI that speaks MCP over stdio —
[Codex CLI](https://github.com/openai/codex),
[Claude Code](https://docs.claude.com/en/docs/claude-code), or your own harness.

Install one binary, point your CLI at it, and the LLM can:

- send iMessage via `reply(chat_id, text)`,
- search your messages via `list_messages(query, limit)`,
- fetch one message by GUID via `read_message(id)`.

Gated by a local allowlist. Fail-closed by default.

## Install

```sh
cargo install dkdc-io-imessage
```

Then grant the terminal (or the CLI's host process) Full Disk Access on macOS
so it can read `~/Library/Messages/chat.db`, and edit
`~/.config/dkdc-io/imessage/access.toml` to add at least one handle or a
`self.chat_id`.

Details + Codex and Claude Code config snippets live in the
[crate README](crates/dkdc-io-imessage/README.md).

## Repo layout

```
imessage-plugin/
  Cargo.toml                              # workspace
  LICENSE-MIT / LICENSE-APACHE
  crates/
    dkdc-io-imessage/                     # the MCP server crate
      Cargo.toml
      README.md
      src/
      tests/injection.rs                  # allowlist + osascript argv tests
      tests/stdio_smoke.rs                # end-to-end JSON-RPC smoke
```

One crate, one binary, zero runtime dependencies on any host framework.

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three run clean on macOS.

## Prior art

This plugin is an independent Rust implementation inspired by Anthropic's
official iMessage plugin for Claude Code
([anthropics/claude-plugins-official/external_plugins/imessage][upstream]),
a TypeScript/Bun server that pioneered the shape we follow: stdio MCP, read
`~/Library/Messages/chat.db` directly, send via AppleScript, gate with a
local allowlist, bypass the gate for self-chat. Credit for the shape goes
there. Bugs and design choices here are ours.

Differences worth knowing about if you're evaluating both:

- **Rust**, single static binary via `cargo install` (the upstream uses Bun).
- **LLM-CLI-agnostic**: one server, three tools, drop-in for Codex CLI,
  Claude Code, or any MCP-over-stdio client.
- Minimal tool surface (`reply`, `list_messages`, `read_message`). No
  channel-event push model; clients poll via `list_messages`.

[upstream]: https://github.com/anthropics/claude-plugins-official/tree/main/external_plugins/imessage

## License

Dual MIT OR Apache-2.0.
