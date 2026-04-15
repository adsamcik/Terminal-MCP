use anyhow::Result;

mod ansi;
mod error_detection;
mod keys;
mod logging;
mod screenshot;
mod scrollback;
mod server;
mod session;
mod shell_integration;
mod terminal;
mod tools;
pub mod wsl;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing to stderr (stdout is used for MCP stdio transport)
    let env_filter = tracing_subscriber::EnvFilter::try_from_env("TERMINAL_MCP_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .json()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "terminal-mcp starting");

    server::run().await
}
