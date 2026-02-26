use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

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
    /// Ingest all markdown files into the index
    Ingest,
    /// Watch for file changes and re-index incrementally
    Watch,
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
        Some(Commands::Ingest) => {
            let provider = mdvdb::embedding::provider::create_provider(&config)?;
            let embed_config = mdvdb::index::EmbeddingConfig {
                provider: format!("{:?}", config.embedding_provider),
                model: config.embedding_model.clone(),
                dimensions: config.embedding_dimensions,
            };
            let index = mdvdb::index::Index::open_or_create(
                &config.index_file,
                &embed_config,
            )?;

            let result = mdvdb::ingest::ingest_full(
                &cwd,
                &config,
                &index,
                provider.as_ref(),
                config.chunk_max_tokens,
                config.chunk_overlap_tokens,
                config.embedding_batch_size,
            )
            .await?;

            println!(
                "Ingestion complete: {} discovered, {} ingested, {} skipped, {} removed, {} chunks",
                result.files_discovered,
                result.files_ingested,
                result.files_skipped,
                result.files_removed,
                result.chunks_total,
            );
        }
        Some(Commands::Watch) => {
            let provider: Arc<dyn mdvdb::embedding::provider::EmbeddingProvider> =
                Arc::from(mdvdb::embedding::provider::create_provider(&config)?);
            let embed_config = mdvdb::index::EmbeddingConfig {
                provider: format!("{:?}", config.embedding_provider),
                model: config.embedding_model.clone(),
                dimensions: config.embedding_dimensions,
            };
            let index = Arc::new(mdvdb::index::Index::open_or_create(
                &config.index_file,
                &embed_config,
            )?);

            let cancel = CancellationToken::new();
            let cancel_clone = cancel.clone();

            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    tracing::info!("received Ctrl+C, shutting down watcher");
                    cancel_clone.cancel();
                }
            });

            println!("Watching for changes... (Ctrl+C to stop)");
            let watcher = mdvdb::watcher::Watcher::new(config, &cwd, index, provider);
            watcher.watch(cancel).await?;
            println!("Watcher stopped.");
        }
    }

    Ok(())
}
