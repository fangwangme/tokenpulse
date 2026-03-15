mod commands;
mod tui;

use clap::{Parser, Subcommand};

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
    Quota {
        #[clap(short, long)]
        provider: Option<String>,
        
        #[clap(long)]
        json: bool,
    },
    Usage {
        #[clap(long)]
        since: Option<String>,
        
        #[clap(short, long)]
        provider: Option<String>,
        
        #[clap(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Quota { provider, json } => {
            commands::quota::run(provider, json).await?;
        }
        Commands::Usage { since, provider, json } => {
            commands::usage::run(since, provider, json).await?;
        }
    }

    Ok(())
}
