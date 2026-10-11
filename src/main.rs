mod browser;
mod daemon;
mod mcp;
mod protocol;
mod runtime;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use protocol::*;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Lazy, isolated Tor Browser automation. Automation is detectable; this is not a stealth tool."
)]
struct Cli {
    #[arg(long, global = true, default_value = "default")]
    session: String,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: CliCommand,
}
#[derive(Subcommand)]
enum CliCommand {
    /// Serve typed MCP tools over standard input and output. No browser starts until a browsing tool is called.
    Mcp,
    /// Start the local daemon in the foreground.
    Daemon,
    /// Shut down the local daemon and all its owned sessions.
    Shutdown,
    /// List commands and their JSON schemas, without starting anything.
    Tools,
    /// Open a complete HTTP(S) URL through Tor Browser.
    Open {
        url: String,
        #[arg(long)]
        headed: bool,
    },
    Snapshot {
        #[arg(long)]
        interactive: bool,
    },
    Read,
    Links {
        #[arg(long)]
        onion_only: bool,
    },
    Click {
        target: String,
    },
    Fill {
        target: String,
        text: String,
    },
    Press {
        key: String,
    },
    Wait {
        text: String,
        #[arg(long, default_value_t = 10000)]
        timeout_ms: u64,
    },
    Screenshot {
        path: PathBuf,
    },
    Tabs {
        #[arg(default_value = "list")]
        action: String,
        tab: Option<String>,
        #[arg(long)]
        url: Option<String>,
    },
    Navigate {
        action: String,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    Discover {
        url: String,
    },
    TorStatus,
    Sessions,
    Close,
    /// Invoke an MCP command by name with a JSON argument object.
    Call {
        command: String,
        arguments: String,
    },
    #[command(hide = true)]
    InternalDriver {
        #[arg(long)]
        geckodriver: PathBuf,
    },
}
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let cli = Cli::parse();
    if let CliCommand::InternalDriver { geckodriver } = &cli.command {
        return runtime::internal_driver(geckodriver).await;
    }
    if matches!(cli.command, CliCommand::Tools) {
        print_value(serde_json::to_value(mcp::definitions())?, cli.json);
        return Ok(());
    }
    let config = runtime::Config::from_env()?;
    match cli.command {
        CliCommand::Mcp => mcp::run(config).await,
        CliCommand::Daemon => daemon::serve(config).await,
        CliCommand::Shutdown => {
            let data = daemon::request(&config, daemon::Request::Shutdown).await?;
            print_value(data, cli.json);
            Ok(())
        }
        command => {
            let mut screenshot = None;
            let session = cli.session;
            let action = match command {
                CliCommand::Open { url, headed } => Action::BrowserOpen(OpenArgs {
                    session,
                    url,
                    headed,
                }),
                CliCommand::Snapshot { interactive } => Action::BrowserSnapshot(SnapshotArgs {
                    session,
                    interactive,
                }),
                CliCommand::Read => Action::BrowserRead(SessionArgs { session }),
                CliCommand::Links { onion_only } => Action::BrowserLinks(LinksArgs {
                    session,
                    onion_only,
                }),
                CliCommand::Click { target } => {
                    Action::BrowserClick(TargetArgs { session, target })
                }
                CliCommand::Fill { target, text } => Action::BrowserFill(FillArgs {
                    session,
                    target,
                    text,
                }),
                CliCommand::Press { key } => Action::BrowserPress(PressArgs { session, key }),
                CliCommand::Wait { text, timeout_ms } => Action::BrowserWait(WaitArgs {
                    session,
                    text,
                    timeout_ms,
                }),
                CliCommand::Screenshot { path } => {
                    screenshot = Some(path);
                    Action::BrowserScreenshot(SessionArgs { session })
                }
                CliCommand::Tabs { action, tab, url } => Action::BrowserTabs(TabsArgs {
                    session,
                    action: serde_json::from_value(serde_json::json!(action))
                        .context("use tabs list, new, select or close")?,
                    tab,
                    url,
                }),
                CliCommand::Navigate { action } => Action::BrowserNavigate(NavigateArgs {
                    session,
                    action: serde_json::from_value(serde_json::json!(action))
                        .context("use navigate back, forward or reload")?,
                }),
                CliCommand::Search { query, limit } => Action::OnionSearch(SearchArgs {
                    session,
                    query,
                    limit,
                }),
                CliCommand::Discover { url } => {
                    Action::OnionDiscover(DiscoverArgs { session, url })
                }
                CliCommand::TorStatus => Action::TorStatus(SessionArgs { session }),
                CliCommand::Sessions => Action::SessionList(EmptyArgs {}),
                CliCommand::Close => Action::SessionClose(SessionArgs { session }),
                CliCommand::Call { command, arguments } => {
                    decode_action(&command, serde_json::from_str(&arguments)?)?
                }
                _ => unreachable!(),
            };
            action.validate()?;
            let mut value = daemon::request(&config, daemon::Request::Action(action)).await?;
            if let Some(path) = screenshot {
                use base64::Engine;
                let data = value
                    .get("base64")
                    .and_then(serde_json::Value::as_str)
                    .context("missing screenshot")?;
                let png = base64::engine::general_purpose::STANDARD.decode(data)?;
                // Never overwrite an existing file or follow a destination symlink.
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?
                    .write_all(&png)?;
                value = serde_json::json!({"path": path, "bytes": png.len()});
            }
            print_value(value, cli.json);
            Ok(())
        }
    }
}
fn print_value(value: serde_json::Value, json: bool) {
    if json {
        println!("{value}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON value serializes")
        );
    }
}
