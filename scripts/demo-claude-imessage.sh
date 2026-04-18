#!/usr/bin/env bash
# Demo: claude TUI sits idle. A trigger iMessage arrives (self-sent via
# osascript here to keep the demo one-shot). A tiny watcher polls the server's
# list_messages via JSON-RPC, detects the new inbound message, and injects its
# text as a prompt into the claude tmux pane. Claude reacts, runs cal, calls
# imessage.reply.
#
# Capture mirrors the codex flow: prompt in, tool calls, reply.
set -euo pipefail

SESSION="${SESSION:-test-claude-imessage}"
OUT="${OUT:-/tmp/claude-tui-capture.txt}"
MCP_CONFIG="${MCP_CONFIG:-/tmp/bare-claude-mcp.json}"

if [[ ! -f "$MCP_CONFIG" ]]; then
  cat > "$MCP_CONFIG" <<'JSON'
{
  "mcpServers": {
    "imessage": {
      "type": "stdio",
      "command": "dkdc-io-imessage",
      "args": ["--stdio"]
    }
  }
}
JSON
fi

HANDLE="$(dkdc-io-imessage check | awk '/^self\.chat_id:/ {print $2}' | sed 's/.*;-;//')"
[[ -n "$HANDLE" ]] || { echo "could not resolve self handle" >&2; exit 1; }

# Helper: fetch the N most recent messages from the server via MCP stdio.
# Returns JSON array.
list_messages_json() {
  local limit="${1:-5}"
  (
    echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"0.1"}}}'
    sleep 0.3
    echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"list_messages\",\"arguments\":{\"limit\":${limit}}}}"
    sleep 0.6
  ) | dkdc-io-imessage --stdio 2>/dev/null | \
    awk '/"id":1/ {print; exit}' | \
    python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(d["result"]["content"][0]["text"])'
}

tmux kill-session -t "$SESSION" 2>/dev/null || true

# Start claude TUI idle.
tmux new-session -d -s "$SESSION" -x 120 -y 40
tmux send-keys -t "$SESSION" \
  "cd /private/tmp && claude --mcp-config $MCP_CONFIG --allowedTools 'mcp__imessage__reply,Bash'" \
  Enter

sleep 8
tmux send-keys -t "$SESSION" Enter  # trust prompt
sleep 3

# Snapshot the most recent inbound message id BEFORE firing the trigger.
BASELINE_ID="$(list_messages_json 5 | python3 -c '
import json, sys
msgs = json.loads(sys.stdin.read()).get("messages", [])
inbound = [m for m in msgs if not m.get("is_from_me", False)]
print(inbound[0]["id"] if inbound else "")
')"

# Fire the trigger iMessage via osascript self-send.
TRIGGER='hi claude! run `cal` and text me the output. use the default chat_id (no need to specify).'
osascript <<OSASCRIPT
tell application "Messages"
  set targetService to 1st account whose service type = iMessage
  set targetBuddy to buddy "$HANDLE" of targetService
  send "$TRIGGER" to targetBuddy
end tell
OSASCRIPT

# Poll for the new inbound message and grab its text.
NEW_TEXT=""
for _ in $(seq 1 30); do
  sleep 1
  NEW_TEXT="$(list_messages_json 5 | python3 -c '
import json, os, sys
baseline = os.environ.get("BASELINE_ID", "")
msgs = json.loads(sys.stdin.read()).get("messages", [])
inbound = [m for m in msgs if not m.get("is_from_me", False)]
for m in inbound:
    if m["id"] != baseline:
        print(m["text"])
        break
' BASELINE_ID="$BASELINE_ID")"
  [[ -n "$NEW_TEXT" ]] && break
done

if [[ -z "$NEW_TEXT" ]]; then
  echo "no new inbound message detected within 30s" >&2
  tmux kill-session -t "$SESSION"
  exit 1
fi

# Inject raw message text into claude's pane as a user prompt.
tmux send-keys -t "$SESSION" "$NEW_TEXT"
sleep 0.5
tmux send-keys -t "$SESSION" Enter

# Give claude room to run cal + call reply.
sleep 30

tmux capture-pane -t "$SESSION" -p -S - > "$OUT"
echo "captured $(wc -l < "$OUT") lines to $OUT"

tmux kill-session -t "$SESSION"
