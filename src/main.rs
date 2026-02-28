mod format;

use std::io::Write;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use serde_json::Value;

use mdvdb::links::{LinkQueryResult, OrphanFile, ResolvedLink};
use mdvdb::search::{MetadataFilter, SearchMode, SearchQuery, SearchResult};
use mdvdb::MarkdownVdb;

/// Wrapped search output for JSON mode.
#[derive(serde::Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
    query: String,
    total_results: usize,
    mode: SearchMode,
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

    /// Boost results that are linked to/from the query matches
    #[arg(long)]
    boost_links: bool,

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
struct SchemaArgs {}

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
}

#[derive(Parser)]
struct BacklinksArgs {
    /// Path to the markdown file
    file_path: PathBuf,
}

#[derive(Parser)]
struct OrphansArgs {}

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

    mdvdb::logging::init(cli.verbose)?;

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
            if args.boost_links {
                query = query.with_boost_links(true);
            }
            query = query.with_mode(mode);
            if let Some(ref path) = args.path {
                query = query.with_path_prefix(path);
            }

            let effective_mode = query.mode;
            let results = vdb.search(query).await?;

            if json {
                let output = SearchOutput {
                    total_results: results.len(),
                    query: args.query.clone(),
                    results,
                    mode: effective_mode,
                };
                serde_json::to_writer_pretty(std::io::stdout(), &output)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_search_results(&results, &args.query);
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
                            main_bar.set_message(format!("{path}"));
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
                serde_json::to_writer_pretty(std::io::stdout(), &result)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_ingest_result(&result);
            }
        }
        Some(Commands::Status(_args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let status = vdb.status();

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &status)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_status(&status);
            }
        }
        Some(Commands::Schema(_args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let schema = vdb.schema()?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &schema)?;
                writeln!(std::io::stdout())?;
            } else {
                let vdb_status = vdb.status();
                format::print_schema(&schema, vdb_status.document_count);
            }
        }
        Some(Commands::Clusters(_args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let clusters = vdb.clusters()?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &clusters)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_clusters(&clusters);
            }
        }
        Some(Commands::Tree(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
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
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
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
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let path_str = args.file_path.to_string_lossy().to_string();
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
        Some(Commands::Backlinks(args)) => {
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
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
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
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
                serde_json::to_writer_pretty(std::io::stdout(), &msg)?;
                writeln!(std::io::stdout())?;
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
            let vdb = MarkdownVdb::open_with_config(cwd, config)?;
            let result = vdb.doctor().await?;

            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &result)?;
                writeln!(std::io::stdout())?;
            } else {
                format::print_doctor(&result);
            }
        }
        Some(Commands::Completions(args)) => {
            // Shell completion generation.
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
complete -c mdvdb -n '__fish_use_subcommand' -a tree -d 'Show file tree with sync status indicators'
complete -c mdvdb -n '__fish_use_subcommand' -a get -d 'Get metadata for a specific file'
complete -c mdvdb -n '__fish_use_subcommand' -a watch -d 'Watch for file changes and re-index automatically'
complete -c mdvdb -n '__fish_use_subcommand' -a init -d 'Initialize a new .markdownvdb config file'
complete -c mdvdb -n '__fish_use_subcommand' -a config -d 'Show resolved configuration'
complete -c mdvdb -n '__fish_use_subcommand' -a doctor -d 'Run diagnostic checks'
complete -c mdvdb -n '__fish_use_subcommand' -a links -d 'Show links originating from a file'
complete -c mdvdb -n '__fish_use_subcommand' -a backlinks -d 'Show backlinks pointing to a file'
complete -c mdvdb -n '__fish_use_subcommand' -a orphans -d 'Find orphan files with no links'"#
                }
                ShellType::PowerShell => {
                    r#"# mdvdb PowerShell completions
Register-ArgumentCompleter -CommandName mdvdb -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @('search', 'ingest', 'status', 'schema', 'clusters', 'tree', 'get', 'watch', 'init', 'config', 'doctor', 'links', 'backlinks', 'orphans')
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
