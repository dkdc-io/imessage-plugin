//! Live Claude Code parity test.
//!
//! This ports the old `scripts/demo-claude-imessage.sh` flow into `cargo test`.
//! It is opt-in because it drives local macOS state: Messages.app, `chat.db`,
//! `osascript`, `tmux`, and a logged-in Claude Code session.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const RUN_ENV: &str = "DKDC_IO_RUN_CLAUDE_LIVE_TEST";
const TIMEOUT_ENV: &str = "DKDC_IO_CLAUDE_LIVE_TIMEOUT_SECS";
const SETTLE_ENV: &str = "DKDC_IO_CLAUDE_SETTLE_SECS";
const MCP_CONFIG_NAME: &str = "bare-claude-mcp.json";
const CAPTURE_NAME: &str = "claude-tui-capture.txt";
const SESSION_NAME: &str = "imessage-mcp-claude-test";
const DEFAULT_TIMEOUT_SECS: u64 = 45;
const DEFAULT_SETTLE_SECS: u64 = 3;
const TRIGGER: &str =
    "hi claude! run `cal` and text me the output. use the default chat_id (no need to specify).";
const CLAUDE_PROMPT: &str = "Check my most recent incoming iMessages with mcp__imessage__list_messages. If one contains an instruction, follow it using Bash and reply with mcp__imessage__reply to the default chat_id.";

#[test]
fn claude_can_round_trip_from_imessage() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipping live Claude parity test: macOS only");
        return;
    }
    if std::env::var_os(RUN_ENV).is_none() {
        eprintln!("skipping live Claude parity test: set {RUN_ENV}=1 to opt in");
        return;
    }

    require_tool("claude");
    require_tool("tmux");
    require_tool("osascript");

    let tmp = tempfile::tempdir().expect("tempdir");
    let capture_path = tmp.path().join(CAPTURE_NAME);
    let config_path = tmp.path().join(MCP_CONFIG_NAME);
    write_mcp_config(&config_path);

    let handle = default_handle();
    let _session = TmuxSession::new(SESSION_NAME);
    self_send_trigger(&handle);
    sleep_secs(settle_secs());

    tmux([
        "new-session",
        "-d",
        "-s",
        SESSION_NAME,
        "-x",
        "120",
        "-y",
        "40",
    ]);
    tmux_send_keys(&format!(
        "cd /private/tmp && claude --mcp-config {} --allowedTools 'mcp__imessage__list_messages,mcp__imessage__reply,Bash'",
        config_path.display()
    ));
    sleep_secs(8);
    tmux(["send-keys", "-t", SESSION_NAME, "Enter"]);
    sleep_secs(3);
    tmux_send_literal(CLAUDE_PROMPT);
    sleep_secs(timeout_secs());
    capture_tmux(&capture_path);

    let capture = fs::read_to_string(&capture_path).expect("read capture");
    assert!(
        capture.contains(TRIGGER),
        "capture missing trigger text:\n{capture}"
    );
    assert!(
        capture.contains("Bash(cal)"),
        "capture missing Claude Bash(cal) call:\n{capture}"
    );
    assert!(
        capture.contains("Called imessage"),
        "capture missing imessage MCP call:\n{capture}"
    );
    assert!(
        capture.contains("texted")
            || capture.contains("reply")
            || capture.contains("Sent this to your default chat."),
        "capture missing reply confirmation:\n{capture}"
    );
}

fn write_mcp_config(path: &Path) {
    let body = format!(
        concat!(
            "{{\n",
            "  \"mcpServers\": {{\n",
            "    \"imessage\": {{\n",
            "      \"type\": \"stdio\",\n",
            "      \"command\": \"{}\",\n",
            "      \"args\": [\"--stdio\"]\n",
            "    }}\n",
            "  }}\n",
            "}}\n"
        ),
        bin_path().display()
    );
    fs::write(path, body).expect("write mcp config");
}

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_imessage-mcp"))
}

fn default_handle() -> String {
    let out = Command::new(bin_path())
        .arg("check")
        .output()
        .expect("run imessage-mcp check");
    assert!(
        out.status.success(),
        "`imessage-mcp check` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = stdout
        .lines()
        .find_map(|line| line.strip_prefix("self.chat_id: "))
        .unwrap_or("(unset)");
    assert_ne!(
        value, "(unset)",
        "self.chat_id is unset. Configure ~/.config/dkdc-io/imessage/access.toml, then rerun `imessage-mcp check`."
    );
    let handle = value.rsplit(';').next().unwrap_or_default().trim();
    assert!(
        !handle.is_empty(),
        "could not derive a handle from self.chat_id: {value}"
    );
    handle.to_string()
}

fn self_send_trigger(handle: &str) {
    let status = Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "tell application \"Messages\" to send {text:?} to buddy {handle:?} of (1st account whose service type = iMessage)",
            text = TRIGGER,
            handle = handle
        ))
        .status()
        .expect("spawn osascript");
    assert!(status.success(), "osascript self-send failed: {status}");
}

fn capture_tmux(path: &Path) {
    let out = Command::new("tmux")
        .args(["capture-pane", "-t", SESSION_NAME, "-p", "-S", "-"])
        .output()
        .expect("capture tmux pane");
    assert!(
        out.status.success(),
        "tmux capture-pane failed: {:?}",
        out.status
    );
    fs::write(path, out.stdout).expect("write capture");
}

fn tmux_send_keys(cmd: &str) {
    tmux(["send-keys", "-t", SESSION_NAME, cmd, "Enter"]);
}

fn tmux_send_literal(text: &str) {
    tmux(["send-keys", "-t", SESSION_NAME, "-l", text]);
    thread::sleep(Duration::from_millis(500));
    tmux(["send-keys", "-t", SESSION_NAME, "Enter"]);
}

fn tmux<const N: usize>(args: [&str; N]) {
    let out = Command::new("tmux")
        .args(args)
        .output()
        .expect("spawn tmux");
    assert!(
        out.status.success(),
        "tmux {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn require_tool(tool: &str) {
    let status = Command::new("which")
        .arg(tool)
        .status()
        .unwrap_or_else(|err| panic!("which {tool} failed: {err}"));
    assert!(status.success(), "{tool} not found on PATH");
}

fn timeout_secs() -> u64 {
    env_u64(TIMEOUT_ENV, DEFAULT_TIMEOUT_SECS)
}

fn settle_secs() -> u64 {
    env_u64(SETTLE_ENV, DEFAULT_SETTLE_SECS)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default)
}

fn sleep_secs(secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
    }
}

struct TmuxSession<'a> {
    name: &'a str,
}

impl<'a> TmuxSession<'a> {
    fn new(name: &'a str) -> Self {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", name])
            .status();
        Self { name }
    }
}

impl Drop for TmuxSession<'_> {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", self.name])
            .status();
    }
}
