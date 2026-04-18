//! Binary entry point. No clap — one flag surface: `--stdio` (the default) runs
//! the MCP server on stdin/stdout. Without args we also run the server, since
//! that is the only thing this binary does.

use std::process::ExitCode;

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--stdio"] => run_server(),
        ["-h"] | ["--help"] => {
            print_help();
            ExitCode::SUCCESS
        }
        ["-V"] | ["--version"] => {
            println!("imessage-mcp {}", crate::VERSION);
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

fn print_help() {
    println!(
        r#"imessage-mcp {version}

iMessage MCP server. Exposes reply / list_messages / read_message tools to an
MCP client (Codex CLI, Claude Code, ...).

Usage:
  imessage-mcp [--stdio]    run the MCP server on stdin/stdout (default)
  imessage-mcp check        print the loaded allowlist and config paths
  imessage-mcp --version    print version and exit
  imessage-mcp --help       show this help

Config:
  ~/.config/dkdc-io/imessage/access.toml     allowlist
  DKDC_IO_ACCESS_FILE=<path>                 override the path
  DKDC_IO_CHAT_DB=<path>                     override chat.db path (tests)

See the README at https://github.com/dkdc-io/imessage-mcp for setup.
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

fn run_server() -> ExitCode {
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
    match rt.block_on(crate::mcp::serve()) {
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
