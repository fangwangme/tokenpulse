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

    #[clap(long)]
    refresh_days: Option<String>,

    #[clap(long)]
    refresh_pricing: bool,

    #[clap(long)]
    rebuild_all: bool,

    /// Emit CSV output (daily or models). Example: --csv daily
    #[clap(long, value_enum, conflicts_with = "json")]
    csv: Option<CsvFormat>,

    /// Emit JSON output instead of text or the interactive dashboard.
    #[clap(long)]
    json: bool,

    /// Force plain-text output instead of the interactive dashboard.
    #[clap(long)]
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
    /// Fire a sample quota recovery notification at the configured level.
    ///
    /// Real recoveries only happen when an exhausted window resets while the
    /// TUI is open, which makes the feature nearly impossible to verify by
    /// hand; this exercises the same code path on demand.
    TestNotification,
}

/// Sends tracing output to a daily file under the data directory.
///
/// Logging must never go to stdout: the dashboard owns the terminal in raw mode
/// and any stray line corrupts the frame. A file also means a misbehaving
/// background task (a keeper ping, a quota fetch) leaves evidence behind instead
/// of vanishing with the status bar message.
///
/// Returns the appender guard, which has to stay alive for the whole process or
/// buffered lines are dropped on exit.
fn init_file_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, EnvFilter};

    // `dirs` rather than $HOME: the variable is routinely unset on Windows and
    // in slim containers, which would drop this under the working directory.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let log_dir = home
        .join(".local")
        .join("share")
        .join("tokenpulse")
        .join("log");
    std::fs::create_dir_all(&log_dir).ok()?;

    let filter = EnvFilter::try_from_env("TOKENPULSE_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let appender = tracing_appender::rolling::daily(&log_dir, "tokenpulse.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let subscriber = fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_env_filter(filter)
        .finish();

    tracing::subscriber::set_global_default(subscriber).ok()?;
    Some(guard)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _log_guard = init_file_logging();

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
                cli.refresh_days,
                cli.refresh_pricing,
                cli.rebuild_all,
                if cli.json || cli.csv.is_some() {
                    false
                } else {
                    resolve_tui_mode(cli.no_tui)?
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

fn resolve_tui_mode(no_tui: bool) -> anyhow::Result<bool> {
    let interactive_tui = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("TERM")
            .map(|term| term != "dumb")
            .unwrap_or(true);

    Ok(if no_tui { false } else { interactive_tui })
}
