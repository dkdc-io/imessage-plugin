#!/usr/bin/env bash
# Demo: codex TUI sits idle. A trigger channel envelope lands in
# CODEX_CHANNEL_DIR/inbox. Codex surfaces that envelope as a user turn, runs
# cal, calls imessage.reply, and the pane capture shows the full round-trip.
set -euo pipefail

SESSION="${SESSION:-test-codex-imessage}"
OUT="${OUT:-/tmp/codex-tui-capture.txt}"
CHANNEL_DIR="${CHANNEL_DIR:-/tmp/codex-imessage-channel}"
SETTLE_SECS="${SETTLE_SECS:-8}"
TIMEOUT_SECS="${TIMEOUT_SECS:-30}"
TRIGGER='hi codex! run `cal` and text me the output. use the default chat_id (no need to specify).'

tmux kill-session -t "$SESSION" 2>/dev/null || true
rm -rf "$CHANNEL_DIR"
mkdir -p "$CHANNEL_DIR/inbox" "$CHANNEL_DIR/outbox" "$CHANNEL_DIR/processed"

tmux new-session -d -s "$SESSION" -x 120 -y 40
tmux send-keys -t "$SESSION" \
  "cd /private/tmp && CODEX_CHANNEL_DIR=$CHANNEL_DIR codex --dangerously-bypass-approvals-and-sandbox -C /private/tmp -c 'mcp_servers={imessage={command=\"dkdc-io-imessage\",args=[\"--stdio\"]}}'" \
  Enter

sleep "$SETTLE_SECS"

cat > "$CHANNEL_DIR/inbox/001.json" <<JSON
{"from":"owner","to":"codex","id":"001","ts":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","text":"$TRIGGER"}
JSON

sleep "$TIMEOUT_SECS"

tmux capture-pane -t "$SESSION" -p -S - > "$OUT"
echo "captured $(wc -l < "$OUT") lines to $OUT"

tmux kill-session -t "$SESSION"
