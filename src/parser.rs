use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::Error;

/// A parsed markdown file with extracted metadata.
#[derive(Debug, Clone, Serialize)]
pub struct MarkdownFile {
    /// Relative path to the markdown file.
    pub path: PathBuf,
    /// YAML frontmatter parsed as dynamic JSON value, if present.
    pub frontmatter: Option<serde_json::Value>,
    /// Headings extracted from the document.
    pub headings: Vec<Heading>,
    /// Raw body content (everything after frontmatter).
    pub body: String,
    /// SHA-256 hex digest of the full file content.
    pub content_hash: String,
    /// File size in bytes.
    pub file_size: u64,
    /// Links extracted from the document body.
    pub links: Vec<RawLink>,
    /// Filesystem modification time as Unix timestamp (seconds since epoch).
    pub modified_at: u64,
    /// Whole-value link references extracted from frontmatter values.
    /// Deliberately separate from `links` so the semantic-edge pipeline and
    /// chunking never see frontmatter references (they have no paragraph context).
    pub frontmatter_links: Vec<FrontmatterLink>,
}

/// A whole-value link reference extracted from a frontmatter field.
#[derive(Debug, Clone, Serialize)]
pub struct FrontmatterLink {
    /// The frontmatter field name this reference originates from.
    pub field: String,
    /// The literal frontmatter value (or list element).
    pub raw: String,
    /// The inner link target (wiki inner before `|` / markdown link target / bare path).
    pub target: String,
    /// The display text (alias or link text), falling back to the target.
    pub text: String,
    /// Whether this was a `[[wikilink]]`.
    pub is_wikilink: bool,
}

/// A raw link extracted from a markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct RawLink {
    /// The target path of the link (relative to the markdown file or project root).
    pub target: String,
    /// The display text of the link.
    pub text: String,
    /// 1-based line number where the link appears.
    pub line_number: usize,
    /// Whether this is a wikilink (`[[...]]`) or standard markdown link.
    pub is_wikilink: bool,
}

/// Parse a markdown file from disk into a [`MarkdownFile`].
///
/// Reads the file at `project_root.join(relative_path)`, extracts frontmatter,
/// headings, content hash, and file size. Returns `Error::MarkdownParse` for
/// non-UTF-8 files.
pub fn parse_markdown_file(
    project_root: &Path,
    relative_path: &Path,
) -> Result<MarkdownFile, Error> {
    let full_path = project_root.join(relative_path);
    let raw_bytes = std::fs::read(&full_path)?;
    let file_size = raw_bytes.len() as u64;

    // Capture filesystem modification time.
    let modified_at = std::fs::metadata(&full_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let content = String::from_utf8(raw_bytes).map_err(|_| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "file is not valid UTF-8".into(),
    })?;

    let content_hash = compute_content_hash(&content);
    let (frontmatter, body) = extract_frontmatter(&content);
    let frontmatter_links = extract_frontmatter_links(frontmatter.as_ref());
    let headings = extract_headings(body);
    let links = extract_links(body);

    Ok(MarkdownFile {
        path: relative_path.to_path_buf(),
        frontmatter,
        headings,
        body: body.to_string(),
        content_hash,
        file_size,
        links,
        modified_at,
        frontmatter_links,
    })
}

/// Extract whole-value link references from frontmatter.
///
/// Iterates top-level object entries; `String` values and `String` elements of
/// `Array` values are classified with the whole-value link-shape predicate
/// ([`crate::relations::is_link_shaped`]). Nested objects, non-string values,
/// and non-link strings are skipped. List elements keep their source order.
pub fn extract_frontmatter_links(frontmatter: Option<&serde_json::Value>) -> Vec<FrontmatterLink> {
    let Some(serde_json::Value::Object(map)) = frontmatter else {
        return Vec::new();
    };

    let mut result = Vec::new();
    let mut push = |field: &str, raw: &str| {
        if let Some(parsed) = crate::relations::parse_link_shaped(raw) {
            if crate::relations::parsed_link_kind(&parsed)
                == crate::relations::FrontmatterLinkKind::File
            {
                return;
            }
            result.push(FrontmatterLink {
                field: field.to_string(),
                raw: raw.to_string(),
                target: parsed.target,
                text: parsed.text,
                is_wikilink: parsed.is_wikilink,
            });
        }
    };

    for (field, value) in map {
        match value {
            serde_json::Value::String(s) => push(field, s),
            serde_json::Value::Array(items) => {
                for item in items {
                    if let serde_json::Value::String(s) = item {
                        push(field, s);
                    }
                }
            }
            _ => {}
        }
    }

    result
}

/// A heading extracted from a markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct Heading {
    /// Heading level (1-6).
    pub level: u8,
    /// The text content of the heading.
    pub text: String,
    /// 1-based line number where the heading appears.
    pub line_number: usize,
}

/// True only for an exact frontmatter delimiter at column zero.
///
/// Keep every frontmatter reader/writer on this predicate. In particular,
/// indented `---` inside a block scalar and suffix-bearing lines such as
/// `--- # comment` are YAML content, never envelope boundaries.
pub(crate) fn is_frontmatter_delimiter_line(line: &str) -> bool {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    line == "---"
}

/// Extract YAML frontmatter from markdown content.
///
/// Frontmatter must appear at the very start of the file, delimited by exact
/// column-zero `---` lines. Returns `None` if no frontmatter is present or if
/// it is malformed.
pub fn extract_frontmatter(content: &str) -> (Option<serde_json::Value>, &str) {
    let trimmed = content.trim_start_matches('\u{feff}'); // strip BOM
    let after_open = match trimmed.find('\n') {
        Some(index) => index + 1,
        None => return (None, content),
    };
    if !is_frontmatter_delimiter_line(&trimmed[..after_open]) {
        return (None, content);
    }

    let mut cursor = after_open;
    let (closing_start, body_start) = loop {
        if cursor >= trimmed.len() {
            tracing::warn!("frontmatter missing closing ---");
            return (None, content);
        }
        let remaining = &trimmed[cursor..];
        let line_len = remaining
            .find('\n')
            .map_or(remaining.len(), |index| index + 1);
        let line = &remaining[..line_len];
        if is_frontmatter_delimiter_line(line) {
            break (cursor, cursor + line_len);
        }
        cursor += line_len;
    };

    let yaml_str = &trimmed[after_open..closing_start];
    let body = &trimmed[body_start..];

    let yaml_trimmed = yaml_str.trim();
    if yaml_trimmed.is_empty() {
        return (None, body);
    }

    match serde_yaml::from_str::<serde_yaml::Value>(yaml_trimmed) {
        Ok(yaml_val) => {
            let float_count = count_yaml_float_numbers(&yaml_val);
            let mut float_lexemes = yaml_float_lexemes(yaml_trimmed)
                .filter(|lexemes| lexemes.len() == float_count)
                .unwrap_or_default()
                .into();
            let json_val = yaml_to_json(yaml_val, &mut float_lexemes);
            (Some(json_val), body)
        }
        Err(e) => {
            tracing::warn!("failed to parse frontmatter YAML: {e}");
            (None, body)
        }
    }
}

/// Collect plain floating-point scalar tokens before serde_yaml normalizes
/// them through `f64`. libyaml is already serde_yaml's parser backend; its
/// event API exposes the original UTF-8 scalar bytes.
fn yaml_float_lexemes(source: &str) -> Option<Vec<String>> {
    use std::mem::MaybeUninit;
    use std::slice;
    use unsafe_libyaml::{
        yaml_event_delete, yaml_event_t, yaml_parser_delete, yaml_parser_initialize,
        yaml_parser_parse, yaml_parser_set_input_string, yaml_parser_t, YAML_ALIAS_EVENT,
        YAML_PLAIN_SCALAR_STYLE, YAML_SCALAR_EVENT, YAML_STREAM_END_EVENT,
    };

    let mut lexemes = Vec::new();
    let mut parser = MaybeUninit::<yaml_parser_t>::uninit();
    let parser = parser.as_mut_ptr();

    // SAFETY: libyaml initializes `parser` before it is read. `source` remains
    // alive until `yaml_parser_delete`, and every successfully produced event
    // is deleted exactly once.
    unsafe {
        if yaml_parser_initialize(parser).fail {
            return None;
        }
        yaml_parser_set_input_string(parser, source.as_ptr(), source.len() as u64);

        let mut event = MaybeUninit::<yaml_event_t>::uninit();
        let event = event.as_mut_ptr();
        loop {
            if yaml_parser_parse(parser, event).fail {
                yaml_parser_delete(parser);
                return None;
            }
            let event_type = (*event).type_;

            // Aliases duplicate values in serde_yaml's resolved tree without
            // duplicating scalar events, so positional pairing is ambiguous.
            if event_type == YAML_ALIAS_EVENT {
                yaml_event_delete(event);
                yaml_parser_delete(parser);
                return None;
            }

            if event_type == YAML_SCALAR_EVENT {
                let scalar = (*event).data.scalar;
                if scalar.style == YAML_PLAIN_SCALAR_STYLE {
                    let bytes = slice::from_raw_parts(scalar.value, scalar.length as usize);
                    if let Ok(raw) = std::str::from_utf8(bytes) {
                        if matches!(
                            serde_yaml::from_str::<serde_yaml::Value>(raw),
                            Ok(serde_yaml::Value::Number(number))
                                if number.as_i64().is_none() && number.as_u64().is_none()
                        ) {
                            lexemes.push(raw.to_string());
                        }
                    }
                }
            }

            yaml_event_delete(event);
            if event_type == YAML_STREAM_END_EVENT {
                break;
            }
        }
        yaml_parser_delete(parser);
    }
    Some(lexemes)
}

fn count_yaml_float_numbers(value: &serde_yaml::Value) -> usize {
    match value {
        serde_yaml::Value::Number(number)
            if number.as_i64().is_none() && number.as_u64().is_none() =>
        {
            1
        }
        serde_yaml::Value::Sequence(values) => values.iter().map(count_yaml_float_numbers).sum(),
        serde_yaml::Value::Mapping(values) => values
            .iter()
            .map(|(key, value)| count_yaml_float_numbers(key) + count_yaml_float_numbers(value))
            .sum(),
        serde_yaml::Value::Tagged(tagged) => count_yaml_float_numbers(&tagged.value),
        _ => 0,
    }
}

fn json_number_from_yaml_lexeme(raw: &str) -> Option<serde_json::Number> {
    use std::str::FromStr;

    let mut token = raw.replace('_', "");
    if token.starts_with('+') {
        token.remove(0);
    }
    let sign_len = usize::from(token.starts_with('-'));
    if token[sign_len..].starts_with('.') {
        token.insert(sign_len, '0');
    }
    let exponent = token.find(['e', 'E']).unwrap_or(token.len());
    if token[..exponent].ends_with('.') {
        token.insert(exponent, '0');
    }
    serde_json::Number::from_str(&token).ok()
}

/// Convert a serde_yaml::Value to serde_json::Value.
fn yaml_to_json(
    val: serde_yaml::Value,
    float_lexemes: &mut std::collections::VecDeque<String>,
) -> serde_json::Value {
    match val {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else {
                // Preserve the source token instead of the serde_yaml f64.
                // serde_json's arbitrary_precision feature retains it for the
                // decimal formula runtime.
                float_lexemes
                    .pop_front()
                    .as_deref()
                    .and_then(json_number_from_yaml_lexeme)
                    .or_else(|| {
                        use std::str::FromStr;
                        serde_json::Number::from_str(&n.to_string()).ok()
                    })
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s),
        serde_yaml::Value::Sequence(seq) => serde_json::Value::Array(
            seq.into_iter()
                .map(|value| yaml_to_json(value, float_lexemes))
                .collect(),
        ),
        serde_yaml::Value::Mapping(map) => {
            let obj = map
                .into_iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        serde_yaml::Value::String(s) => s,
                        other => {
                            // Consume any float lexemes belonging to complex
                            // mapping keys before visiting the value.
                            let float_key_count = count_yaml_float_numbers(&other);
                            for _ in 0..float_key_count {
                                float_lexemes.pop_front();
                            }
                            serde_yaml::to_string(&other).ok()?.trim().to_string()
                        }
                    };
                    Some((key, yaml_to_json(v, float_lexemes)))
                })
                .collect();
            serde_json::Value::Object(obj)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(tagged.value, float_lexemes),
    }
}

/// Extract headings from markdown content.
///
/// Uses `pulldown_cmark::Parser` to find all headings (h1-h6) and returns them
/// with their text content and 1-based line numbers.
pub fn extract_headings(content: &str) -> Vec<Heading> {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let parser = Parser::new_ext(content, Options::all());
    let mut headings = Vec::new();
    let mut in_heading: Option<(u8, usize)> = None; // (level, byte_offset)
    let mut heading_text = String::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let byte_offset = range.start;
                let level_num = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                in_heading = Some((level_num, byte_offset));
                heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, byte_offset)) = in_heading.take() {
                    let line_number = content[..byte_offset].matches('\n').count() + 1;
                    headings.push(Heading {
                        level,
                        text: heading_text.trim().to_string(),
                        line_number,
                    });
                    heading_text.clear();
                }
            }
            Event::Text(text) | Event::Code(text) if in_heading.is_some() => {
                heading_text.push_str(&text);
            }
            _ => {}
        }
    }

    headings
}

/// Extract internal links from markdown content.
///
/// Finds standard markdown links `[text](target)` using pulldown_cmark and
/// wikilinks `[[target]]` or `[[target|text]]` using regex. Filters out
/// external URLs (http://, https://, mailto:) and anchor-only links (#heading).
pub fn extract_links(content: &str) -> Vec<RawLink> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut links = Vec::new();

    // Pre-scan for wikilinks using regex (pulldown_cmark doesn't parse these)
    let wikilink_re = regex::Regex::new(r"\[\[([^\]]+)\]\]").expect("valid regex");
    for mat in wikilink_re.find_iter(content) {
        let line_number = content[..mat.start()].matches('\n').count() + 1;
        let inner = &content[mat.start() + 2..mat.end() - 2];
        let (target, text) = if let Some(pipe_pos) = inner.find('|') {
            (&inner[..pipe_pos], &inner[pipe_pos + 1..])
        } else {
            (inner, inner)
        };
        let target = target.trim();
        let text = text.trim();
        if !target.is_empty()
            && !is_external_or_anchor(target)
            && crate::relations::target_link_kind(target)
                == crate::relations::FrontmatterLinkKind::Relation
        {
            links.push(RawLink {
                target: target.to_string(),
                text: text.to_string(),
                line_number,
                is_wikilink: true,
            });
        }
    }

    // Standard markdown links via pulldown_cmark
    let parser = Parser::new_ext(content, Options::all());
    let mut current_link: Option<(String, usize)> = None; // (target, byte_offset)
    let mut link_text = String::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                let url = dest_url.to_string();
                if !is_external_or_anchor(&url)
                    && crate::relations::target_link_kind(&url)
                        == crate::relations::FrontmatterLinkKind::Relation
                {
                    current_link = Some((url, range.start));
                    link_text.clear();
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some((target, byte_offset)) = current_link.take() {
                    let line_number = content[..byte_offset].matches('\n').count() + 1;
                    links.push(RawLink {
                        target,
                        text: link_text.trim().to_string(),
                        line_number,
                        is_wikilink: false,
                    });
                    link_text.clear();
                }
            }
            Event::Text(text) | Event::Code(text) if current_link.is_some() => {
                link_text.push_str(&text);
            }
            _ => {}
        }
    }

    links
}

/// Context around a link extracted from a markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct LinkContext {
    /// The raw link this context belongs to.
    pub link: RawLink,
    /// The surrounding paragraph text where the link appears.
    pub paragraph: String,
}

/// Extract the paragraph surrounding a link at the given 1-based line number.
///
/// Walks backward and forward from `line_number` until hitting an empty line,
/// a heading line (starting with `# `), or file boundary. Uses FULL file content
/// because `RawLink.line_number` is file-relative (includes frontmatter lines).
pub fn extract_link_paragraph(content: &str, line_number: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if line_number == 0 || line_number > lines.len() {
        return String::new();
    }
    let idx = line_number - 1; // convert to 0-based

    let is_boundary = |line: &str| -> bool { line.trim().is_empty() || line.starts_with('#') };

    // Walk backward
    let mut start = idx;
    while start > 0 {
        let prev = start - 1;
        if is_boundary(lines[prev]) {
            break;
        }
        start = prev;
    }

    // Walk forward
    let mut end = idx;
    while end + 1 < lines.len() {
        let next = end + 1;
        if is_boundary(lines[next]) {
            break;
        }
        end = next;
    }

    let paragraph: String = lines[start..=end].join("\n");
    let trimmed = paragraph.trim();
    if trimmed.is_empty() {
        // Fall back to the single link line
        lines.get(idx).unwrap_or(&"").trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract link contexts for all provided links using the full file content.
pub fn extract_links_with_context(content: &str, links: &[RawLink]) -> Vec<LinkContext> {
    links
        .iter()
        .map(|link| {
            let paragraph = extract_link_paragraph(content, link.line_number);
            LinkContext {
                link: link.clone(),
                paragraph,
            }
        })
        .collect()
}

/// Check if a URL is external (http/https/mailto) or anchor-only (#heading).
pub(crate) fn is_external_or_anchor(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || (url.starts_with('#') && !url.contains('/'))
}

/// Compute a SHA-256 hex digest of the given content.
pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_frontmatter tests ---

    #[test]
    fn extract_frontmatter_basic() {
        let content = "---\ntitle: Hello\ntags:\n  - rust\n---\nBody here";
        let (fm, body) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm["title"], "Hello");
        assert_eq!(fm["tags"][0], "rust");
        assert_eq!(body, "Body here");
    }

    #[test]
    fn extract_frontmatter_none_when_missing() {
        let content = "# Just a heading\nSome text";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn extract_frontmatter_empty() {
        let content = "---\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, "Body");
    }

    #[test]
    fn extract_frontmatter_uses_only_exact_column_zero_delimiters() {
        let block_scalar = concat!(
            "---\n",
            "title: Safe\n",
            "notes: |-\n",
            "  before\n",
            "  ---\n",
            "  after\n",
            "---\n",
            "Body\n",
        );
        let (frontmatter, body) = extract_frontmatter(block_scalar);
        let frontmatter = frontmatter.unwrap();
        assert_eq!(frontmatter["title"], "Safe");
        assert_eq!(frontmatter["notes"], "before\n---\nafter");
        assert_eq!(body, "Body\n");

        for ambiguous in [
            "--- #comment\ntitle: Safe\n---\nBody\n",
            "--- garbage\ntitle: Safe\n---\nBody\n",
            "---   \ntitle: Safe\n---\nBody\n",
            "---\ntitle: Safe\n--- #comment\nBody\n",
            "---\ntitle: Safe\n--- garbage\nBody\n",
            "---\ntitle: Safe\n---   \nBody\n",
        ] {
            let (frontmatter, body) = extract_frontmatter(ambiguous);
            assert!(frontmatter.is_none(), "{ambiguous:?}");
            assert_eq!(body, ambiguous, "{ambiguous:?}");
        }
    }

    #[test]
    fn exact_frontmatter_delimiters_support_bom_and_crlf() {
        let content = "\u{feff}---\r\ntitle: Safe\r\n---\r\nBody\r\n";
        let (frontmatter, body) = extract_frontmatter(content);
        assert_eq!(frontmatter.unwrap()["title"], "Safe");
        assert_eq!(body, "Body\r\n");
    }

    #[test]
    fn extract_frontmatter_missing_closing() {
        let content = "---\ntitle: Oops\nNo closing delimiter";
        let (fm, _body) = extract_frontmatter(content);
        assert!(fm.is_none());
    }

    #[test]
    fn extract_frontmatter_malformed_yaml() {
        let content = "---\n: :\n  - [invalid\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, "Body");
    }

    #[test]
    fn extract_frontmatter_with_numbers() {
        let content = "---\ncount: 42\npi: 3.14\n---\n";
        let (fm, _) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm["count"], 42);
        assert_eq!(fm["pi"].to_string(), "3.14");
    }

    #[test]
    fn extract_frontmatter_preserves_decimal_token() {
        let content = "---\nprice: 0.1000000000000000000000000001\n---\n";
        let (fm, _) = extract_frontmatter(content);
        assert_eq!(
            fm.unwrap()["price"].to_string(),
            "0.1000000000000000000000000001"
        );
    }

    #[test]
    fn extract_frontmatter_preserves_nested_decimal_tokens_in_source_order() {
        let content = concat!(
            "---\n",
            "amounts: [0.1000000000000000000000000001, {tax: 2.5000000000000000000000000001}]\n",
            "quoted: \"0.1000000000000000000000000001\"\n",
            "scientific: 1.234567890123456789e+5\n",
            "float_key: {0.3333333333333333333333333333: 9.876543210987654321}\n",
            "---\n"
        );
        let (fm, _) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(
            fm["amounts"][0].to_string(),
            "0.1000000000000000000000000001"
        );
        assert_eq!(
            fm["amounts"][1]["tax"].to_string(),
            "2.5000000000000000000000000001"
        );
        assert_eq!(
            fm["quoted"],
            serde_json::Value::String("0.1000000000000000000000000001".to_string())
        );
        assert_eq!(fm["scientific"].to_string(), "1.234567890123456789e+5");
        let keyed_value = fm["float_key"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert_eq!(keyed_value.to_string(), "9.876543210987654321");
    }

    #[test]
    fn extract_frontmatter_with_bom() {
        let content = "\u{feff}---\ntitle: BOM\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert_eq!(fm.unwrap()["title"], "BOM");
        assert_eq!(body, "Body");
    }

    // --- extract_headings tests ---

    #[test]
    fn extract_headings_basic() {
        let content = "# Title\n\nSome text\n\n## Section\n\nMore text";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Title");
        assert_eq!(headings[0].line_number, 1);
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].text, "Section");
        assert_eq!(headings[1].line_number, 5);
    }

    #[test]
    fn extract_headings_all_levels() {
        let content = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 6);
        for (i, h) in headings.iter().enumerate() {
            assert_eq!(h.level, (i + 1) as u8);
            assert_eq!(h.line_number, i + 1);
        }
    }

    #[test]
    fn extract_headings_no_headings() {
        let content = "Just some text\nwithout headings";
        let headings = extract_headings(content);
        assert!(headings.is_empty());
    }

    #[test]
    fn extract_headings_with_inline_code() {
        let content = "# Heading with `code`";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "Heading with code");
    }

    #[test]
    fn extract_headings_empty_content() {
        let headings = extract_headings("");
        assert!(headings.is_empty());
    }

    #[test]
    fn extract_headings_after_frontmatter() {
        let content = "---\ntitle: Test\n---\n# First Heading\n\nBody\n\n## Second";
        let (_fm, body) = extract_frontmatter(content);
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "First Heading");
        assert_eq!(headings[1].text, "Second");
    }

    // --- content_hash tests ---

    // --- extract_links tests ---

    #[test]
    fn extract_links_standard_markdown() {
        let content = "Check [this doc](other.md) for details.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other.md");
        assert_eq!(links[0].text, "this doc");
        assert_eq!(links[0].line_number, 1);
        assert!(!links[0].is_wikilink);
    }

    #[test]
    fn extract_links_wikilink() {
        let content = "See [[other-note]] for more.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other-note");
        assert_eq!(links[0].text, "other-note");
        assert!(links[0].is_wikilink);
    }

    #[test]
    fn extract_links_wikilink_with_alias() {
        let content = "See [[path/to/note|display text]] here.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "path/to/note");
        assert_eq!(links[0].text, "display text");
        assert!(links[0].is_wikilink);
    }

    #[test]
    fn extract_links_filters_external() {
        let content =
            "[Google](https://google.com) and [local](notes.md) and [mail](mailto:x@y.com)";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "notes.md");
    }

    #[test]
    fn extract_links_excludes_non_markdown_files_from_graph() {
        let content = "[spec](assets/spec.pdf), [[images/mockup.png]], and [[notes/design.md]]";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "notes/design.md");
    }

    #[test]
    fn extract_links_filters_anchors() {
        let content = "[section](#heading) and [file](other.md#section)";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other.md#section");
    }

    #[test]
    fn extract_links_line_numbers() {
        let content = "Line 1\n[link1](a.md)\nLine 3\n[[b]]";
        let links = extract_links(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].line_number, 4); // wikilinks come first (pre-scan)
        assert_eq!(links[1].line_number, 2);
    }

    #[test]
    fn extract_links_empty_content() {
        let links = extract_links("");
        assert!(links.is_empty());
    }

    #[test]
    fn extract_links_no_links() {
        let links = extract_links("Just plain text without any links.");
        assert!(links.is_empty());
    }

    // --- content_hash tests ---

    #[test]
    fn content_hash_deterministic() {
        let hash1 = compute_content_hash("hello world");
        let hash2 = compute_content_hash("hello world");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn content_hash_length() {
        let hash = compute_content_hash("test content");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn content_hash_hex_chars() {
        let hash = compute_content_hash("test");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn content_hash_content_sensitive() {
        let hash1 = compute_content_hash("content a");
        let hash2 = compute_content_hash("content b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_empty_string() {
        let hash = compute_content_hash("");
        assert_eq!(hash.len(), 64);
    }

    // --- extract_link_paragraph tests ---

    #[test]
    fn extract_link_paragraph_basic() {
        let content = "Some intro text.\n\nThis paragraph has a [link](other.md) in it.\nIt continues on this line.\n\nAnother paragraph.";
        let para = extract_link_paragraph(content, 3);
        assert_eq!(
            para,
            "This paragraph has a [link](other.md) in it.\nIt continues on this line."
        );
    }

    #[test]
    fn extract_link_paragraph_heading_boundary() {
        let content = "# Heading\nSome text with a [link](a.md).\n\nMore text.";
        let para = extract_link_paragraph(content, 2);
        assert_eq!(para, "Some text with a [link](a.md).");
    }

    #[test]
    fn extract_link_paragraph_start_of_file() {
        let content = "First line [link](a.md).\nSecond line.\n\nThird.";
        let para = extract_link_paragraph(content, 1);
        assert_eq!(para, "First line [link](a.md).\nSecond line.");
    }

    #[test]
    fn extract_link_paragraph_end_of_file() {
        let content = "Intro.\n\nLast line [link](a.md).";
        let para = extract_link_paragraph(content, 3);
        assert_eq!(para, "Last line [link](a.md).");
    }

    #[test]
    fn extract_link_paragraph_bare_link() {
        let content = "Before.\n\n[link](a.md)\n\nAfter.";
        let para = extract_link_paragraph(content, 3);
        assert_eq!(para, "[link](a.md)");
    }

    #[test]
    fn extract_link_paragraph_multiple_links_same_paragraph() {
        let content = "Intro.\n\nSee [a](a.md) and [b](b.md) here.\n\nEnd.";
        let para1 = extract_link_paragraph(content, 3);
        let para2 = extract_link_paragraph(content, 3);
        assert_eq!(para1, para2);
        assert!(para1.contains("[a](a.md)"));
        assert!(para1.contains("[b](b.md)"));
    }

    #[test]
    fn extract_link_paragraph_with_frontmatter() {
        // Frontmatter followed by empty line then body — empty line acts as boundary
        let content = "---\ntitle: Test\n---\n\nBody with [link](a.md) here.";
        let para = extract_link_paragraph(content, 5);
        assert_eq!(para, "Body with [link](a.md) here.");
    }

    #[test]
    fn extract_link_paragraph_invalid_line_number() {
        let content = "Some text.";
        assert_eq!(extract_link_paragraph(content, 0), "");
        assert_eq!(extract_link_paragraph(content, 100), "");
    }

    // --- extract_frontmatter_links tests ---

    #[test]
    fn frontmatter_links_wiki_and_alias() {
        let fm = serde_json::json!({
            "client": "[[clients/acme]]",
            "owner": "[[people/jane|Jane]]",
        });
        let mut links = extract_frontmatter_links(Some(&fm));
        links.sort_by(|a, b| a.field.cmp(&b.field));
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].field, "client");
        assert_eq!(links[0].raw, "[[clients/acme]]");
        assert_eq!(links[0].target, "clients/acme");
        assert_eq!(links[0].text, "clients/acme");
        assert!(links[0].is_wikilink);
        assert_eq!(links[1].field, "owner");
        assert_eq!(links[1].target, "people/jane");
        assert_eq!(links[1].text, "Jane");
    }

    #[test]
    fn frontmatter_links_markdown_and_bare() {
        let fm = serde_json::json!({
            "spec": "[Spec](docs/spec.md)",
            "note": "notes/idea.md",
        });
        let mut links = extract_frontmatter_links(Some(&fm));
        links.sort_by(|a, b| a.field.cmp(&b.field));
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].field, "note");
        assert_eq!(links[0].target, "notes/idea.md");
        assert!(!links[0].is_wikilink);
        assert_eq!(links[1].field, "spec");
        assert_eq!(links[1].target, "docs/spec.md");
        assert_eq!(links[1].text, "Spec");
        assert!(!links[1].is_wikilink);
    }

    #[test]
    fn frontmatter_file_links_are_not_document_relations() {
        let fm = serde_json::json!({
            "attachments": ["[[assets/mockup.png]]", "[Spec](documents/spec.pdf)"],
            "client": "[[clients/acme]]",
        });
        let links = extract_frontmatter_links(Some(&fm));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].field, "client");
        assert_eq!(links[0].target, "clients/acme");
    }

    #[test]
    fn frontmatter_links_array_preserves_order_and_skips_non_links() {
        let fm = serde_json::json!({
            "clients": ["[[clients/b]]", "todo", "[[clients/a]]", "[[clients/b]]"],
        });
        let links = extract_frontmatter_links(Some(&fm));
        // Source order preserved, duplicates preserved, non-link element skipped.
        let targets: Vec<&str> = links.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(targets, vec!["clients/b", "clients/a", "clients/b"]);
    }

    #[test]
    fn frontmatter_links_skips_nested_and_non_string() {
        let fm = serde_json::json!({
            "meta": {"inner": "[[clients/acme]]"},
            "count": 42,
            "flag": true,
            "nothing": null,
            "nested_list": [["clients/acme"]],
        });
        assert!(extract_frontmatter_links(Some(&fm)).is_empty());
    }

    #[test]
    fn frontmatter_links_whole_value_strictness() {
        let fm = serde_json::json!({
            "note": "See [[x]] for details",
            "url": "[site](https://example.com)",
        });
        assert!(extract_frontmatter_links(Some(&fm)).is_empty());
    }

    #[test]
    fn frontmatter_links_none_frontmatter() {
        assert!(extract_frontmatter_links(None).is_empty());
        let non_object = serde_json::json!("just a string");
        assert!(extract_frontmatter_links(Some(&non_object)).is_empty());
    }

    #[test]
    fn parse_markdown_file_extracts_frontmatter_links() {
        let dir = tempfile::tempdir().unwrap();
        let content =
            "---\nclient: \"[[clients/acme]]\"\ntitle: Invoice\n---\n# Body\nSee [[other]].";
        std::fs::write(dir.path().join("i1.md"), content).unwrap();
        let md = parse_markdown_file(dir.path(), Path::new("i1.md")).unwrap();
        assert_eq!(md.frontmatter_links.len(), 1);
        assert_eq!(md.frontmatter_links[0].field, "client");
        assert_eq!(md.frontmatter_links[0].target, "clients/acme");
        // Body links stay separate.
        assert_eq!(md.links.len(), 1);
        assert_eq!(md.links[0].target, "other");
    }

    #[test]
    fn unquoted_wikilink_is_yaml_nested_array_not_a_relation() {
        // `client: [[clients/acme]]` without quotes parses as a nested YAML
        // sequence — it must NOT produce a frontmatter link (doctor warns instead).
        let content = "---\nclient: [[clients/acme]]\n---\nBody";
        let (fm, _) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert!(fm["client"].is_array());
        assert!(extract_frontmatter_links(Some(&fm)).is_empty());
    }

    // --- extract_links_with_context tests ---

    #[test]
    fn extract_links_with_context_basic() {
        let content = "Intro.\n\nSee [doc](other.md) for details.\n\nEnd.";
        let links = extract_links(content);
        let contexts = extract_links_with_context(content, &links);
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].link.target, "other.md");
        assert_eq!(contexts[0].paragraph, "See [doc](other.md) for details.");
    }

    #[test]
    fn extract_links_with_context_multiple() {
        let content = "First [a](a.md) link.\n\nSecond [b](b.md) link.";
        let links = extract_links(content);
        let contexts = extract_links_with_context(content, &links);
        assert_eq!(contexts.len(), 2);
    }
}
