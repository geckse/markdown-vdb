use std::io::Write;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;

use mdvdb::search::{MetadataFilter, SearchQuery};
use mdvdb::MarkdownVdb;

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

    /// Metadata filter expression (KEY=VALUE)
    #[arg(short, long)]
    filter: Vec<String>,

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

/// Parse a KEY=VALUE filter string into a MetadataFilter::Equals.
fn parse_filter(s: &str) -> anyhow::Result<MetadataFilter> {
    let (key, val) = s
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid filter format '{}', expected KEY=VALUE", s))?;
    let key = key.trim().to_string();
    let val = val.trim();

    // Try to parse as number or boolean, fall back to string.
    let value: Value = if let Ok(n) = val.parse::<f64>() {
        Value::Number(serde_json::Number::from_f64(n).unwrap_or_else(|| serde_json::Number::from(0)))
    } else if val == "true" {
        Value::Bool(true)
    } else if val == "false" {
        Value::Bool(false)
    } else {
        Value::String(val.to_string())
    };

    Ok(MetadataFilter::Equals { field: key, value })
}

/// Run the main logic, returning Result for error handling. Errors are printed to stderr.
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    mdvdb::logging::init(cli.verbose)?;

    let cwd = match &cli.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()?,
    };
    let config = mdvdb::config::Config::load(&cwd)?;

    match cli.command {
        Some(Commands::Search(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;

            let mut query = SearchQuery::new(&args.query);
            if let Some(limit) = args.limit {
                query = query.with_limit(limit);
            }
            if let Some(min_score) = args.min_score {
                query = query.with_min_score(min_score as f64);
            }
            for f in &args.filter {
                query = query.with_filter(parse_filter(f)?);
            }

            let results = vdb.search(query).await?;

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &results)?;
                writeln!(std::io::stdout())?;
            } else {
                if results.is_empty() {
                    eprintln!("No results found.");
                } else {
                    for (i, r) in results.iter().enumerate() {
                        println!("{}. [score: {:.4}] {}", i + 1, r.score, r.file.path);
                        if !r.chunk.heading_hierarchy.is_empty() {
                            println!("   Section: {}", r.chunk.heading_hierarchy.join(" > "));
                        }
                        println!("   Lines {}-{}", r.chunk.start_line, r.chunk.end_line);
                        // Show first 200 chars of content.
                        let preview: String = r.chunk.content.chars().take(200).collect();
                        println!("   {}", preview.replace('\n', " "));
                        println!();
                    }
                }
            }
        }
        Some(Commands::Ingest(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;

            let options = mdvdb::IngestOptions {
                full: args.full,
                file: args.file,
            };

            let result = vdb.ingest(options).await?;

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &result)?;
                writeln!(std::io::stdout())?;
            } else {
                println!("Ingestion complete");
                println!("  Files indexed:  {}", result.files_indexed);
                println!("  Files skipped:  {}", result.files_skipped);
                println!("  Files removed:  {}", result.files_removed);
                println!("  Chunks created: {}", result.chunks_created);
                println!("  API calls:      {}", result.api_calls);
                if result.files_failed > 0 {
                    println!("  Files failed:   {}", result.files_failed);
                    for err in &result.errors {
                        eprintln!("    {}: {}", err.path, err.message);
                    }
                }
            }
        }
        Some(Commands::Status(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let status = vdb.status();

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &status)?;
                writeln!(std::io::stdout())?;
            } else {
                println!("Index Status");
                println!("  Documents:  {}", status.document_count);
                println!("  Chunks:     {}", status.chunk_count);
                println!("  Vectors:    {}", status.vector_count);
                println!("  File size:  {} bytes", status.file_size);
                println!("  Updated:    {}", status.last_updated);
                println!("  Provider:   {}", status.embedding_config.provider);
                println!("  Model:      {}", status.embedding_config.model);
                println!("  Dimensions: {}", status.embedding_config.dimensions);
            }
        }
        Some(Commands::Schema(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let schema = vdb.schema()?;

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &schema)?;
                writeln!(std::io::stdout())?;
            } else {
                if schema.fields.is_empty() {
                    println!("No schema fields found.");
                } else {
                    println!("Metadata Schema ({} fields)", schema.fields.len());
                    println!();
                    for field in &schema.fields {
                        println!("  {} ({:?})", field.name, field.field_type);
                        if let Some(desc) = &field.description {
                            println!("    Description: {}", desc);
                        }
                        println!("    Occurrences: {}", field.occurrence_count);
                        if field.required {
                            println!("    Required: yes");
                        }
                        if !field.sample_values.is_empty() {
                            let samples: Vec<_> = field.sample_values.iter().take(5).collect();
                            println!("    Samples: {}", samples.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
                        }
                        if let Some(allowed) = &field.allowed_values {
                            println!("    Allowed: {}", allowed.join(", "));
                        }
                        println!();
                    }
                }
            }
        }
        Some(Commands::Clusters(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let clusters = vdb.clusters()?;

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &clusters)?;
                writeln!(std::io::stdout())?;
            } else if clusters.is_empty() {
                println!("No clusters available. Run `mdvdb ingest` first, then clustering will be computed.");
            } else {
                println!("Document Clusters ({} clusters)", clusters.len());
                println!();
                for cluster in &clusters {
                    println!("  Cluster {} ({} chunks)", cluster.id, cluster.chunk_count);
                    if let Some(label) = &cluster.label {
                        if !label.is_empty() {
                            println!("    Label: {}", label);
                        }
                    }
                }
            }
        }
        Some(Commands::Get(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let path_str = args.file_path.to_string_lossy();
            let doc = vdb.get_document(&path_str)?;

            if args.json {
                serde_json::to_writer_pretty(std::io::stdout(), &doc)?;
                writeln!(std::io::stdout())?;
            } else {
                println!("Document: {}", doc.path);
                println!("  Content hash: {}", doc.content_hash);
                println!("  Chunks:       {}", doc.chunk_count);
                println!("  File size:    {} bytes", doc.file_size);
                println!("  Indexed at:   {}", doc.indexed_at);
            }
        }
        Some(Commands::Watch(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;

            if args.json {
                let msg = serde_json::json!({"status": "watching", "message": "File watching started"});
                serde_json::to_writer_pretty(std::io::stdout(), &msg)?;
                writeln!(std::io::stdout())?;
            } else {
                println!("Watching for file changes... (press Ctrl+C to stop)");
            }

            vdb.watch()?;
        }
        Some(Commands::Init(_args)) => {
            MarkdownVdb::init(&cwd)?;
            println!("Created .markdownvdb config file in {}", cwd.display());
            println!("Edit it to configure your embedding provider and other settings.");
        }
        Some(Commands::Completions(args)) => {
            // Shell completion generation.
            // When clap_complete is available, this will use clap_complete::generate().
            // For now, output basic completion scripts directly.
            let script = match args.shell {
                ShellType::Bash => {
                    r#"# mdvdb bash completions
_mdvdb() {
    local cur prev commands
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    commands="search ingest status schema clusters get watch init completions"

    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=($(compgen -W "$commands --help --version --verbose --root" -- "$cur"))
    fi
}
complete -F _mdvdb mdvdb"#
                }
                ShellType::Zsh => {
                    r#"#compdef mdvdb
_mdvdb() {
    local -a commands
    commands=(
        'search:Semantic search across indexed markdown files'
        'ingest:Ingest markdown files into the index'
        'status:Show index status and configuration'
        'schema:Show inferred metadata schema'
        'clusters:Show document clusters'
        'get:Get metadata for a specific file'
        'watch:Watch for file changes and re-index automatically'
        'init:Initialize a new .markdownvdb config file'
    )
    _describe 'command' commands
}
_mdvdb"#
                }
                ShellType::Fish => {
                    r#"# mdvdb fish completions
complete -c mdvdb -n '__fish_use_subcommand' -a search -d 'Semantic search across indexed markdown files'
complete -c mdvdb -n '__fish_use_subcommand' -a ingest -d 'Ingest markdown files into the index'
complete -c mdvdb -n '__fish_use_subcommand' -a status -d 'Show index status and configuration'
complete -c mdvdb -n '__fish_use_subcommand' -a schema -d 'Show inferred metadata schema'
complete -c mdvdb -n '__fish_use_subcommand' -a clusters -d 'Show document clusters'
complete -c mdvdb -n '__fish_use_subcommand' -a get -d 'Get metadata for a specific file'
complete -c mdvdb -n '__fish_use_subcommand' -a watch -d 'Watch for file changes and re-index automatically'
complete -c mdvdb -n '__fish_use_subcommand' -a init -d 'Initialize a new .markdownvdb config file'"#
                }
                ShellType::PowerShell => {
                    r#"# mdvdb PowerShell completions
Register-ArgumentCompleter -CommandName mdvdb -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @('search', 'ingest', 'status', 'schema', 'clusters', 'get', 'watch', 'init')
    $commands | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}"#
                }
            };
            write!(std::io::stdout(), "{}", script)?;
            writeln!(std::io::stdout())?;
        }
        None => {
            println!("mdvdb - Markdown Vector Database");
            println!("Run `mdvdb --help` for usage information.");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {:#}", e);
        process::exit(1);
    }
}
