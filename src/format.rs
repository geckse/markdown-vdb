use colored::Colorize;
use serde_json::Value;
use std::time::SystemTime;

use mdvdb::search::SearchResult;
use mdvdb::schema::{FieldType, Schema};
use mdvdb::links::{LinkQueryResult, LinkState, OrphanFile, ResolvedLink};
use mdvdb::tree::FileTree;
use mdvdb::ClusterSummary;
use mdvdb::IndexStatus;
use mdvdb::DocumentInfo;
use mdvdb::IngestResult;
use mdvdb::{CheckStatus, DoctorResult};
use mdvdb::config::Config;

/// Format a timestamp as a human-readable relative time string.
///
/// Uses `SystemTime` directly to avoid a `chrono` dependency.
pub fn format_timestamp(time: SystemTime) -> String {
    let elapsed = match SystemTime::now().duration_since(time) {
        Ok(d) => d,
        Err(_) => return "in the future".to_string(),
    };

    let secs = elapsed.as_secs();
    if secs < 60 {
        return "just now".to_string();
    }

    let mins = secs / 60;
    if mins < 60 {
        return if mins == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{mins} minutes ago")
        };
    }

    let hours = mins / 60;
    if hours < 24 {
        return if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        };
    }

    let days = hours / 24;
    if days > 30 {
        // Convert to absolute date format for old timestamps
        let epoch_secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        return format_epoch_datetime(epoch_secs);
    }
    if days == 1 {
        "1 day ago".to_string()
    } else {
        format!("{days} days ago")
    }
}

/// Format epoch seconds as "YYYY-MM-DD HH:MM:SS" without chrono.
fn format_epoch_datetime(epoch_secs: u64) -> String {
    // Days from epoch
    let total_days = epoch_secs / 86400;
    let day_secs = epoch_secs % 86400;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    // Convert total days since 1970-01-01 to year/month/day
    // Using a standard civil-from-days algorithm
    let z = total_days as i64 + 719468; // shift to 0000-03-01 epoch
    let era = z / 146097;
    let doe = z - era * 146097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y, m, d, hour, minute, second
    )
}

/// Format a byte count as a human-readable file size (1024-based).
pub fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// ASCII art logo for the mdvdb CLI.
const LOGO: &str = r#"
               _       _ _
  _ __ ___  __| |_   _| | |__
 | '_ ` _ \/ _` \ \ / / | '_ \
 | | | | | |_| |\ V /| |_| |_) |
 |_| |_| |_|\__,_| \_/ |_|_.__/
"#;

/// Print the ASCII logo in bold cyan to stdout.
pub fn print_logo() {
    for line in LOGO.trim_start_matches('\n').lines() {
        println!("{}", line.bold().cyan());
    }
}

/// Print the logo followed by version and tagline.
pub fn print_version() {
    print_logo();
    println!(
        "  {} {}",
        "v".dimmed(),
        env!("CARGO_PKG_VERSION").bold()
    );
    println!(
        "  {}",
        "Filesystem-native vector database for Markdown".dimmed()
    );
}

/// Render an ASCII progress/percentage bar.
///
/// `filled` is the number of filled segments, `total` is the bar width.
/// Uses `█` for filled and `░` for unfilled segments.
pub fn render_bar(filled: usize, total: usize) -> String {
    let filled = filled.min(total);
    let unfilled = total - filled;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(unfilled),
    )
}

/// Print search results with colored formatting to stdout.
///
/// Displays numbered results with score bars, colored scores, bold file paths,
/// section hierarchy, line ranges, content previews, file sizes, and frontmatter.
pub fn print_search_results(results: &[SearchResult], query: &str) {
    if results.is_empty() {
        println!(
            "  {} No results found for {}",
            "✗".red().bold(),
            format!("\"{}\"", query).yellow()
        );
        return;
    }

    println!(
        "{} {} result{} for {}\n",
        "Search:".bold(),
        results.len().to_string().bold(),
        if results.len() == 1 { "" } else { "s" },
        format!("\"{}\"", query).yellow()
    );

    for (i, r) in results.iter().enumerate() {
        // Score bar: map 0.0–1.0 to 0–10 filled segments
        let filled = (r.score * 10.0).round() as usize;
        let bar = render_bar(filled, 10);

        println!(
            "  {} {} {} {}",
            format!("{}.", i + 1).bold(),
            bar,
            format!("{:.4}", r.score).yellow(),
            r.file.path.bold()
        );

        // Section hierarchy
        if !r.chunk.heading_hierarchy.is_empty() {
            println!(
                "     {} {}",
                "Section:".dimmed(),
                r.chunk.heading_hierarchy.join(" > ").cyan()
            );
        }

        // Line range and file size
        let line_range = format!("{}-{}", r.chunk.start_line, r.chunk.end_line);
        let size_str = format!("({})", format_file_size(r.file.file_size));
        println!(
            "     {} {}  {}",
            "Lines:".dimmed(),
            line_range,
            size_str.dimmed()
        );

        // Content preview (first 200 chars, dimmed)
        let preview: String = r.chunk.content.chars().take(200).collect();
        let preview = preview.replace('\n', " ");
        if !preview.is_empty() {
            println!("     {}", preview.dimmed());
        }

        // Frontmatter key-value pairs
        if let Some(Value::Object(map)) = &r.file.frontmatter {
            if !map.is_empty() {
                let pairs: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        format!("{}: {}", k.dimmed(), val)
                    })
                    .collect();
                println!("     {}", pairs.join("  "));
            }
        }

        println!();
    }
}

/// Print ingest results with colored formatting to stdout.
///
/// Displays success/failure counts with colored indicators:
/// green checkmark and counts for successful operations,
/// red for failures, yellow for numeric values.
pub fn print_ingest_result(result: &IngestResult) {
    println!(
        "\n  {} {}\n",
        "✓".green().bold(),
        "Ingestion complete".bold()
    );
    println!(
        "  {}  {}",
        "Files indexed:".dimmed(),
        result.files_indexed.to_string().green()
    );
    println!(
        "  {}  {}",
        "Files skipped:".dimmed(),
        result.files_skipped.to_string().yellow()
    );
    println!(
        "  {}  {}",
        "Files removed:".dimmed(),
        result.files_removed.to_string().yellow()
    );
    println!(
        "  {} {}",
        "Chunks created:".dimmed(),
        result.chunks_created.to_string().yellow()
    );
    println!(
        "  {}      {}",
        "API calls:".dimmed(),
        result.api_calls.to_string().yellow()
    );

    if result.files_failed > 0 {
        println!(
            "  {}  {}",
            "Files failed:".dimmed(),
            result.files_failed.to_string().red().bold()
        );
        for err in &result.errors {
            eprintln!(
                "    {} {}: {}",
                "✗".red().bold(),
                err.path,
                err.message
            );
        }
    }
    println!();
}

/// Convert a Unix timestamp (seconds since epoch) to a SystemTime.
fn unix_to_system_time(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)
}

/// Print index status with colored formatting to stdout.
pub fn print_status(status: &IndexStatus) {
    println!("\n  {} {}\n", "●".cyan().bold(), "Index Status".bold());
    println!(
        "  {}  {}",
        "Documents:".cyan(),
        status.document_count.to_string().yellow()
    );
    println!(
        "  {}     {}",
        "Chunks:".cyan(),
        status.chunk_count.to_string().yellow()
    );
    println!(
        "  {}    {}",
        "Vectors:".cyan(),
        status.vector_count.to_string().yellow()
    );
    println!(
        "  {}  {}",
        "File size:".cyan(),
        format_file_size(status.file_size).yellow()
    );
    let updated = format_timestamp(unix_to_system_time(status.last_updated));
    println!(
        "  {}    {}",
        "Updated:".cyan(),
        updated
    );
    println!();
    println!("  {} {}", "Embedding:".cyan(), status.embedding_config.provider.bold());
    println!(
        "  {}      {}",
        "Model:".cyan(),
        status.embedding_config.model
    );
    println!(
        "  {} {}",
        "Dimensions:".cyan(),
        status.embedding_config.dimensions.to_string().yellow()
    );
    println!();
}

/// Print document info with colored formatting to stdout.
pub fn print_document(doc: &DocumentInfo) {
    println!("\n  {} {}\n", "●".cyan().bold(), doc.path.bold());
    println!(
        "  {}  {}",
        "File size:".cyan(),
        format_file_size(doc.file_size).yellow()
    );
    println!(
        "  {} {}",
        "Indexed at:".cyan(),
        format_timestamp(unix_to_system_time(doc.indexed_at))
    );
    println!(
        "  {}     {}",
        "Hash:".cyan(),
        doc.content_hash.dimmed()
    );
    println!(
        "  {}   {}",
        "Chunks:".cyan(),
        doc.chunk_count.to_string().yellow()
    );

    if let Some(Value::Object(map)) = &doc.frontmatter {
        if !map.is_empty() {
            println!();
            println!("  {}", "Frontmatter:".cyan());
            for (k, v) in map {
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                println!("    {}: {}", k.dimmed(), val);
            }
        }
    }
    println!();
}

/// Print the metadata schema with occurrence bars and field details.
pub fn print_schema(schema: &Schema, total_docs: usize) {
    if schema.fields.is_empty() {
        println!(
            "  {} No schema fields found. Run {} first.",
            "✗".red().bold(),
            "mdvdb ingest".yellow()
        );
        return;
    }

    println!(
        "\n  {} {} {}\n",
        "●".cyan().bold(),
        "Metadata Schema".bold(),
        format!("({} fields)", schema.fields.len()).dimmed()
    );

    for field in &schema.fields {
        // Field type display
        let type_str = match &field.field_type {
            FieldType::String => "string",
            FieldType::Number => "number",
            FieldType::Boolean => "boolean",
            FieldType::List => "list",
            FieldType::Date => "date",
            FieldType::Mixed => "mixed",
        };

        // Occurrence bar: 20-char width, proportional to total_docs
        let filled = if total_docs > 0 {
            ((field.occurrence_count as f64 / total_docs as f64) * 20.0).round() as usize
        } else {
            0
        };
        let bar = render_bar(filled, 20);

        // Required tag
        let required_tag = if field.required {
            format!(" {}", "[required]".yellow())
        } else {
            String::new()
        };

        println!(
            "  {} {} {} {}{}",
            field.name.bold(),
            format!("({})", type_str).dimmed(),
            bar,
            format!("{}/{}", field.occurrence_count, total_docs).dimmed(),
            required_tag
        );

        // Description
        if let Some(desc) = &field.description {
            println!("    {}", desc.dimmed());
        }

        // Sample values (dimmed)
        if !field.sample_values.is_empty() {
            let samples: Vec<&str> = field.sample_values.iter().take(5).map(|s| s.as_str()).collect();
            println!("    {} {}", "Samples:".dimmed(), samples.join(", ").dimmed());
        }

        // Allowed values
        if let Some(allowed) = &field.allowed_values {
            if !allowed.is_empty() {
                println!("    {} {}", "Allowed:".dimmed(), allowed.join(", ").cyan());
            }
        }

        println!();
    }
}

/// Print cluster summaries with distribution bars and keywords.
pub fn print_clusters(clusters: &[ClusterSummary]) {
    if clusters.is_empty() {
        println!(
            "  {} No clusters available. Run {} first.",
            "✗".red().bold(),
            "mdvdb ingest".yellow()
        );
        return;
    }

    let total_docs: usize = clusters.iter().map(|c| c.document_count).sum();
    let max_size = clusters.iter().map(|c| c.document_count).max().unwrap_or(1);

    println!(
        "\n  {} {} {}\n",
        "●".cyan().bold(),
        "Document Clusters".bold(),
        format!("({} clusters, {} documents)", clusters.len(), total_docs).dimmed()
    );

    for cluster in clusters {
        // Distribution bar: 20-char, proportional to max cluster size
        let filled = if max_size > 0 {
            ((cluster.document_count as f64 / max_size as f64) * 20.0).round() as usize
        } else {
            0
        };
        let bar = render_bar(filled, 20);

        // Label
        let label = cluster
            .label
            .as_deref()
            .filter(|l| !l.is_empty())
            .unwrap_or("(unlabeled)");

        println!(
            "  {} {} {} {}",
            format!("Cluster {}:", cluster.id).bold(),
            bar,
            format!("{} docs", cluster.document_count).yellow(),
            label
        );

        // Keywords (blue)
        if !cluster.keywords.is_empty() {
            let kw: Vec<String> = cluster.keywords.iter().map(|k| format!("{}", k.blue())).collect();
            println!("    {} {}", "Keywords:".dimmed(), kw.join(", "));
        }

        println!();
    }
}

/// Print file tree with colored formatting to stdout.
///
/// Uses `tree::render_tree()` for the actual rendering, then prints to stdout.
pub fn print_file_tree(tree: &FileTree, colored: bool) {
    use mdvdb::tree::render_tree;

    let rendered = render_tree(tree, colored);
    print!("{rendered}");
}

/// Print watch startup message with green text and directory list.
pub fn print_watch_started(dirs: &[String]) {
    println!(
        "\n  {} {}\n",
        "●".green().bold(),
        "Watching for changes".bold()
    );
    for dir in dirs {
        println!("  {}  {}", "→".green(), dir);
    }
    println!(
        "\n  {}",
        "Press Ctrl+C to stop".dimmed()
    );
}

/// Print init success message with green checkmark.
pub fn print_init_success(path: &str) {
    println!(
        "\n  {} {}\n",
        "✓".green().bold(),
        "Initialized".bold()
    );
    println!(
        "  {} {}",
        "Config:".dimmed(),
        format!("{}/.markdownvdb", path).bold()
    );
    println!(
        "  {}",
        "Edit it to configure your embedding provider and other settings.".dimmed()
    );
    println!();
}

/// Print link query results with tree-rendered outgoing and incoming links.
///
/// Shows outgoing links with broken/wikilink badges, incoming backlinks,
/// and a summary line with counts.
pub fn print_links(result: &LinkQueryResult) {
    let broken_count = result
        .outgoing
        .iter()
        .filter(|r| r.state == LinkState::Broken)
        .count();

    println!(
        "\n  {} {}\n",
        "●".cyan().bold(),
        result.file.bold()
    );

    // Outgoing links
    let outgoing_count = result.outgoing.len();
    println!(
        "  {} {}",
        "Outgoing:".cyan(),
        outgoing_count.to_string().yellow()
    );

    for (i, link) in result.outgoing.iter().enumerate() {
        let connector = if i == outgoing_count - 1 {
            "└──"
        } else {
            "├──"
        };

        let mut badges = String::new();
        if link.state == LinkState::Broken {
            badges.push_str(&format!(" {}", "[broken]".red()));
        }
        if link.entry.is_wikilink {
            badges.push_str(&format!(" {}", "[wikilink]".blue()));
        }

        println!(
            "  {} {} {}{}",
            connector.dimmed(),
            link.entry.target.bold(),
            format!("\"{}\"", link.entry.text).dimmed(),
            badges
        );
        println!(
            "  {}   {}",
            if i == outgoing_count - 1 { " " } else { "│" }.dimmed(),
            format!("line {}", link.entry.line_number).dimmed()
        );
    }

    if outgoing_count == 0 {
        println!("  {} {}", "└──".dimmed(), "(none)".dimmed());
    }

    println!();

    // Incoming links (backlinks)
    let incoming_count = result.incoming.len();
    println!(
        "  {} {}",
        "Incoming:".cyan(),
        incoming_count.to_string().yellow()
    );

    for (i, entry) in result.incoming.iter().enumerate() {
        let connector = if i == incoming_count - 1 {
            "└──"
        } else {
            "├──"
        };

        let mut badges = String::new();
        if entry.is_wikilink {
            badges.push_str(&format!(" {}", "[wikilink]".blue()));
        }

        println!(
            "  {} {} {}{}",
            connector.dimmed(),
            entry.source.bold(),
            format!("\"{}\"", entry.text).dimmed(),
            badges
        );
        println!(
            "  {}   {}",
            if i == incoming_count - 1 { " " } else { "│" }.dimmed(),
            format!("line {}", entry.line_number).dimmed()
        );
    }

    if incoming_count == 0 {
        println!("  {} {}", "└──".dimmed(), "(none)".dimmed());
    }

    // Summary
    println!();
    let mut summary = format!(
        "  {} outgoing, {} incoming",
        outgoing_count.to_string().yellow(),
        incoming_count.to_string().yellow()
    );
    if broken_count > 0 {
        summary.push_str(&format!(", {} {}", broken_count.to_string().red().bold(), "broken".red()));
    }
    println!("  {}", summary);
    println!();
}

/// Print backlinks (incoming links only) for a file.
///
/// Simpler layout than `print_links` — shows only files linking TO the given file.
pub fn print_backlinks(file_path: &str, backlinks: &[ResolvedLink]) {
    println!(
        "\n  {} {} {}\n",
        "●".cyan().bold(),
        "Backlinks to".cyan(),
        file_path.bold()
    );

    if backlinks.is_empty() {
        println!(
            "  {} No files link to {}",
            "✗".red().bold(),
            file_path.yellow()
        );
        println!();
        return;
    }

    println!(
        "  {} {} incoming link{}\n",
        "Incoming:".cyan(),
        backlinks.len().to_string().yellow(),
        if backlinks.len() == 1 { "" } else { "s" }
    );

    for (i, link) in backlinks.iter().enumerate() {
        let connector = if i == backlinks.len() - 1 {
            "└──"
        } else {
            "├──"
        };

        let mut badges = String::new();
        if link.entry.is_wikilink {
            badges.push_str(&format!(" {}", "[wikilink]".blue()));
        }

        println!(
            "  {} {} {}{}",
            connector.dimmed(),
            link.entry.source.bold(),
            format!("\"{}\"", link.entry.text).dimmed(),
            badges
        );
        println!(
            "  {}   {}",
            if i == backlinks.len() - 1 { " " } else { "│" }.dimmed(),
            format!("line {}", link.entry.line_number).dimmed()
        );
    }

    println!();
}

/// Print orphan files (files with no incoming or outgoing links).
pub fn print_orphans(orphans: &[OrphanFile]) {
    if orphans.is_empty() {
        println!(
            "\n  {} No orphan files found — all files are connected.\n",
            "✓".green().bold()
        );
        return;
    }

    println!(
        "\n  {} {} {} {}\n",
        "●".yellow().bold(),
        "Orphan Files".bold(),
        format!("({})", orphans.len()).dimmed(),
        "— no incoming or outgoing links".dimmed()
    );

    for orphan in orphans {
        println!("  {} {}", "•".yellow(), orphan.path.bold());
    }

    println!(
        "\n  {} {} orphan file{}\n",
        "Total:".dimmed(),
        orphans.len().to_string().yellow(),
        if orphans.len() == 1 { "" } else { "s" }
    );
}

/// Print success message for `mdvdb init --global`.
pub fn print_init_global_success(path: &str) {
    println!(
        "\n  {} {}\n",
        "✓".green().bold(),
        "User config initialized".bold()
    );
    println!(
        "  {} {}",
        "Config:".dimmed(),
        path.bold()
    );
    println!(
        "  {}",
        "Uncomment and set your API key and default settings.".dimmed()
    );
    println!();
}

/// Print the resolved configuration with colored formatting.
pub fn print_config(config: &Config, user_config_path: Option<&std::path::Path>) {
    println!("\n  {} {}\n", "●".cyan().bold(), "Configuration".bold());

    println!(
        "  {}     {:?}",
        "Provider:".cyan(),
        config.embedding_provider
    );
    println!(
        "  {}        {}",
        "Model:".cyan(),
        config.embedding_model
    );
    println!(
        "  {}   {}",
        "Dimensions:".cyan(),
        config.embedding_dimensions.to_string().yellow()
    );
    println!(
        "  {}   {}",
        "Batch size:".cyan(),
        config.embedding_batch_size.to_string().yellow()
    );

    let key_status = if config.openai_api_key.is_some() {
        "set".green().to_string()
    } else {
        "not set".yellow().to_string()
    };
    println!("  {}      {}", "API key:".cyan(), key_status);

    println!(
        "  {}  {}",
        "Ollama host:".cyan(),
        config.ollama_host
    );

    let dirs: Vec<String> = config
        .source_dirs
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .collect();
    println!(
        "  {}  {}",
        "Source dirs:".cyan(),
        dirs.join(", ")
    );

    if !config.ignore_patterns.is_empty() {
        println!(
            "  {}      {}",
            "Ignore:".cyan(),
            config.ignore_patterns.join(", ")
        );
    }

    println!();
    println!(
        "  {}  {} tokens max, {} overlap",
        "Chunking:".cyan(),
        config.chunk_max_tokens.to_string().yellow(),
        config.chunk_overlap_tokens.to_string().yellow()
    );

    println!(
        "  {}    mode={:?}, limit={}, min_score={}, rrf_k={}",
        "Search:".cyan(),
        config.search_default_mode,
        config.search_default_limit,
        config.search_min_score,
        config.search_rrf_k
    );

    println!(
        "  {}  enabled={}, debounce={}ms",
        "Watching:".cyan(),
        config.watch_enabled,
        config.watch_debounce_ms
    );

    println!(
        "  {}  enabled={}, threshold={}",
        "Clustering:".cyan(),
        config.clustering_enabled,
        config.clustering_rebalance_threshold
    );

    if let Some(path) = user_config_path {
        println!();
        let status = if path.is_file() { "exists" } else { "not found" };
        println!(
            "  {} {} ({})",
            "User config:".dimmed(),
            path.display(),
            status
        );
    }
    println!();
}

/// Print doctor diagnostic results with colored pass/fail indicators.
pub fn print_doctor(result: &DoctorResult) {
    println!("\n  {} {}\n", "●".cyan().bold(), "mdvdb doctor".bold());

    for check in &result.checks {
        let icon = match check.status {
            CheckStatus::Pass => "✓".green().bold(),
            CheckStatus::Fail => "✗".red().bold(),
            CheckStatus::Warn => "!".yellow().bold(),
        };

        // Pad the check name for alignment.
        println!(
            "  {} {:<25} {}",
            icon,
            check.name,
            check.detail.dimmed()
        );
    }

    println!(
        "\n  {}/{} checks passed\n",
        result.passed.to_string().green().bold(),
        result.total
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamp_just_now() {
        let time = SystemTime::now() - Duration::from_secs(30);
        assert_eq!(format_timestamp(time), "just now");
    }

    #[test]
    fn timestamp_one_minute() {
        let time = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(format_timestamp(time), "1 minute ago");
    }

    #[test]
    fn timestamp_multiple_minutes() {
        let time = SystemTime::now() - Duration::from_secs(300);
        assert_eq!(format_timestamp(time), "5 minutes ago");
    }

    #[test]
    fn timestamp_one_hour() {
        let time = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(format_timestamp(time), "1 hour ago");
    }

    #[test]
    fn timestamp_multiple_hours() {
        let time = SystemTime::now() - Duration::from_secs(7200);
        assert_eq!(format_timestamp(time), "2 hours ago");
    }

    #[test]
    fn timestamp_one_day() {
        let time = SystemTime::now() - Duration::from_secs(86400);
        assert_eq!(format_timestamp(time), "1 day ago");
    }

    #[test]
    fn timestamp_multiple_days() {
        let time = SystemTime::now() - Duration::from_secs(86400 * 7);
        assert_eq!(format_timestamp(time), "7 days ago");
    }

    #[test]
    fn timestamp_future() {
        let time = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(format_timestamp(time), "in the future");
    }

    #[test]
    fn test_format_timestamp_old_date() {
        // ~60 days ago should show YYYY-MM-DD HH:MM:SS
        let time = SystemTime::now() - Duration::from_secs(86400 * 60);
        let result = format_timestamp(time);
        // Should NOT contain "days ago"
        assert!(!result.contains("days ago"), "Expected date format, got: {}", result);
        // Should match YYYY-MM-DD HH:MM:SS pattern
        assert!(
            result.len() == 19 && result.chars().nth(4) == Some('-') && result.chars().nth(10) == Some(' '),
            "Expected YYYY-MM-DD HH:MM:SS format, got: {}",
            result
        );
    }

    #[test]
    fn test_format_timestamp_epoch_zero() {
        let time = SystemTime::UNIX_EPOCH;
        let result = format_timestamp(time);
        assert_eq!(result, "1970-01-01 00:00:00");
    }

    #[test]
    fn test_format_timestamp_30_days_still_relative() {
        let time = SystemTime::now() - Duration::from_secs(86400 * 30);
        let result = format_timestamp(time);
        assert_eq!(result, "30 days ago");
    }

    #[test]
    fn file_size_bytes() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1023), "1023 B");
    }

    #[test]
    fn file_size_kilobytes() {
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
    }

    #[test]
    fn file_size_megabytes() {
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 5), "5.0 MB");
    }

    #[test]
    fn file_size_gigabytes() {
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn bar_full() {
        assert_eq!(render_bar(10, 10), "██████████");
    }

    #[test]
    fn bar_empty() {
        assert_eq!(render_bar(0, 10), "░░░░░░░░░░");
    }

    #[test]
    fn bar_half() {
        assert_eq!(render_bar(5, 10), "█████░░░░░");
    }

    #[test]
    fn bar_overflow_clamped() {
        assert_eq!(render_bar(15, 10), "██████████");
    }

    #[test]
    fn bar_zero_width() {
        assert_eq!(render_bar(0, 0), "");
    }

    #[test]
    fn test_logo() {
        // Disable colors for deterministic assertions
        colored::control::set_override(false);

        // Verify logo content
        // ASCII art spells out "mdvdb" in stylized form
        assert!(LOGO.contains("__,_"));
        // Logo lines should be under 40 chars wide
        for line in LOGO.lines() {
            assert!(
                line.len() <= 40,
                "Logo line too wide ({} chars): {:?}",
                line.len(),
                line
            );
        }
        // Logo should be 3-5 content lines
        let content_lines: Vec<&str> = LOGO.trim().lines().collect();
        assert!(
            (3..=5).contains(&content_lines.len()),
            "Logo should be 3-5 lines, got {}",
            content_lines.len()
        );
    }

    #[test]
    fn test_version_contains_version_string() {
        colored::control::set_override(false);
        // Just verify the version string is accessible
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
    }
}
