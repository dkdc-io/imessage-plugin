# Demo scripts

Human-runnable end-to-end demos that match the live round-trip capture shape.

## Scripts

- `scripts/demo-claude-imessage.sh`: starts Claude idle in tmux, self-sends a trigger iMessage, polls `dkdc-io-imessage --stdio` for the new inbound message, injects the raw message text into Claude's pane, and captures `/tmp/claude-tui-capture.txt`.
- `scripts/demo-codex-imessage.sh`: starts Codex idle in tmux, drops one channel envelope into `CODEX_CHANNEL_DIR/inbox/001.json`, lets Codex process it with the `dkdc-io-imessage` MCP server, and captures `/tmp/codex-tui-capture.txt`.

## Prereqs

- macOS
- `dkdc-io-imessage`, `tmux`, `claude`, `codex`, `osascript`, and `python3` on `PATH`
- Messages.app signed in
- Full Disk Access for the host process that will read `~/Library/Messages/chat.db`
- `~/.config/dkdc-io/imessage/access.toml` configured with an allowlist and `self.chat_id`
- `dkdc-io-imessage check` printing a non-`(unset)` `self.chat_id`
- Claude MCP config path: `~/.claude.json` for global config, or a project `.mcp.json`
- Codex MCP config path: `~/.codex/config.toml`

## Run

```sh
scripts/demo-claude-imessage.sh
scripts/demo-codex-imessage.sh
```

Optional overrides:

- `SESSION=<name>`: tmux session name
- `OUT=<path>`: capture file path
- `MCP_CONFIG=<path>`: Claude-only JSON config path
- `CHANNEL_DIR=<path>`: Codex-only channel root
- `SETTLE_SECS=<n>`: startup delay before injection
- `TIMEOUT_SECS=<n>`: wait before capture

## Expect

- Claude demo: the script sends `hi claude! run \`cal\`...`, the watcher sees that inbound iMessage through the MCP server, injects the exact message text into Claude, Claude runs `cal`, and Claude calls `mcp__imessage__reply`.
- Codex demo: the script writes `hi codex! run \`cal\`...` into the channel inbox, Codex surfaces it as a channel envelope, runs `cal`, and calls `imessage.reply`.
- Both demos leave a full tmux capture in `/tmp` for screenshots, docs, or fixture updates.
