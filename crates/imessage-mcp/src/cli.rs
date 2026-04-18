//! Binary entry point. Flag surface:
//! - `--stdio` (default): run the MCP server on stdin/stdout.
//! - `--watch` / `--no-watch`: enable/disable push-mode watcher. Watch is ON
//!   by default when running as an MCP server, since the whole point of the
//!   binary is for inbound iMessages to land in the session automatically.
//!   Set `DKDC_IO_WATCH=0` or pass `--no-watch` to suppress.

use std::process::ExitCode;

use crate::mcp::ServeOptions;

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rest: Vec<&str> = Vec::new();
    let mut watch_override: Option<bool> = None;
    for a in &args {
        match a.as_str() {
            "--watch" => watch_override = Some(true),
            "--no-watch" => watch_override = Some(false),
            other => rest.push(other),
        }
    }
    match rest.as_slice() {
        [] | ["--stdio"] => run_server(resolve_watch(watch_override)),
        ["-h"] | ["--help"] => {
            print_help();
            ExitCode::SUCCESS
        }
        ["-V"] | ["--version"] => {
            println!("dkdc-io-imessage {}", crate::VERSION);
            ExitCode::SUCCESS
        }
        ["check"] => check_access(),
        _ => {
            eprintln!("unknown arguments: {:?}", args);
            print_help();
            ExitCode::from(2)
        }
    }
}

/// Resolve the effective `watch` setting. Default ON. `--watch`/`--no-watch`
/// override env. `DKDC_IO_WATCH=0` or `DKDC_IO_WATCH=false` disables when no
/// CLI override is present.
fn resolve_watch(cli_override: Option<bool>) -> bool {
    if let Some(v) = cli_override {
        return v;
    }
    !matches!(
        std::env::var("DKDC_IO_WATCH").ok().as_deref(),
        Some("0") | Some("false") | Some("no")
    )
}

fn print_help() {
    println!(
        r#"dkdc-io-imessage {version}

iMessage MCP server. Exposes reply / list_messages / read_message tools to an
MCP client (Codex CLI, Claude Code, ...) AND pushes inbound iMessages into the
session as MCP channel notifications so the LLM reacts without polling.

Usage:
  dkdc-io-imessage [--stdio] [--watch|--no-watch]
                              run the MCP server on stdin/stdout (default).
                              Watch mode is ON by default — it tails chat.db
                              and emits notifications/claude/channel for new
                              allowlisted inbound messages.
  dkdc-io-imessage check      print the loaded allowlist and config paths
  dkdc-io-imessage --version  print version and exit
  dkdc-io-imessage --help     show this help

Config:
  ~/.config/dkdc-io/imessage/access.toml     allowlist
  DKDC_IO_ACCESS_FILE=<path>                 override the path
  DKDC_IO_CHAT_DB=<path>                     override chat.db path (tests)
  DKDC_IO_WATCH=0                            disable watch mode
  DKDC_IO_WATCH_INTERVAL_MS=<n>              poll interval (default 750)
  CODEX_CHANNEL_DIR=<path>                   also drop codex envelopes here

See the README at https://github.com/dkdc-io/imessage for setup.
"#,
        version = crate::VERSION,
    );
}

fn check_access() -> ExitCode {
    init_tracing();
    let path = match crate::access::access_file() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let access = crate::access::load();
    println!("access file: {}", path.display());
    println!(
        "self.chat_id: {}",
        access.self_chat_id.as_deref().unwrap_or("(unset)")
    );
    println!("self.handles:");
    if access.self_handles.is_empty() {
        println!("  (none)");
    } else {
        for h in &access.self_handles {
            println!("  - {h}");
        }
    }
    println!("allow_from:");
    if access.allow_from.is_empty() {
        println!("  (none)");
    } else {
        for h in &access.allow_from {
            println!("  - {h}");
        }
    }
    if access.is_empty() {
        println!();
        println!("NOTE: allowlist is empty. All tools will fail closed.");
        println!(
            "      Edit {} to add handles or a self chat_id.",
            path.display()
        );
    }
    ExitCode::SUCCESS
}

fn run_server(watch: bool) -> ExitCode {
    init_tracing();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to build tokio runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let opts = ServeOptions { watch };
    match rt.block_on(crate::mcp::serve_with(opts)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("server error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("DKDC_IO_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
