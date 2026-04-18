# Changelog

All notable changes to `dkdc-io-imessage` are documented here. Format loosely
follows [Keep a Changelog](https://keepachangelog.com/) and the project uses
[SemVer](https://semver.org/).

## [0.1.6] - 2026-04-18

### Changed

- rename the GitHub repo from `imessage-plugin` to `imessage-mcp`
- keep the crate and binary names as `dkdc-io-imessage`
- update docs, screenshots, scripts, and public URLs to the new name
- docs: real end-to-end Claude capture + round-trip screenshot + scripts/
- note the Codex fork requirement for `codex mcp add`
- make the prior-art arc explicit: upstream TypeScript/Bun MCP server, direct
  port, typedstream truncation bug, echo-tracker bug, fixes, then Rust rewrite
  for correctness
- the existing `dkdc-io-imessage` crate remains the published crates.io name

## [0.1.5] - 2026-04-18

### Added

- Ported the old Netsky `demo-claude-imessage.sh` flow into an opt-in live
  integration test at `crates/imessage-mcp/tests/claude_parity.rs`, with
  prerequisites and run instructions in `tests/claude_parity.md`.
- Added a repo README "Same MCP server, Claude Code" section with the captured
  Claude TUI round-trip, to match the Codex demo block.

## [0.1.4] - 2026-04-18

### Changed

- Rewrote the repo README around a live Codex-to-iMessage round-trip demo.
- Added the product screenshot to the repo so the GitHub page carries the
  terminal and phone flow directly.

## [0.1.3] - 2026-04-18

### Added

- README install section now leads with the
  `curl -LsSf https://dkdc.sh/imessage-mcp/install.sh | sh`
  one-liner for users who do not already have `cargo`. The script installs
  `rustup` if absent, then runs `cargo install imessage-mcp`.

## [0.1.2] - 2026-04-18

### Added

- Prior-art attribution to Anthropic's official iMessage MCP server for Claude
  Code ([`anthropics/claude-plugins-official/external_plugins/imessage`][upstream]),
  the TypeScript/Bun implementation that pioneered the chat.db + AppleScript +
  allowlist shape. Acknowledgment lands in both READMEs and in the crate's
  top-level module docstring.

[upstream]: https://github.com/anthropics/claude-plugins-official/tree/main/external_plugins/imessage

## [0.1.1] - 2026-04-17

### Fixed

- `access.toml` example ordering. The published 0.1.0 README showed `[self]`
  before `allow_from`, which TOML treats as nesting `allow_from` inside the
  `[self]` table and silently drops it from the allowlist. The example now
  places `allow_from` first.

### Changed

- Crate and repo READMEs polished for standalone positioning.
- Module docstrings trimmed; removed references to features that only make
  sense inside a larger system.

## [0.1.0] - 2026-04-17

Initial release.

- Stdio MCP server (JSON-RPC 2.0) exposing three tools:
  - `reply(chat_id, text)` — send an iMessage via `osascript` → Messages.app.
  - `list_messages(query, limit)` — search allowlisted chats in `chat.db`.
  - `read_message(id)` — fetch one message body by GUID.
- Allowlist at `~/.config/dkdc-io/imessage/access.toml`. Empty allowlist =
  fail closed with a pointer back to the config file.
- Injection-safe osascript send path (argv, not interpolation), covered by
  structural + fuzz tests in `tests/injection.rs`.
- End-to-end stdio smoke test in `tests/stdio_smoke.rs`.
- macOS only. Dual-licensed MIT OR Apache-2.0.
