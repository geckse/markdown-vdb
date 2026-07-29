mod format;
mod update;

use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::str::FromStr;

use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use serde_json::Value;

use mdvdb::links::{LinkQueryResult, OrphanFile, ResolvedLink, SemanticEdge};
use mdvdb::search::{
    EdgeSearchResult, GraphContextItem, MetadataFilter, SearchMode, SearchQuery, SearchResult,
    SearchTimings, SortOrder,
};
use mdvdb::{CollectionQuery, GraphLevel, IngestTimings, MarkdownVdb};

/// Wrapped search output for JSON mode.
#[derive(serde::Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
    query: String,
    total_results: usize,
    mode: SearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<SearchTimings>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    graph_context: Vec<GraphContextItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    edge_results: Vec<EdgeSearchResult>,
}

/// Wrapped ingest output for JSON mode (verbosity-gated timings).
#[derive(serde::Serialize)]
struct IngestOutput {
    files_indexed: usize,
    files_skipped: usize,
    files_removed: usize,
    chunks_created: usize,
    api_calls: usize,
    files_failed: usize,
    errors: Vec<mdvdb::IngestError>,
    module_reports: Vec<mdvdb::modules::ModuleReport>,
    duration_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<IngestTimings>,
    cancelled: bool,
}

#[derive(serde::Serialize)]
struct FormulaValidationOutput {
    valid: bool,
    diagnostics: Vec<mdvdb::formula::FormulaDiagnostic>,
}

/// Wrapped links output for JSON mode.
#[derive(serde::Serialize)]
struct LinksOutput {
    file: String,
    links: LinkQueryResult,
}

/// Wrapped backlinks output for JSON mode.
#[derive(serde::Serialize)]
struct BacklinksOutput {
    file: String,
    backlinks: Vec<ResolvedLink>,
    total_backlinks: usize,
}

/// Wrapped orphans output for JSON mode.
#[derive(serde::Serialize)]
struct OrphansOutput {
    orphans: Vec<OrphanFile>,
    total_orphans: usize,
}

/// Wrapped edges output for JSON mode.
#[derive(serde::Serialize)]
struct EdgesOutput {
    edges: Vec<SemanticEdge>,
    total_edges: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relationship_filter: Option<String>,
}

/// mdvdb — Markdown Vector Database
#[derive(Parser)]
#[command(name = "mdvdb", about)]
struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Project root directory (defaults to current directory)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    /// Output results as JSON
    #[arg(long, global = true)]
    json: bool,

    /// Print version information with logo
    #[arg(long)]
    version: bool,

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

    /// Show vault or folder stats (files, chunks, tokens, reindex estimate)
    Info(InfoArgs),

    /// Show inferred metadata schema
    Schema(SchemaArgs),

    /// Show document clusters
    Clusters(ClustersArgs),

    /// Show file tree with sync status indicators
    Tree(TreeArgs),

    /// Get metadata for a specific file
    Get(GetArgs),

    /// List a folder's documents as a table (rows = files, columns = frontmatter)
    #[command(alias = "list")]
    Collection(CollectionArgs),

    /// Watch for file changes and re-index automatically
    Watch(WatchArgs),

    /// Inspect and run built-in derived-data modules
    Modules(ModulesArgs),

    /// Initialize a new .markdownvdb config file
    Init(InitArgs),

    /// Show resolved configuration
    Config(ConfigArgs),

    /// Run diagnostic checks on config, provider, and index
    Doctor(DoctorArgs),

    /// Show links originating from a file
    Links(LinksArgs),

    /// Show backlinks pointing to a file
    Backlinks(BacklinksArgs),

    /// Find orphan files with no links
    Orphans(OrphansArgs),

    /// Show semantic edges between linked files
    Edges(EdgesArgs),

    /// Show graph data (nodes, edges, clusters) for visualization
    Graph(GraphArgs),

    /// Dump chunks as JSON (for benchmarking — ensures identical chunking)
    #[command(hide = true)]
    Chunks(ChunksArgs),

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

    /// Enable link boosting (favor results linked to/from top matches)
    #[arg(long, conflicts_with = "no_boost_links")]
    boost_links: bool,

    /// Disable link boosting (even if enabled in config)
    #[arg(long, conflicts_with = "boost_links")]
    no_boost_links: bool,

    /// Search mode: hybrid, semantic, or lexical
    #[arg(long, value_name = "MODE")]
    mode: Option<SearchMode>,

    /// Shorthand for --mode=semantic
    #[arg(long, conflicts_with_all = ["lexical", "mode", "edge_search"])]
    semantic: bool,

    /// Shorthand for --mode=lexical
    #[arg(long, conflicts_with_all = ["semantic", "mode", "edge_search"])]
    lexical: bool,

    /// Shorthand for --mode=edge (search edge embeddings)
    #[arg(long, conflicts_with_all = ["semantic", "lexical", "mode"])]
    edge_search: bool,

    /// Restrict search to files under this path prefix
    #[arg(long)]
    path: Option<String>,

    /// Enable time decay (favor recently modified files)
    #[arg(long, conflicts_with = "no_decay")]
    decay: bool,

    /// Disable time decay (even if enabled in config)
    #[arg(long, conflicts_with = "decay")]
    no_decay: bool,

    /// Half-life in days for time decay (how many days until score is halved)
    #[arg(long, value_name = "DAYS")]
    decay_half_life: Option<f64>,

    /// Comma-separated path prefixes excluded from time decay
    #[arg(long, value_name = "PATTERNS")]
    decay_exclude: Option<String>,

    /// Comma-separated path prefixes where time decay applies (whitelist)
    #[arg(long, value_name = "PATTERNS")]
    decay_include: Option<String>,

    /// Number of link hops for graph-aware boosting (1-3, requires --boost-links)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=3), requires = "boost_links")]
    hops: Option<u8>,

    /// Graph expansion depth for context (0-3, 0 disables)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(0..=3))]
    expand: Option<u8>,

    /// Resolve frontmatter relations ([[wiki-link]] values) inline: path, existence, title, target frontmatter
    #[arg(long)]
    populate: bool,
}

#[derive(Parser)]
struct IngestArgs {
    /// Force re-embedding of all files
    #[arg(long)]
    reindex: bool,

    /// Hidden alias for --reindex (deprecated)
    #[arg(long, hide = true)]
    full: bool,

    /// Ingest a specific file only
    #[arg(long)]
    file: Option<PathBuf>,

    /// Preview what ingestion would do without actually ingesting
    #[arg(long)]
    preview: bool,
}

#[derive(Parser)]
struct StatusArgs {}

#[derive(Parser)]
struct InfoArgs {
    /// Folder path to scope stats to (relative). Defaults to the whole vault.
    #[arg(default_value = ".")]
    path: String,
}

#[derive(Parser)]
struct SchemaArgs {
    /// Restrict schema to files under this path prefix
    #[arg(long)]
    path: Option<String>,
}

#[derive(Parser)]
struct ClustersArgs {
    /// Show custom clusters instead of auto clusters
    #[arg(long)]
    custom: bool,

    /// Manage custom cluster definitions
    #[command(subcommand)]
    action: Option<ClusterAction>,
}

#[derive(Subcommand)]
enum ClusterAction {
    /// Add a topic (custom cluster) definition
    Add {
        /// Topic name
        name: String,
        /// Comma-separated seed words/phrases
        #[arg(long)]
        seeds: Option<String>,
        /// Natural-language description (improves matching accuracy)
        #[arg(long)]
        description: Option<String>,
        /// Per-topic similarity threshold in [0.0, 1.0]
        #[arg(long)]
        threshold: Option<f32>,
    },
    /// Update an existing topic (custom cluster) definition
    Update {
        /// Topic name to update
        name: String,
        /// Replace the comma-separated seed list
        #[arg(long)]
        seeds: Option<String>,
        /// Replace the description ("" clears it)
        #[arg(long)]
        description: Option<String>,
        /// Replace the similarity threshold (negative value clears it)
        #[arg(long)]
        threshold: Option<f32>,
        /// Rename the topic
        #[arg(long)]
        rename: Option<String>,
    },
    /// Remove a topic (custom cluster) definition
    Remove {
        /// Topic name to remove
        name: String,
    },
    /// List topic (custom cluster) definitions from config
    List,
    /// Show documents matching no topic (the Unassigned bucket)
    Unassigned,
}

#[derive(Parser)]
struct TreeArgs {
    /// Restrict tree to files under this path prefix
    #[arg(long)]
    path: Option<String>,
}

#[derive(Parser)]
struct GetArgs {
    /// Path to the markdown file
    file_path: PathBuf,

    /// Resolve frontmatter relations ([[wiki-link]] values) inline: path, existence, title, target frontmatter
    #[arg(long)]
    populate: bool,
}

#[derive(Parser)]
struct CollectionArgs {
    /// Folder path prefix (relative). Defaults to the whole vault.
    #[arg(default_value = ".")]
    path: String,

    /// Include files in all nested subfolders (default: direct children only)
    #[arg(short, long)]
    recursive: bool,

    /// Frontmatter field to sort rows by (default: sort by path)
    #[arg(long, value_name = "FIELD")]
    sort: Option<String>,

    /// Sort direction
    #[arg(long, value_name = "ORDER", default_value = "asc")]
    order: SortOrder,

    /// Metadata filter expression (KEY=VALUE), repeatable (AND logic)
    #[arg(short, long)]
    filter: Vec<String>,

    /// Maximum number of rows to return
    #[arg(long)]
    limit: Option<usize>,

    /// Number of rows to skip (for pagination)
    #[arg(long, default_value = "0")]
    offset: usize,

    /// Resolve frontmatter relations ([[wiki-link]] values) inline: path, existence, title, target frontmatter
    #[arg(long)]
    populate: bool,
}

#[derive(Parser)]
struct WatchArgs {}

#[derive(Parser)]
struct ModulesArgs {
    #[command(subcommand)]
    action: ModuleAction,
}

#[derive(Subcommand)]
enum ModuleAction {
    /// List compiled-in modules and their hooks
    List,
    /// Validate module input without changing the index
    Validate {
        /// Module id (`formula`)
        module: String,
        /// Formula expression to validate
        #[arg(long)]
        formula: String,
        /// Declared formula result type
        #[arg(long)]
        result_type: String,
    },
    /// Recompute a module's persisted derived data
    Run {
        /// Module id (`formula`)
        module: String,
        /// Optional folder scope
        #[arg(long)]
        path: Option<String>,
    },
    /// Show persisted diagnostics for a module
    Status {
        /// Module id (`formula`)
        module: String,
        /// Optional folder scope
        #[arg(long)]
        path: Option<String>,
    },
}

#[derive(Parser)]
struct LinksArgs {
    /// Path to the markdown file
    file_path: PathBuf,

    /// Link traversal depth (1 = direct links, 2-3 = multi-hop)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=3), default_value = "1")]
    depth: u8,
}

#[derive(Parser)]
struct BacklinksArgs {
    /// Path to the markdown file
    file_path: PathBuf,
}

#[derive(Parser)]
struct OrphansArgs {}

#[derive(Parser)]
struct EdgesArgs {
    /// Filter edges by file (source or target)
    file: Option<PathBuf>,

    /// Filter by relationship type (substring match on cluster label)
    #[arg(long)]
    relationship: Option<String>,
}

#[derive(Parser)]
struct GraphArgs {
    /// Graph granularity level
    #[arg(long, value_enum, default_value = "document")]
    level: GraphLevelArg,

    /// Restrict graph to files under this path prefix
    #[arg(long)]
    path: Option<String>,

    /// Emit the versioned app wire format with response-level interned contexts
    #[arg(long, visible_alias = "intern-contexts", requires = "json")]
    compact: bool,
}

#[derive(Clone, ValueEnum)]
enum GraphLevelArg {
    Document,
    Chunk,
}

#[derive(Parser)]
struct InitArgs {
    /// Create user-level config at ~/.mdvdb/config instead of project config
    #[arg(long)]
    global: bool,
}

#[derive(Parser)]
struct ConfigArgs {
    /// Modify configuration
    #[command(subcommand)]
    action: Option<ConfigAction>,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set a config value in .markdownvdb/config.yaml using a dotted key path
    /// (e.g. `mdvdb config set clustering.algorithm kmeans`)
    Set {
        /// Dotted key path (e.g. clustering.topics.min_similarity)
        key: String,
        /// New value (parsed as bool/number when possible, else string)
        value: String,
    },
}

#[derive(Parser)]
struct DoctorArgs {}

#[derive(Parser)]
struct ChunksArgs {
    /// Directory containing markdown files to chunk
    dir: PathBuf,

    /// Maximum tokens per chunk
    #[arg(long, default_value = "512")]
    max_tokens: usize,

    /// Overlap tokens for sub-split chunks
    #[arg(long, default_value = "50")]
    overlap_tokens: usize,
}

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
    let value: Value = if val == "true" {
        Value::Bool(true)
    } else if val == "false" {
        Value::Bool(false)
    } else if let Ok(number) = serde_json::Number::from_str(val) {
        Value::Number(number)
    } else {
        Value::String(val.to_string())
    };

    Ok(MetadataFilter::Equals { field: key, value })
}

/// Run the main logic, returning Result for error handling. Errors are printed to stderr.
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Spawn background update check (non-blocking, never causes failures)
    let update_handle = update::spawn_check();

    // Disable colors if --no-color flag, NO_COLOR env var, or JSON mode is active.
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        colored::control::set_override(false);
    }

    if cli.version {
        if cli.json {
            // Machine-readable version (the Tesseract app gates relation
            // features on this — see app repo `lib/cli-features.svelte.ts`).
            println!(
                "{}",
                serde_json::json!({ "version": env!("CARGO_PKG_VERSION") })
            );
        } else {
            format::print_version();
            // Show update notice on --version if available
            if let Ok(Some(msg)) = update_handle.await {
                eprintln!("{msg}");
            }
        }
        return Ok(());
    }

    // In JSON mode, suppress tracing logs to avoid any possibility of
    // log output leaking into stdout and breaking JSON parsing.
    // In JSON mode, suppress tracing logs to keep stdout clean for JSON parsing.
    // Exception: if verbose is set, allow logs to stderr even in JSON mode.
    if cli.json && cli.verbose == 0 {
        mdvdb::logging::init_silent()?;
    } else {
        mdvdb::logging::init(cli.verbose)?;
    }

    let cwd = match &cli.root {
        Some(root) => root.clone(),
        None => std::env::current_dir()?,
    };
    let config = mdvdb::config::Config::load(&cwd)?;
    let json = cli.json;
    let no_color = cli.no_color || std::env::var_os("NO_COLOR").is_some();

    match cli.command {
        Some(Commands::Search(args)) => {
            // Determine search mode: explicit --mode takes priority, then shorthand flags, then config default.
            let mode = if let Some(m) = args.mode {
                m
            } else if args.semantic {
                SearchMode::Semantic
            } else if args.lexical {
                SearchMode::Lexical
            } else if args.edge_search {
                SearchMode::Edge
            } else {
                config.search_default_mode
            };

            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;

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
            if args.boost_links {
                query = query.with_boost_links(true);
            } else if args.no_boost_links {
                query = query.with_boost_links(false);
            }
            query = query.with_mode(mode);
            if let Some(ref path) = args.path {
                query = query.with_path_prefix(path);
            }
            if args.decay {
                query = query.with_decay(true);
            } else if args.no_decay {
                query = query.with_decay(false);
            }
            if let Some(half_life) = args.decay_half_life {
                query = query.with_decay_half_life(half_life);
            }
            if let Some(ref patterns) = args.decay_exclude {
                let list: Vec<String> = patterns.split(',').map(|s| s.trim().to_string()).collect();
                query = query.with_decay_exclude(list);
            }
            if let Some(ref patterns) = args.decay_include {
                let list: Vec<String> = patterns.split(',').map(|s| s.trim().to_string()).collect();
                query = query.with_decay_include(list);
            }
            if let Some(hops) = args.hops {
                query = query.with_boost_hops(hops as usize);
            }
            if let Some(expand) = args.expand {
                query = query.with_expand_graph(expand as usize);
            }
            if args.populate {
                query = query.with_populate(true);
            }

            let effective_mode = query.mode;
            let response = vdb.search(query).await?;

            if json {
                let output = SearchOutput {
                    total_results: response.results.len(),
                    query: args.query.clone(),
                    results: response.results,
                    mode: effective_mode,
                    timings: if cli.verbose > 0 {
                        Some(response.timings)
                    } else {
                        None
                    },
                    graph_context: response.graph_context,
                    edge_results: response.edge_results,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_search_results(&response.results, &args.query);
                if !response.graph_context.is_empty() {
                    format::print_graph_context(&response.graph_context);
                }
                if cli.verbose > 0 {
                    eprintln!(
                        "  [timing] embed={:.0}ms hnsw={:.0}ms bm25={:.0}ms fusion={:.0}ms assemble={:.0}ms total={:.0}ms",
                        response.timings.embed_secs * 1000.0,
                        response.timings.vector_search_secs * 1000.0,
                        response.timings.lexical_search_secs * 1000.0,
                        response.timings.fusion_secs * 1000.0,
                        response.timings.assemble_secs * 1000.0,
                        response.timings.total_secs * 1000.0,
                    );
                }
            }
        }
        Some(Commands::Ingest(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;

            if args.preview {
                let preview = vdb.preview(args.reindex || args.full, args.file)?;
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &preview)?;
                    writeln!(std::io::stdout())?;
                } else {
                    format::print_ingest_preview(&preview);
                }
                return Ok(());
            }

            let interactive = !json && std::io::IsTerminal::is_terminal(&std::io::stdout());

            // Set up Ctrl+C cancellation (same pattern as watch command).
            let cancel = tokio_util::sync::CancellationToken::new();
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                cancel_clone.cancel();
            });

            // Set up progress bars if interactive.
            let progress_callback: Option<mdvdb::ProgressCallback> = if interactive {
                let mp = indicatif::MultiProgress::new();
                let main_bar = mp.add(indicatif::ProgressBar::new(0));
                main_bar.set_style(
                    indicatif::ProgressStyle::with_template(
                        "  {spinner:.green} [{pos}/{len}] {msg} {wide_bar:.cyan/dim} {percent}%",
                    )
                    .unwrap()
                    .progress_chars("█░░"),
                );
                main_bar.enable_steady_tick(std::time::Duration::from_millis(120));

                let status_bar = mp.add(indicatif::ProgressBar::new_spinner());
                status_bar.set_style(
                    indicatif::ProgressStyle::with_template("  {spinner:.dim} {msg}").unwrap(),
                );
                status_bar.enable_steady_tick(std::time::Duration::from_millis(120));

                let start = std::time::Instant::now();

                Some(Box::new(move |phase: &mdvdb::IngestPhase| {
                    let elapsed = start.elapsed().as_secs();
                    let elapsed_str = format!("{}:{:02}", elapsed / 60, elapsed % 60);
                    match phase {
                        mdvdb::IngestPhase::Discovering => {
                            main_bar.set_message("Discovering files...");
                            status_bar.set_message(format!("[{elapsed_str}] discovering"));
                        }
                        mdvdb::IngestPhase::Parsing {
                            current,
                            total,
                            path,
                        } => {
                            main_bar.set_length(*total as u64);
                            main_bar.set_position(*current as u64);
                            main_bar.set_message(path.to_string());
                            status_bar
                                .set_message(format!("[{elapsed_str}] parsing {current}/{total}"));
                        }
                        mdvdb::IngestPhase::Skipped {
                            current,
                            total,
                            path,
                        } => {
                            main_bar.set_length(*total as u64);
                            main_bar.set_position(*current as u64);
                            main_bar.set_message(format!("{path} (skipped)"));
                            status_bar
                                .set_message(format!("[{elapsed_str}] skipped {current}/{total}"));
                        }
                        mdvdb::IngestPhase::Embedding {
                            batch,
                            total_batches,
                        } => {
                            main_bar
                                .set_message(format!("Embedding batch {batch}/{total_batches}"));
                            status_bar.set_message(format!("[{elapsed_str}] embedding"));
                        }
                        mdvdb::IngestPhase::Saving => {
                            main_bar.set_message("Saving index...");
                            status_bar.set_message(format!("[{elapsed_str}] saving"));
                        }
                        mdvdb::IngestPhase::Clustering => {
                            main_bar.set_message("Clustering...");
                            status_bar.set_message(format!("[{elapsed_str}] clustering"));
                        }
                        mdvdb::IngestPhase::Cleaning => {
                            main_bar.set_message("Cleaning removed files...");
                            status_bar.set_message(format!("[{elapsed_str}] cleaning"));
                        }
                        mdvdb::IngestPhase::Done => {
                            main_bar.finish_and_clear();
                            status_bar.finish_and_clear();
                        }
                    }
                }))
            } else {
                None
            };

            let options = mdvdb::IngestOptions {
                full: args.reindex || args.full,
                file: args.file,
                progress: progress_callback,
                cancel: Some(cancel),
            };

            let result = vdb.ingest(options).await?;

            if json {
                let output = IngestOutput {
                    files_indexed: result.files_indexed,
                    files_skipped: result.files_skipped,
                    files_removed: result.files_removed,
                    chunks_created: result.chunks_created,
                    api_calls: result.api_calls,
                    files_failed: result.files_failed,
                    errors: result.errors.clone(),
                    module_reports: result.module_reports.clone(),
                    duration_secs: result.duration_secs,
                    timings: if cli.verbose > 0 {
                        result.timings.clone()
                    } else {
                        None
                    },
                    cancelled: result.cancelled,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_ingest_result(&result);
                if cli.verbose > 0 {
                    if let Some(ref t) = result.timings {
                        eprintln!(
                            "  [timing] discover={:.0}ms parse={:.0}ms embed={:.0}ms upsert={:.0}ms save={:.0}ms total={:.0}ms",
                            t.discover_secs * 1000.0,
                            t.parse_secs * 1000.0,
                            t.embed_secs * 1000.0,
                            t.upsert_secs * 1000.0,
                            t.save_secs * 1000.0,
                            t.total_secs * 1000.0,
                        );
                    }
                }
            }
        }
        Some(Commands::Status(_args)) => {
            let empty_embedding = mdvdb::index::types::EmbeddingConfig {
                provider: format!("{:?}", config.embedding_provider),
                model: config.embedding_model.clone(),
                dimensions: config.embedding_dimensions,
            };
            let status = match MarkdownVdb::open_readonly_with_config(cwd, config) {
                Ok(vdb) => vdb.status(),
                Err(mdvdb::Error::IndexNotFound { .. }) => mdvdb::IndexStatus {
                    document_count: 0,
                    chunk_count: 0,
                    vector_count: 0,
                    edge_count: 0,
                    last_updated: 0,
                    file_size: 0,
                    embedding_config: empty_embedding,
                },
                Err(error) => return Err(error.into()),
            };

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &status)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_status(&status);
            }
        }
        Some(Commands::Info(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let info = vdb.info(Some(&args.path))?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &info)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_info(&info);
            }
        }
        Some(Commands::Schema(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;

            if let Some(ref prefix) = args.path {
                let scoped = vdb.schema_scoped(prefix)?;

                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &scoped)?;
                    writeln!(std::io::stdout())?;
                } else {
                    let scope_label = format!("Schema (scoped to {})", prefix);
                    eprintln!("{}", scope_label.bold());
                    let vdb_status = vdb.status();
                    format::print_schema(&scoped.schema, vdb_status.document_count, Some(prefix));
                }
            } else {
                let schema = vdb.schema()?;

                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &schema)?;
                    writeln!(std::io::stdout())?;
                } else {
                    let vdb_status = vdb.status();
                    format::print_schema(&schema, vdb_status.document_count, None);
                }
            }
        }
        Some(Commands::Clusters(args)) => {
            match args.action {
                Some(ClusterAction::Add {
                    name,
                    seeds,
                    description,
                    threshold,
                }) => {
                    let seed_list = parse_seed_list(seeds.as_deref())?;
                    let description = normalize_description(description);
                    validate_topic_fields(&name, &seed_list, description.as_deref(), threshold)?;

                    // Read existing defs, add new one, write back.
                    let yaml_config_path = cwd.join(".markdownvdb").join("config.yaml");
                    let mut defs = read_custom_clusters_from_yaml(&yaml_config_path);

                    // Check for duplicate name.
                    if defs.iter().any(|d| d.name == name) {
                        anyhow::bail!("topic '{}' already exists", name);
                    }

                    defs.push(mdvdb::CustomClusterDef {
                        name: name.clone(),
                        description,
                        seeds: seed_list,
                        threshold,
                    });

                    write_custom_clusters_to_yaml(&yaml_config_path, &defs)?;

                    if !json {
                        eprintln!(
                            "Added topic '{name}'. Run `mdvdb ingest` to compute assignments."
                        );
                    }
                }
                Some(ClusterAction::Update {
                    name,
                    seeds,
                    description,
                    threshold,
                    rename,
                }) => {
                    let yaml_config_path = cwd.join(".markdownvdb").join("config.yaml");
                    let mut defs = read_custom_clusters_from_yaml(&yaml_config_path);

                    let Some(def) = defs.iter_mut().find(|d| d.name == name) else {
                        anyhow::bail!("topic '{}' not found", name);
                    };

                    if let Some(seeds) = seeds.as_deref() {
                        def.seeds = parse_seed_list(Some(seeds))?;
                    }
                    if let Some(desc) = description {
                        def.description = normalize_description(Some(desc));
                    }
                    if let Some(t) = threshold {
                        def.threshold = if t < 0.0 { None } else { Some(t) };
                    }
                    if let Some(new_name) = rename {
                        def.name = new_name;
                    }
                    let def_snapshot = def.clone();
                    validate_topic_fields(
                        &def_snapshot.name,
                        &def_snapshot.seeds,
                        def_snapshot.description.as_deref(),
                        def_snapshot.threshold,
                    )?;
                    let duplicates = defs.iter().filter(|d| d.name == def_snapshot.name).count();
                    if duplicates > 1 {
                        anyhow::bail!("topic '{}' already exists", def_snapshot.name);
                    }

                    write_custom_clusters_to_yaml(&yaml_config_path, &defs)?;

                    if !json {
                        eprintln!(
                            "Updated topic '{}'. Run `mdvdb ingest` to recompute assignments.",
                            def_snapshot.name
                        );
                    }
                }
                Some(ClusterAction::Remove { name }) => {
                    let yaml_config_path = cwd.join(".markdownvdb").join("config.yaml");
                    let mut defs = read_custom_clusters_from_yaml(&yaml_config_path);
                    let before_len = defs.len();
                    defs.retain(|d| d.name != name);

                    if defs.len() == before_len {
                        anyhow::bail!("topic '{}' not found", name);
                    }

                    write_custom_clusters_to_yaml(&yaml_config_path, &defs)?;

                    if !json {
                        eprintln!(
                            "Removed topic '{name}'. Run `mdvdb ingest` to update assignments."
                        );
                    }
                }
                Some(ClusterAction::List) => {
                    let yaml_config_path = cwd.join(".markdownvdb").join("config.yaml");
                    let defs = read_custom_clusters_from_yaml(&yaml_config_path);

                    if json {
                        serde_json::to_writer_pretty(std::io::stdout(), &defs)?;
                        writeln!(std::io::stdout())?;
                    } else if defs.is_empty() {
                        println!("No topic definitions.");
                    } else {
                        println!("Topic definitions:");
                        for (i, def) in defs.iter().enumerate() {
                            println!("  {}. {}", i + 1, def.name);
                            if let Some(desc) = &def.description {
                                println!("     Description: {desc}");
                            }
                            if !def.seeds.is_empty() {
                                println!("     Seeds: {}", def.seeds.join(", "));
                            }
                            if let Some(t) = def.threshold {
                                println!("     Threshold: {t:.2}");
                            }
                        }
                    }
                }
                Some(ClusterAction::Unassigned) => {
                    let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
                    let paths = vdb.topic_unassigned()?;

                    if json {
                        #[derive(serde::Serialize)]
                        struct UnassignedOutput {
                            count: usize,
                            paths: Vec<String>,
                        }
                        let out = UnassignedOutput {
                            count: paths.len(),
                            paths,
                        };
                        serde_json::to_writer_pretty(std::io::stdout(), &out)?;
                        writeln!(std::io::stdout())?;
                    } else if paths.is_empty() {
                        println!("No unassigned documents.");
                    } else {
                        println!("Unassigned documents ({}):", paths.len());
                        for p in &paths {
                            println!("  {p}");
                        }
                    }
                }
                None => {
                    if args.custom {
                        let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
                        let custom = vdb.custom_clusters()?;
                        let unassigned_count = vdb.topic_unassigned()?.len();

                        if json {
                            serde_json::to_writer_pretty(std::io::stdout(), &custom)?;
                            writeln!(std::io::stdout())?;
                        } else if custom.is_empty() {
                            println!("No topics. Use `mdvdb clusters add` or define them in .markdownvdb/config.yaml and run ingest.");
                        } else {
                            let total_docs: usize = custom.iter().map(|c| c.document_count).sum();
                            println!(
                                "Topics ({} topics, {} memberships):",
                                custom.len(),
                                total_docs
                            );
                            for c in &custom {
                                let mean = c
                                    .mean_score
                                    .map(|s| format!(", avg {:.0}%", s * 100.0))
                                    .unwrap_or_default();
                                let threshold = c
                                    .threshold
                                    .map(|t| format!(", threshold {t:.2}"))
                                    .unwrap_or_default();
                                println!(
                                    "  {}. {} ({} docs{mean}{threshold})",
                                    c.id + 1,
                                    c.name,
                                    c.document_count
                                );
                            }
                            println!("  Unassigned: {unassigned_count} documents");
                        }
                    } else {
                        let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
                        let clusters = vdb.clusters()?;

                        if json {
                            serde_json::to_writer_pretty(std::io::stdout(), &clusters)?;
                            writeln!(std::io::stdout())?;
                        } else {
                            format::print_clusters(&clusters);
                        }
                    }
                }
            }
        }
        Some(Commands::Tree(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let tree = vdb.file_tree()?;

            if json {
                if let Some(ref prefix) = args.path {
                    if let Some(subtree) = mdvdb::tree::filter_subtree(&tree.root, prefix) {
                        let filtered = mdvdb::tree::FileTree {
                            root: subtree,
                            ..tree
                        };
                        serde_json::to_writer_pretty(std::io::stdout(), &filtered)?;
                    } else {
                        let empty = mdvdb::tree::FileTree {
                            root: mdvdb::tree::FileTreeNode {
                                name: ".".to_string(),
                                path: ".".to_string(),
                                is_dir: true,
                                state: None,
                                children: Vec::new(),
                            },
                            total_files: 0,
                            indexed_count: 0,
                            modified_count: 0,
                            new_count: 0,
                            deleted_count: 0,
                        };
                        serde_json::to_writer_pretty(std::io::stdout(), &empty)?;
                    }
                } else {
                    serde_json::to_writer_pretty(std::io::stdout(), &tree)?;
                }
                writeln!(std::io::stdout())?;
            } else if let Some(ref prefix) = args.path {
                if let Some(subtree) = mdvdb::tree::filter_subtree(&tree.root, prefix) {
                    let filtered = mdvdb::tree::FileTree {
                        root: subtree,
                        ..tree
                    };
                    format::print_file_tree(&filtered, !no_color);
                } else {
                    let empty = mdvdb::tree::FileTree {
                        root: mdvdb::tree::FileTreeNode {
                            name: ".".to_string(),
                            path: ".".to_string(),
                            is_dir: true,
                            state: None,
                            children: Vec::new(),
                        },
                        total_files: 0,
                        indexed_count: 0,
                        modified_count: 0,
                        new_count: 0,
                        deleted_count: 0,
                    };
                    format::print_file_tree(&empty, !no_color);
                }
            } else {
                format::print_file_tree(&tree, !no_color);
            }
        }
        Some(Commands::Get(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let path_str = mdvdb::path_util::to_slash(&args.file_path);
            let doc = if args.populate {
                vdb.get_document_populated(&path_str)?
            } else {
                vdb.get_document(&path_str)?
            };

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &doc)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_document(&doc);
            }
        }
        Some(Commands::Collection(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;

            let mut filters = Vec::new();
            for f in &args.filter {
                filters.push(parse_filter(f)?);
            }

            let opts = CollectionQuery {
                path: mdvdb::path_util::normalize_path_input(&args.path),
                recursive: args.recursive,
                sort_by: args.sort,
                order: args.order,
                filters,
                limit: args.limit,
                offset: args.offset,
                populate: args.populate,
            };
            let resp = vdb.collection(opts)?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &resp)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_collection(&resp);
            }
        }
        Some(Commands::Links(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let path_str = mdvdb::path_util::to_slash(&args.file_path);
            let depth = args.depth as usize;

            if depth > 1 {
                let result = vdb.links_neighborhood(&path_str, depth)?;
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &result)?;
                    writeln!(std::io::stdout())?;
                } else {
                    format::print_link_neighborhood(&result);
                }
            } else {
                let result = vdb.links(&path_str)?;
                if json {
                    let output = LinksOutput {
                        file: path_str,
                        links: result,
                    };
                    serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                    writeln!(std::io::stdout())?;
                } else {
                    format::print_links(&result);
                }
            }
        }
        Some(Commands::Backlinks(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let path_str = mdvdb::path_util::to_slash(&args.file_path);
            let result = vdb.backlinks(&path_str)?;

            if json {
                let output = BacklinksOutput {
                    total_backlinks: result.len(),
                    file: path_str,
                    backlinks: result,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_backlinks(&path_str, &result);
            }
        }
        Some(Commands::Orphans(_args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let result = vdb.orphans()?;

            if json {
                let output = OrphansOutput {
                    total_orphans: result.len(),
                    orphans: result,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_orphans(&result);
            }
        }
        Some(Commands::Edges(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let file_str = args.file.as_ref().map(|p| mdvdb::path_util::to_slash(p));
            let mut edges = vdb.edges(file_str.as_deref())?;

            // Filter by relationship type substring if provided.
            if let Some(ref rel_filter) = args.relationship {
                let lower = rel_filter.to_lowercase();
                edges.retain(|e| {
                    e.relationship_type
                        .as_ref()
                        .is_some_and(|r| r.to_lowercase().contains(&lower))
                });
            }

            if json {
                let output = EdgesOutput {
                    total_edges: edges.len(),
                    edges,
                    file: file_str,
                    relationship_filter: args.relationship,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_edges(&edges);
            }
        }
        Some(Commands::Graph(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let level = match args.level {
                GraphLevelArg::Document => GraphLevel::Document,
                GraphLevelArg::Chunk => GraphLevel::Chunk,
            };

            if json {
                if args.compact {
                    // The compact contract is also serialized without JSON
                    // indentation to minimize bytes copied across the app's
                    // process boundary.
                    let data = vdb.graph_compact(level, args.path.as_deref())?;
                    serde_json::to_writer(std::io::stdout(), &data)?;
                } else {
                    // Preserve the existing public CLI JSON byte shape and
                    // pretty-printing unless compact output is explicitly set.
                    let data = vdb.graph(level, args.path.as_deref())?;
                    serde_json::to_writer_pretty(std::io::stdout(), &data)?;
                }
                writeln!(std::io::stdout())?;
            } else {
                let data = vdb.graph(level, args.path.as_deref())?;
                format::print_graph_summary(&data);
            }
        }
        Some(Commands::Watch(_args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;

            let cancel = tokio_util::sync::CancellationToken::new();
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                cancel_clone.cancel();
            });

            if json {
                let msg =
                    serde_json::json!({"status": "watching", "message": "File watching started"});
                let line = serde_json::to_string(&msg)?;
                println!("{line}");
            } else {
                let dirs: Vec<String> = vdb
                    .config()
                    .source_dirs
                    .iter()
                    .map(|d| d.to_string_lossy().to_string())
                    .collect();
                format::print_watch_started(&dirs);
            }

            let use_json = json;
            let callback: mdvdb::WatchEventCallback = Box::new(move |report| {
                if use_json {
                    if let Ok(line) = serde_json::to_string(report) {
                        println!("{line}");
                    }
                } else {
                    format::print_watch_event(report);
                }
            });

            vdb.watch(cancel, Some(callback)).await?;
        }
        Some(Commands::Modules(args)) => match args.action {
            ModuleAction::List => {
                let descriptors = mdvdb::modules::ModuleRunner::builtins().descriptors();
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &descriptors)?;
                    writeln!(std::io::stdout())?;
                } else {
                    for descriptor in descriptors {
                        let mode = if descriptor.always_on {
                            "always on"
                        } else {
                            "manual"
                        };
                        println!(
                            "{}\t{}\tv{}\t{}",
                            descriptor.id, descriptor.name, descriptor.version, mode
                        );
                    }
                }
            }
            ModuleAction::Validate {
                module,
                formula,
                result_type,
            } => {
                if module != mdvdb::formula::FORMULA_MODULE_ID {
                    anyhow::bail!("unknown module `{module}`");
                }
                let result_type = result_type
                    .parse::<mdvdb::FormulaResultType>()
                    .map_err(anyhow::Error::msg)?;
                let diagnostics = mdvdb::formula::FormulaEngine::default()
                    .validate(&formula, result_type)
                    .err()
                    .into_iter()
                    .collect::<Vec<_>>();
                let output = FormulaValidationOutput {
                    valid: diagnostics.is_empty(),
                    diagnostics,
                };
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                    writeln!(std::io::stdout())?;
                } else if output.valid {
                    println!("Formula is valid.");
                } else {
                    for diagnostic in output.diagnostics {
                        eprintln!("{}: {}", diagnostic.code, diagnostic.message);
                    }
                }
            }
            ModuleAction::Run { module, path } => {
                let vdb = MarkdownVdb::open_with_config(cwd, config)?;
                let report = vdb.run_module(&module, path.as_deref())?;
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &report)?;
                    writeln!(std::io::stdout())?;
                } else {
                    println!(
                        "{}: evaluated {} files, updated {} fields, {} diagnostics",
                        report.module,
                        report.files_evaluated,
                        report.fields_updated,
                        report.diagnostics.len()
                    );
                }
            }
            ModuleAction::Status { module, path } => {
                let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
                let diagnostics = vdb.module_status(&module, path.as_deref())?;
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &diagnostics)?;
                    writeln!(std::io::stdout())?;
                } else if diagnostics.is_empty() {
                    println!("{module}: no cached diagnostics");
                } else {
                    for diagnostic in diagnostics {
                        let path = diagnostic.path.as_deref().unwrap_or("-");
                        println!(
                            "{}\t{}\t{}\t{}",
                            path, diagnostic.field, diagnostic.code, diagnostic.message
                        );
                    }
                }
            }
        },
        Some(Commands::Init(args)) => {
            if args.global {
                let config_path = mdvdb::config::Config::user_config_path()
                    .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))?;
                MarkdownVdb::init_global(&config_path)?;
                format::print_init_global_success(&config_path.display().to_string());
            } else {
                MarkdownVdb::init(&cwd)?;
                format::print_init_success(&cwd.display().to_string());
            }
        }
        Some(Commands::Config(args)) => match args.action {
            Some(ConfigAction::Set { key, value }) => {
                let yaml_config_path = cwd.join(".markdownvdb").join("config.yaml");
                mdvdb::config_update_yaml_value(
                    &yaml_config_path,
                    &key,
                    parse_yaml_scalar(&value),
                )?;
                // Validate the resulting config; roll back is not needed since
                // the user can simply set the value again, but surface errors.
                if let Err(e) = mdvdb::config::Config::load(&cwd) {
                    eprintln!("warning: config now fails validation: {e}");
                }
                if !json {
                    eprintln!("Set {key} = {value}");
                }
            }
            None => {
                if json {
                    serde_json::to_writer_pretty(std::io::stdout(), &config)?;
                    writeln!(std::io::stdout())?;
                } else {
                    let user_config = mdvdb::config::Config::user_config_path();
                    format::print_config(&config, user_config.as_deref());
                }
            }
        },
        Some(Commands::Doctor(_args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let result = vdb.doctor().await?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &result)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_doctor(&result);
            }
        }
        Some(Commands::Chunks(args)) => {
            use mdvdb::chunker::chunk_document;
            use mdvdb::parser::parse_markdown_file;

            let dir = args.dir.canonicalize()?;
            let mut md_files: Vec<_> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .map(|e| e.file_name().into())
                .collect::<Vec<std::path::PathBuf>>();
            md_files.sort();

            let mut all_chunks: Vec<serde_json::Value> = Vec::new();

            for file_name in &md_files {
                let parsed = parse_markdown_file(&dir, file_name)?;
                let chunks = chunk_document(&parsed, args.max_tokens, args.overlap_tokens)?;
                for chunk in &chunks {
                    let content_hash = {
                        use sha2::{Digest, Sha256};
                        let mut hasher = Sha256::new();
                        hasher.update(chunk.content.as_bytes());
                        format!("{:x}", hasher.finalize())
                    };
                    all_chunks.push(serde_json::json!({
                        "content": chunk.content,
                        "heading_hierarchy": chunk.heading_hierarchy,
                        "chunk_index": chunk.chunk_index,
                        "is_sub_split": chunk.is_sub_split,
                        "file_path": mdvdb::path_util::to_slash(file_name),
                        "content_hash": content_hash,
                        "start_char": 0,
                        "end_char": 0,
                    }));
                }
            }

            serde_json::to_writer(std::io::stdout(), &all_chunks)?;
            writeln!(std::io::stdout())?;
        }
        Some(Commands::Completions(args)) => {
            // Shell completion generation.
            // TODO: Replace with clap_complete::generate() when clap_complete crate is available offline.
            let script = match args.shell {
                ShellType::Bash => {
                    r#"# mdvdb bash completions
_mdvdb() {
    local cur prev commands
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    commands="search ingest status info schema clusters tree get collection watch modules init config doctor links backlinks orphans completions"

    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=($(compgen -W "$commands --help --version --verbose --root --json --no-color" -- "$cur"))
    fi

    case "$prev" in
        ingest)
            COMPREPLY=($(compgen -W "--reindex --preview --file --full --help" -- "$cur"))
            ;;
        search)
            COMPREPLY=($(compgen -W "--limit --min-score --filter --boost-links --no-boost-links --mode --semantic --lexical --path --decay --no-decay --decay-half-life --decay-exclude --decay-include --hops --expand --help" -- "$cur"))
            ;;
        tree)
            COMPREPLY=($(compgen -W "--path --help" -- "$cur"))
            ;;
        get)
            COMPREPLY=($(compgen -f -- "$cur"))
            ;;
        info)
            COMPREPLY=($(compgen -d -- "$cur"))
            ;;
        collection|list)
            COMPREPLY=($(compgen -W "--recursive --sort --order --filter --limit --offset --help" -- "$cur"))
            ;;
        init)
            COMPREPLY=($(compgen -W "--global --help" -- "$cur"))
            ;;
        completions)
            COMPREPLY=($(compgen -W "bash zsh fish power-shell" -- "$cur"))
            ;;
    esac
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
        'info:Show vault or folder stats'
        'schema:Show inferred metadata schema'
        'clusters:Show document clusters'
        'tree:Show file tree with sync status indicators'
        'get:Get metadata for a specific file'
        'collection:List a folder'\''s documents as a table'
        'watch:Watch for file changes and re-index automatically'
        'modules:Inspect and run built-in derived-data modules'
        'init:Initialize a new .markdownvdb config file'
        'config:Show resolved configuration'
        'doctor:Run diagnostic checks'
        'links:Show links originating from a file'
        'backlinks:Show backlinks pointing to a file'
        'orphans:Find orphan files with no links'
    )

    _arguments \
        '(-v --verbose)'{-v,--verbose}'[Increase log verbosity]' \
        '--root[Project root directory]:directory:_directories' \
        '--no-color[Disable colored output]' \
        '--json[Output results as JSON]' \
        '--version[Print version information]' \
        '1:command:->commands' \
        '*::arg:->args'

    case "$state" in
        commands)
            _describe 'command' commands
            ;;
        args)
            case "$words[1]" in
                ingest)
                    _arguments \
                        '--reindex[Force re-embedding of all files]' \
                        '--preview[Preview what ingestion would do]' \
                        '--file[Ingest a specific file only]:file:_files' \
                        '--full[Alias for --reindex (deprecated)]'
                    ;;
                search)
                    _arguments \
                        '1:query:' \
                        '(-l --limit)'{-l,--limit}'[Maximum results]:number:' \
                        '--min-score[Minimum similarity score]:score:' \
                        '(-f --filter)'{-f,--filter}'[Metadata filter (KEY=VALUE)]:filter:' \
                        '--boost-links[Boost linked results]' \
                        '--no-boost-links[Disable link boosting]' \
                        '--mode[Search mode]:mode:(hybrid semantic lexical)' \
                        '--semantic[Shorthand for --mode=semantic]' \
                        '--lexical[Shorthand for --mode=lexical]' \
                        '--path[Restrict to path prefix]:path:' \
                        '--decay[Enable time decay]' \
                        '--no-decay[Disable time decay]' \
                        '--decay-half-life[Half-life in days]:days:' \
                        '--decay-exclude[Path prefixes excluded from decay]:patterns:' \
                        '--decay-include[Path prefixes where decay applies]:patterns:' \
                        '--hops[Number of link hops for graph boosting (1-3)]:hops:' \
                        '--expand[Graph expansion depth for context (0-3)]:depth:'
                    ;;
                info)
                    _arguments \
                        '1:path:_directories'
                    ;;
                collection|list)
                    _arguments \
                        '1:path:_directories' \
                        '(-r --recursive)'{-r,--recursive}'[Include nested subfolders]' \
                        '--sort[Frontmatter field to sort by]:field:' \
                        '--order[Sort direction]:order:(asc desc)' \
                        '(-f --filter)'{-f,--filter}'[Metadata filter (KEY=VALUE)]:filter:' \
                        '--limit[Maximum rows to return]:number:' \
                        '--offset[Rows to skip]:number:'
                    ;;
            esac
            ;;
    esac
}
_mdvdb"#
                }
                ShellType::Fish => {
                    r#"# mdvdb fish completions
complete -c mdvdb -n '__fish_use_subcommand' -a search -d 'Semantic search across indexed markdown files'
complete -c mdvdb -n '__fish_use_subcommand' -a ingest -d 'Ingest markdown files into the index'
complete -c mdvdb -n '__fish_use_subcommand' -a status -d 'Show index status and configuration'
complete -c mdvdb -n '__fish_use_subcommand' -a info -d 'Show vault or folder stats'
complete -c mdvdb -n '__fish_use_subcommand' -a schema -d 'Show inferred metadata schema'
complete -c mdvdb -n '__fish_use_subcommand' -a clusters -d 'Show document clusters'
complete -c mdvdb -n '__fish_use_subcommand' -a tree -d 'Show file tree with sync status indicators'
complete -c mdvdb -n '__fish_use_subcommand' -a get -d 'Get metadata for a specific file'
complete -c mdvdb -n '__fish_use_subcommand' -a collection -d 'List a folder'\''s documents as a table'
complete -c mdvdb -n '__fish_use_subcommand' -a watch -d 'Watch for file changes and re-index automatically'
complete -c mdvdb -n '__fish_use_subcommand' -a modules -d 'Inspect and run built-in derived-data modules'
complete -c mdvdb -n '__fish_use_subcommand' -a init -d 'Initialize a new .markdownvdb config file'
complete -c mdvdb -n '__fish_use_subcommand' -a config -d 'Show resolved configuration'
complete -c mdvdb -n '__fish_use_subcommand' -a doctor -d 'Run diagnostic checks'
complete -c mdvdb -n '__fish_use_subcommand' -a links -d 'Show links originating from a file'
complete -c mdvdb -n '__fish_use_subcommand' -a backlinks -d 'Show backlinks pointing to a file'
complete -c mdvdb -n '__fish_use_subcommand' -a orphans -d 'Find orphan files with no links'
complete -c mdvdb -n '__fish_use_subcommand' -a completions -d 'Generate shell completions'

# Global flags
complete -c mdvdb -l verbose -s v -d 'Increase log verbosity'
complete -c mdvdb -l root -d 'Project root directory' -r -F
complete -c mdvdb -l no-color -d 'Disable colored output'
complete -c mdvdb -l json -d 'Output results as JSON'
complete -c mdvdb -l version -d 'Print version information'

# Ingest subcommand flags
complete -c mdvdb -n '__fish_seen_subcommand_from ingest' -l reindex -d 'Force re-embedding of all files'
complete -c mdvdb -n '__fish_seen_subcommand_from ingest' -l preview -d 'Preview what ingestion would do'
complete -c mdvdb -n '__fish_seen_subcommand_from ingest' -l file -d 'Ingest a specific file only' -r -F

# Search subcommand flags
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l limit -s l -d 'Maximum number of results'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l min-score -d 'Minimum similarity score'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l filter -s f -d 'Metadata filter (KEY=VALUE)'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l boost-links -d 'Boost linked results'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l no-boost-links -d 'Disable link boosting'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l mode -d 'Search mode' -r -a 'hybrid semantic lexical'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l semantic -d 'Shorthand for --mode=semantic'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l lexical -d 'Shorthand for --mode=lexical'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l path -d 'Restrict to path prefix'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l decay -d 'Enable time decay'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l no-decay -d 'Disable time decay'
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l decay-half-life -d 'Half-life in days' -r
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l decay-exclude -d 'Path prefixes excluded from decay' -r
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l decay-include -d 'Path prefixes where decay applies' -r
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l hops -d 'Number of link hops for graph boosting (1-3)' -r
complete -c mdvdb -n '__fish_seen_subcommand_from search' -l expand -d 'Graph expansion depth for context (0-3)' -r

# Collection subcommand flags
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l recursive -s r -d 'Include nested subfolders'
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l sort -d 'Frontmatter field to sort by' -r
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l order -d 'Sort direction' -r -a 'asc desc'
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l filter -s f -d 'Metadata filter (KEY=VALUE)' -r
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l limit -d 'Maximum rows to return' -r
complete -c mdvdb -n '__fish_seen_subcommand_from collection' -l offset -d 'Rows to skip' -r

# Init subcommand flags
complete -c mdvdb -n '__fish_seen_subcommand_from init' -l global -d 'Create global config'

# Completions subcommand
complete -c mdvdb -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish power-shell' -d 'Shell type'"#
                }
                ShellType::PowerShell => {
                    r#"# mdvdb PowerShell completions
Register-ArgumentCompleter -CommandName mdvdb -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @(
        @{ Name = 'search'; Tooltip = 'Semantic search across indexed markdown files' },
        @{ Name = 'ingest'; Tooltip = 'Ingest markdown files into the index' },
        @{ Name = 'status'; Tooltip = 'Show index status and configuration' },
        @{ Name = 'info'; Tooltip = 'Show vault or folder stats' },
        @{ Name = 'schema'; Tooltip = 'Show inferred metadata schema' },
        @{ Name = 'clusters'; Tooltip = 'Show document clusters' },
        @{ Name = 'tree'; Tooltip = 'Show file tree with sync status indicators' },
        @{ Name = 'get'; Tooltip = 'Get metadata for a specific file' },
        @{ Name = 'collection'; Tooltip = 'List a folder''s documents as a table' },
        @{ Name = 'watch'; Tooltip = 'Watch for file changes and re-index automatically' },
        @{ Name = 'modules'; Tooltip = 'Inspect and run built-in derived-data modules' },
        @{ Name = 'init'; Tooltip = 'Initialize a new .markdownvdb config file' },
        @{ Name = 'config'; Tooltip = 'Show resolved configuration' },
        @{ Name = 'doctor'; Tooltip = 'Run diagnostic checks' },
        @{ Name = 'links'; Tooltip = 'Show links originating from a file' },
        @{ Name = 'backlinks'; Tooltip = 'Show backlinks pointing to a file' },
        @{ Name = 'orphans'; Tooltip = 'Find orphan files with no links' }
    )
    $commands | Where-Object { $_.Name -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_.Name, $_.Name, 'ParameterValue', $_.Tooltip)
    }
}"#
                }
            };
            write!(std::io::stdout(), "{}", script)?;
            writeln!(std::io::stdout())?;
        }
        None => {
            format::print_logo();
            println!("{}", "  Run `mdvdb --help` for usage information.".dimmed());
        }
    }

    // Show update notice after command output completes
    if let Ok(Some(msg)) = update_handle.await {
        eprintln!("{msg}");
    }

    Ok(())
}

/// Parse a comma-separated seed list, rejecting '|' inside seeds.
fn parse_seed_list(seeds: Option<&str>) -> anyhow::Result<Vec<String>> {
    let list: Vec<String> = seeds
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for seed in &list {
        if seed.contains('|') {
            anyhow::bail!("seed phrases cannot contain '|'");
        }
    }
    Ok(list)
}

/// Treat an empty/whitespace description as None.
fn normalize_description(description: Option<String>) -> Option<String> {
    description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
}

/// Shared validation for topic definitions from the CLI.
fn validate_topic_fields(
    name: &str,
    seeds: &[String],
    description: Option<&str>,
    threshold: Option<f32>,
) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("topic name cannot be empty");
    }
    if name.contains(':') || name.contains('|') {
        anyhow::bail!("topic name cannot contain ':' or '|'");
    }
    if seeds.is_empty() && description.is_none() {
        anyhow::bail!("a topic needs --seeds or --description (or both)");
    }
    if let Some(t) = threshold {
        if !(0.0..=1.0).contains(&t) {
            anyhow::bail!("--threshold must be in [0.0, 1.0], got {t}");
        }
    }
    Ok(())
}

/// Parse a CLI string into a YAML scalar: bool and numbers when possible,
/// otherwise a string.
fn parse_yaml_scalar(value: &str) -> serde_yaml::Value {
    if let Ok(b) = value.parse::<bool>() {
        return serde_yaml::Value::Bool(b);
    }
    if let Ok(i) = value.parse::<i64>() {
        return serde_yaml::Value::Number(serde_yaml::Number::from(i));
    }
    if let Ok(f) = value.parse::<f64>() {
        return serde_yaml::Value::Number(serde_yaml::Number::from(f));
    }
    serde_yaml::Value::String(value.to_string())
}

/// Read custom cluster definitions from a YAML config file.
fn read_custom_clusters_from_yaml(yaml_path: &std::path::Path) -> Vec<mdvdb::CustomClusterDef> {
    let content = std::fs::read_to_string(yaml_path).unwrap_or_default();
    if content.is_empty() {
        return Vec::new();
    }
    let cfg: mdvdb::config::YamlConfig = serde_yaml::from_str(&content).unwrap_or_default();
    cfg.clustering
        .custom
        .into_iter()
        .map(|c| mdvdb::CustomClusterDef {
            name: c.name,
            description: c.description,
            seeds: c.seeds,
            threshold: c.threshold,
        })
        .collect()
}

/// Write custom cluster definitions to a YAML config file, preserving other settings.
fn write_custom_clusters_to_yaml(
    yaml_path: &std::path::Path,
    defs: &[mdvdb::CustomClusterDef],
) -> anyhow::Result<()> {
    let clusters: Vec<serde_yaml::Value> = defs
        .iter()
        .map(|d| {
            let mut map = serde_yaml::Mapping::new();
            map.insert(
                serde_yaml::Value::String("name".into()),
                serde_yaml::Value::String(d.name.clone()),
            );
            if let Some(desc) = &d.description {
                map.insert(
                    serde_yaml::Value::String("description".into()),
                    serde_yaml::Value::String(desc.clone()),
                );
            }
            if !d.seeds.is_empty() {
                map.insert(
                    serde_yaml::Value::String("seeds".into()),
                    serde_yaml::Value::Sequence(
                        d.seeds
                            .iter()
                            .map(|s| serde_yaml::Value::String(s.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(t) = d.threshold {
                map.insert(
                    serde_yaml::Value::String("threshold".into()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(t as f64)),
                );
            }
            serde_yaml::Value::Mapping(map)
        })
        .collect();

    mdvdb::config::update_yaml_config_value(
        yaml_path,
        "clustering.custom",
        serde_yaml::Value::Sequence(clusters),
    )?;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {:#}", e);
        process::exit(1);
    }
}
