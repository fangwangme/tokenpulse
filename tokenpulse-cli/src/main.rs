mod commands;
mod tui;

use clap::{Parser, Subcommand};
use tokenpulse_core::config::ConfigManager;

#[derive(Parser)]
#[clap(name = "tokenpulse")]
#[clap(about = "Token usage and quota dashboard for coding agents")]
#[clap(version)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive setup wizard
    Init {
        /// Skip interactive prompts, auto-detect and enable found providers
        #[clap(long)]
        default: bool,
    },
    Quota {
        #[clap(short, long)]
        provider: Option<String>,

        #[clap(long)]
        refresh: bool,
    },
    Usage {
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

        #[clap(long)]
        tui: bool,
    },
    Config {
        #[clap(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    Show,
    Enable { provider: String },
    Disable { provider: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var_os("RUST_LOG").is_some() || std::env::var_os("TOKENPULSE_LOG").is_some() {
        let _ = tracing_subscriber::fmt::try_init();
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { default } => {
            commands::init::run(default)?;
        }
        Commands::Quota { provider, refresh } => {
            check_config_exists();
            commands::quota::run(provider, refresh).await?;
        }
        Commands::Usage {
            since,
            provider,
            refresh_days,
            refresh_pricing,
            rebuild_all,
            tui,
        } => {
            check_config_exists();
            commands::usage::run(
                since,
                provider,
                refresh_days,
                refresh_pricing,
                rebuild_all,
                tui,
            )
            .await?;
        }
        Commands::Config { action } => {
            commands::config::run(action)?;
        }
    }

    Ok(())
}

fn check_config_exists() {
    let config_manager = ConfigManager::new();
    if !config_manager.exists() {
        eprintln!("Hint: No configuration found. Run `tokenpulse init` for guided setup.\n");
    }
}
