use clap::{Parser, Subcommand};

/// mdvdb — Markdown Vector Database
#[derive(Parser)]
#[command(name = "mdvdb", version, about)]
struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show loaded configuration and status
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    mdvdb::logging::init(cli.verbose)?;

    let cwd = std::env::current_dir()?;
    let config = mdvdb::config::Config::load(&cwd)?;

    match cli.command {
        Some(Commands::Status) | None => {
            tracing::info!("mdvdb v{}", env!("CARGO_PKG_VERSION"));
            tracing::info!(provider = ?config.embedding_provider, model = %config.embedding_model, "config loaded");
            println!("mdvdb - Markdown Vector Database");
            println!("Config loaded from: {}", cwd.display());
            println!(
                "  provider: {:?}, model: {}, dimensions: {}",
                config.embedding_provider, config.embedding_model, config.embedding_dimensions
            );
        }
    }

    Ok(())
}
