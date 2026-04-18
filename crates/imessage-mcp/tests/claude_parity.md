# Claude parity integration test

`cargo test` discovers `tests/claude_parity.rs`, but the test is opt-in and
returns early unless `DKDC_IO_RUN_CLAUDE_LIVE_TEST=1` is set.

Prereqs:

- macOS
- `claude`, `tmux`, and `osascript` on `PATH`
- Messages.app signed in
- Full Disk Access for the host process that runs `imessage-mcp`
- `~/.config/dkdc-io/imessage/access.toml` configured with `self.chat_id`
- `imessage-mcp check` printing a non-`(unset)` `self.chat_id`

Run:

```sh
DKDC_IO_RUN_CLAUDE_LIVE_TEST=1 cargo test -p imessage-mcp claude_can_round_trip_from_imessage -- --nocapture
```

Useful knobs:

- `DKDC_IO_CLAUDE_SETTLE_SECS=5`: wait longer after the self-send lands in `chat.db`
- `DKDC_IO_CLAUDE_LIVE_TIMEOUT_SECS=60`: give Claude more time before capturing the tmux pane
