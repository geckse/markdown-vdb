mod format;

use std::io::Write;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use serde_json::Value;

use mdvdb::links::{LinkQueryResult, OrphanFile, ResolvedLink};
use mdvdb::search::{GraphContextItem, MetadataFilter, SearchMode, SearchQuery, SearchResult, SearchTimings};
use mdvdb::{GraphLevel, IngestTimings, MarkdownVdb};

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
    duration_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<IngestTimings>,
    cancelled: bool,
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

    /// Show inferred metadata schema
    Schema(SchemaArgs),

    /// Show document clusters
    Clusters(ClustersArgs),

    /// Show file tree with sync status indicators
    Tree(TreeArgs),

    /// Get metadata for a specific file
    Get(GetArgs),

    /// Watch for file changes and re-index automatically
    Watch(WatchArgs),

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
    #[arg(long, conflicts_with_all = ["lexical", "mode"])]
    semantic: bool,

    /// Shorthand for --mode=lexical
    #[arg(long, conflicts_with_all = ["semantic", "mode"])]
    lexical: bool,

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
struct SchemaArgs {
    /// Restrict schema to files under this path prefix
    #[arg(long)]
    path: Option<String>,
}

#[derive(Parser)]
struct ClustersArgs {}

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
}

#[derive(Parser)]
struct WatchArgs {}

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
struct GraphArgs {
    /// Graph granularity level
    #[arg(long, value_enum, default_value = "document")]
    level: GraphLevelArg,

    /// Restrict graph to files under this path prefix
    #[arg(long)]
    path: Option<String>,
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
struct ConfigArgs {}

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

    // Disable colors if --no-color flag, NO_COLOR env var, or JSON mode is active.
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        colored::control::set_override(false);
    }

    if cli.version {
        format::print_version();
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

            let effective_mode = query.mode;
            let response = vdb.search(query).await?;

            if json {
                let output = SearchOutput {
                    total_results: response.results.len(),
                    query: args.query.clone(),
                    results: response.results,
                    mode: effective_mode,
                    timings: if cli.verbose > 0 { Some(response.timings) } else { None },
                    graph_context: response.graph_context,
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
                        "  {spinner:.green} [{pos}/{len}] {msg} {wide_bar:.cyan/dim} {percent}%"
                    )
                    .unwrap()
                    .progress_chars("█░░"),
                );
                main_bar.enable_steady_tick(std::time::Duration::from_millis(120));

                let status_bar = mp.add(indicatif::ProgressBar::new_spinner());
                status_bar.set_style(
                    indicatif::ProgressStyle::with_template(
                        "  {spinner:.dim} {msg}"
                    )
                    .unwrap(),
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
                        mdvdb::IngestPhase::Parsing { current, total, path } => {
                            main_bar.set_length(*total as u64);
                            main_bar.set_position(*current as u64);
                            main_bar.set_message(path.to_string());
                            status_bar.set_message(format!("[{elapsed_str}] parsing {current}/{total}"));
                        }
                        mdvdb::IngestPhase::Skipped { current, total, path } => {
                            main_bar.set_length(*total as u64);
                            main_bar.set_position(*current as u64);
                            main_bar.set_message(format!("{path} (skipped)"));
                            status_bar.set_message(format!("[{elapsed_str}] skipped {current}/{total}"));
                        }
                        mdvdb::IngestPhase::Embedding { batch, total_batches } => {
                            main_bar.set_message(format!("Embedding batch {batch}/{total_batches}"));
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
                    duration_secs: result.duration_secs,
                    timings: if cli.verbose > 0 { result.timings.clone() } else { None },
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
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let status = vdb.status();

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &status)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_status(&status);
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
                    format::print_schema(&scoped.schema, vdb_status.document_count, Some(&prefix));
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
        Some(Commands::Clusters(_args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let clusters = vdb.clusters()?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &clusters)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_clusters(&clusters);
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
            let path_str = args.file_path.to_string_lossy();
            let doc = vdb.get_document(&path_str)?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &doc)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_document(&doc);
            }
        }
        Some(Commands::Links(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let path_str = args.file_path.to_string_lossy().to_string();
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
            let path_str = args.file_path.to_string_lossy().to_string();
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
        Some(Commands::Graph(args)) => {
            let vdb = MarkdownVdb::open_readonly_with_config(cwd, config)?;
            let level = match args.level {
                GraphLevelArg::Document => GraphLevel::Document,
                GraphLevelArg::Chunk => GraphLevel::Chunk,
            };
            let data = vdb.graph(level, args.path.as_deref())?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &data)?;
                writeln!(std::io::stdout())?;
            } else {
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
                let msg = serde_json::json!({"status": "watching", "message": "File watching started"});
                let line = serde_json::to_string(&msg)?;
                println!("{line}");
            } else {
                let dirs: Vec<String> = vdb.config().source_dirs.iter()
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
        Some(Commands::Config(_args)) => {
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &config)?;
                writeln!(std::io::stdout())?;
            } else {
                let user_config = mdvdb::config::Config::user_config_path();
                format::print_config(&config, user_config.as_deref());
            }
        }
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
                .filter(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|ext| ext == "md")
                })
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
                        "file_path": file_name.to_string_lossy(),
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
    commands="search ingest status schema clusters tree get watch init config doctor links backlinks orphans completions"

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
        'schema:Show inferred metadata schema'
        'clusters:Show document clusters'
        'tree:Show file tree with sync status indicators'
        'get:Get metadata for a specific file'
        'watch:Watch for file changes and re-index automatically'
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
complete -c mdvdb -n '__fish_use_subcommand' -a schema -d 'Show inferred metadata schema'
complete -c mdvdb -n '__fish_use_subcommand' -a clusters -d 'Show document clusters'
complete -c mdvdb -n '__fish_use_subcommand' -a tree -d 'Show file tree with sync status indicators'
complete -c mdvdb -n '__fish_use_subcommand' -a get -d 'Get metadata for a specific file'
complete -c mdvdb -n '__fish_use_subcommand' -a watch -d 'Watch for file changes and re-index automatically'
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
        @{ Name = 'schema'; Tooltip = 'Show inferred metadata schema' },
        @{ Name = 'clusters'; Tooltip = 'Show document clusters' },
        @{ Name = 'tree'; Tooltip = 'Show file tree with sync status indicators' },
        @{ Name = 'get'; Tooltip = 'Get metadata for a specific file' },
        @{ Name = 'watch'; Tooltip = 'Watch for file changes and re-index automatically' },
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

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {:#}", e);
        process::exit(1);
    }
}
