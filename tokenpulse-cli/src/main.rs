mod commands;
mod tui;

use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;
use tokenpulse_core::config::ConfigManager;

#[derive(Parser)]
#[clap(name = "tokenpulse")]
#[clap(about = "Token usage and quota dashboard for coding agents")]
#[clap(version)]
struct Cli {
    #[clap(long)]
    since: Option<String>,

    #[clap(short, long)]
    provider: Option<String>,

    #[clap(long)]
    refresh_days: Option<String>,

    #[clap(long)]
    refresh_pricing: bool,

    #[clap(long)]
    rebuild_all: bool,

    /// Emit CSV output (daily or models). Example: --csv daily
    #[clap(long, value_enum, conflicts_with = "json", conflicts_with = "tui")]
    csv: Option<CsvFormat>,

    /// Emit JSON output instead of text or the interactive dashboard.
    #[clap(long, conflicts_with = "tui")]
    json: bool,

    /// Force the interactive dashboard even if auto-detection would stay in text mode.
    #[clap(long, conflicts_with = "no_tui")]
    tui: bool,

    /// Force plain-text output instead of the interactive dashboard.
    #[clap(long, conflicts_with = "tui")]
    no_tui: bool,

    /// Write usage startup timing to a new log file under ~/.local/share/tokenpulse/log/.
    #[clap(long)]
    log: bool,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Clone, ValueEnum)]
enum CsvFormat {
    Daily,
    Models,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive setup wizard
    Init {
        /// Skip interactive prompts, auto-detect and enable found providers
        #[clap(long)]
        default: bool,
    },
    Config {
        #[clap(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    Show,
    Enable {
        provider: String,
    },
    Disable {
        provider: String,
    },
    /// Set a config value (e.g. quota_display_mode=used)
    Set {
        /// Key=value pair
        setting: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var_os("RUST_LOG").is_some() || std::env::var_os("TOKENPULSE_LOG").is_some() {
        let _ = tracing_subscriber::fmt::try_init();
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { default }) => {
            commands::init::run(default)?;
        }
        Some(Commands::Config { action }) => {
            commands::config::run(action)?;
        }
        None => {
            check_config_exists();
            commands::usage::run(
                cli.since,
                cli.provider,
                cli.refresh_days,
                cli.refresh_pricing,
                cli.rebuild_all,
                if cli.json || cli.csv.is_some() {
                    false
                } else {
                    resolve_tui_mode(cli.tui, cli.no_tui)?
                },
                cli.json,
                cli.csv.map(|format| match format {
                    CsvFormat::Daily => "daily".to_string(),
                    CsvFormat::Models => "models".to_string(),
                }),
                cli.log,
            )
            .await?;
        }
    }

    Ok(())
}

fn check_config_exists() {
    let config_manager = ConfigManager::new();
    if !config_manager.exists() {
        eprintln!(
            "Hint: No config found. Creating default at {}\n      Run `tokenpulse init` for guided setup, or edit the file directly.\n",
            config_manager.config_path().display()
        );
    }
}

fn resolve_tui_mode(tui: bool, no_tui: bool) -> anyhow::Result<bool> {
    let interactive_tui = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("TERM")
            .map(|term| term != "dumb")
            .unwrap_or(true);

    if tui && !interactive_tui {
        anyhow::bail!(
            "--tui requires an interactive terminal; run in a terminal or use `--no-tui` for plain-text output"
        );
    }

    Ok(if no_tui {
        false
    } else if tui {
        true
    } else {
        interactive_tui
    })
}
