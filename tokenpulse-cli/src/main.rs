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
    },
    Usage {
        #[clap(long)]
        since: Option<String>,

        #[clap(short, long)]
        provider: Option<String>,
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
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { default } => {
            commands::init::run(default)?;
        }
        Commands::Quota { provider } => {
            check_config_exists();
            commands::quota::run(provider).await?;
        }
        Commands::Usage { since, provider } => {
            check_config_exists();
            commands::usage::run(since, provider).await?;
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
