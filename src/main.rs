use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// mdvdb — Markdown Vector Database
#[derive(Parser)]
#[command(name = "mdvdb", version, about)]
struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Project root directory (defaults to current directory)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Semantic search across indexed markdown files
    Search(SearchArgs),

    /// Ingest markdown files into the index
    Ingest(IngestArgs),

    /// Show index status and configuration
    Status(StatusArgs),

    /// Show inferred metadata schema
    Schema(SchemaArgs),

    /// Show document clusters
    Clusters(ClustersArgs),

    /// Get metadata for a specific file
    Get(GetArgs),

    /// Watch for file changes and re-index automatically
    Watch(WatchArgs),

    /// Initialize a new .markdownvdb config file
    Init(InitArgs),

    /// Generate shell completions
    #[command(hide = true)]
    Completions(CompletionsArgs),
}

#[derive(Parser)]
struct SearchArgs {
    /// Search query string
    query: String,

    /// Maximum number of results to return
    #[arg(short, long)]
    limit: Option<usize>,

    /// Minimum similarity score (0.0 to 1.0)
    #[arg(long)]
    min_score: Option<f32>,

    /// Metadata filter expression (e.g. "tags:rust")
    #[arg(short, long)]
    filter: Option<String>,

    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct IngestArgs {
    /// Force full re-ingestion of all files
    #[arg(long)]
    full: bool,

    /// Ingest a specific file only
    #[arg(long)]
    file: Option<PathBuf>,

    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct StatusArgs {
    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct SchemaArgs {
    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct ClustersArgs {
    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct GetArgs {
    /// Path to the markdown file
    file_path: PathBuf,

    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct WatchArgs {
    /// Output results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct InitArgs {}

#[derive(Clone, ValueEnum)]
enum ShellType {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

#[derive(Parser)]
struct CompletionsArgs {
    /// Shell to generate completions for
    shell: ShellType,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    mdvdb::logging::init(cli.verbose)?;

    let cwd = match &cli.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()?,
    };
    let config = mdvdb::config::Config::load(&cwd)?;

    match cli.command {
        Some(Commands::Search(_args)) => {
            todo!("search command implementation")
        }
        Some(Commands::Ingest(_args)) => {
            todo!("ingest command implementation")
        }
        Some(Commands::Status(_args)) => {
            tracing::info!("mdvdb v{}", env!("CARGO_PKG_VERSION"));
            tracing::info!(provider = ?config.embedding_provider, model = %config.embedding_model, "config loaded");
            println!("mdvdb - Markdown Vector Database");
            println!("Config loaded from: {}", cwd.display());
            println!(
                "  provider: {:?}, model: {}, dimensions: {}",
                config.embedding_provider, config.embedding_model, config.embedding_dimensions
            );
        }
        Some(Commands::Schema(_args)) => {
            todo!("schema command implementation")
        }
        Some(Commands::Clusters(_args)) => {
            todo!("clusters command implementation")
        }
        Some(Commands::Get(_args)) => {
            todo!("get command implementation")
        }
        Some(Commands::Watch(_args)) => {
            todo!("watch command implementation")
        }
        Some(Commands::Init(_args)) => {
            todo!("init command implementation")
        }
        Some(Commands::Completions(_args)) => {
            todo!("completions command implementation")
        }
        None => {
            tracing::info!("mdvdb v{}", env!("CARGO_PKG_VERSION"));
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
