# dkdc-io-imessage

An iMessage MCP plugin. Lets an LLM CLI (Codex CLI, Claude Code, or any
JSON-RPC-over-stdio MCP client) read and send iMessages on macOS.

Three tools:

- `reply(chat_id, text)` — send an iMessage.
- `list_messages(query, limit)` — search recent messages.
- `read_message(id)` — fetch one message by GUID.

Fail-closed by default: an empty allowlist makes every tool call error out with
a pointer back to the config file.

## Install

```sh
# no rust? one line:
curl -LsSf https://dkdc.sh/imessage-plugin/install.sh | sh

# already have cargo:
cargo install dkdc-io-imessage
```

Either way you end up with the `dkdc-io-imessage` binary on your `$PATH`. The
first script installs `rustup` if it isn't present, then runs `cargo install`.

## macOS prerequisites

1. **Full Disk Access**. The binary reads `~/Library/Messages/chat.db`. Grant
   it to whatever you're launching the MCP server from (your terminal, Codex,
   Claude Code). System Settings -> Privacy & Security -> Full Disk Access.
2. **Messages.app signed in**. Sending goes through `osascript` -> Messages,
   so the app has to be running and logged in to your Apple ID.

## Allowlist

Edit `~/.config/dkdc-io/imessage/access.toml`:

```toml
# Other handles the LLM can interact with via DMs. Must appear BEFORE [self].
allow_from = [
  "friend@example.com",
]

# Your own chat GUID. Lets an LLM `reply` with no chat_id to text you.
# Copy it from chat.db (`SELECT guid FROM chat WHERE style = 45`) or look at
# the URL bar in Messages after selecting your "note to self" chat.
[self]
chat_id  = "iMessage;-;+15551234567"
handles  = ["+15551234567", "you@icloud.com"]
```

Verify with:

```sh
dkdc-io-imessage check
```

Empty allowlist is intentional. Any tool call returns:

```
allowlist is empty. dkdc-io-imessage is fail-closed by default. Edit
~/.config/dkdc-io/imessage/access.toml to add `self.chat_id` and/or
`allow_from` handles, then retry.
```

## Configure the client

### Codex CLI

Preferred path:

```sh
codex mcp add imessage -- dkdc-io-imessage --stdio
codex mcp list
```

Direct edit works too, for reference:

```toml
[mcp_servers.imessage]
command = "dkdc-io-imessage"
args    = ["--stdio"]
```

You should see `imessage` in the MCP list, with `reply`, `list_messages`, and
`read_message` available on the next Codex start.

### Claude Code

Preferred path:

```sh
claude mcp add imessage dkdc-io-imessage --stdio
claude mcp list
```

Direct edit works too, for reference. Add to `~/.claude.json` (or per-project
`.mcp.json`):

```json
{
  "mcpServers": {
    "imessage": {
      "type": "stdio",
      "command": "dkdc-io-imessage",
      "args": ["--stdio"]
    }
  }
}
```

You should see `imessage` in the MCP list on the next Claude start.

## Example prompts

After setup:

- "text myself 'build done'"
- "what did I text Friend today?"
- "read the last message from my note-to-self chat"

## Config

| Env var                 | Purpose                                                 |
|-------------------------|---------------------------------------------------------|
| `DKDC_IO_ACCESS_FILE`   | Override the allowlist TOML path.                       |
| `DKDC_IO_STATE_DIR`     | Override the config dir (default `~/.config/dkdc-io/imessage`). |
| `DKDC_IO_CHAT_DB`       | Override the chat.db path. Useful for tests.            |
| `DKDC_IO_LOG`           | Tracing filter (`warn`, `info`, `debug`, ...).          |

## Security posture

- Allowlist is the only access surface. Empty = fail closed.
- `reply` rejects chat GUIDs that don't resolve through an allowlisted
  handle (or `self.chat_id`).
- `read_message` / `list_messages` never surface rows from non-allowlisted
  chats.
- osascript is invoked with `text` / `chat_guid` as argv items; the AppleScript
  body is a fixed string. There is no shell or string-concatenation path for
  user-controlled input. See `tests/injection.rs` for the anti-regression.

## Prior art

Inspired by Anthropic's official iMessage plugin for Claude Code
([anthropics/claude-plugins-official/external_plugins/imessage][upstream]),
which pioneered the chat.db + AppleScript + allowlist shape. This is an
independent Rust implementation with an LLM-CLI-agnostic surface (Codex CLI,
Claude Code, or any MCP-over-stdio client).

[upstream]: https://github.com/anthropics/claude-plugins-official/tree/main/external_plugins/imessage

## License

MIT OR Apache-2.0.
