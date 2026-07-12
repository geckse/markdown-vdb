use std::collections::{HashMap, HashSet};
use std::path::Path;

use parking_lot::Mutex;
use serde::Serialize;

use crate::schema::{OverlaySchema, Schema};

/// A resolved frontmatter relation value, emitted by the populate surfaces
/// (`get --populate`, `collection --populate`, `search --populate`).
///
/// Contract (phase 31): `frontmatter` is an ALWAYS-present key serialized as
/// `object | null` — `null` when the target is missing or has no frontmatter.
/// A populated target's frontmatter is never itself populated (depth 1).
#[derive(Debug, Clone, Serialize)]
pub struct RelationValue {
    /// The literal frontmatter value (or list element).
    pub raw: String,
    /// Resolved root-relative path; `None` only if unresolvable.
    pub path: Option<String>,
    /// Whether the resolved path is present in the index / known file set.
    pub exists: bool,
    /// Display title derived from the target (phase-29 rule); `None` when `!exists`.
    pub title: Option<String>,
    /// The target's raw frontmatter. No skip attribute — serializes as null per the contract.
    pub frontmatter: Option<serde_json::Value>,
}

/// A reverse relation lookup entry: which document references this one, via which field.
#[derive(Debug, Clone, Serialize)]
pub struct ReferencedBy {
    /// Source document (relative path) holding the relation.
    pub source: String,
    /// Frontmatter field name the relation originates from.
    pub field: String,
    /// Display title of the source document.
    pub title: String,
}

/// A parsed link-shaped frontmatter value.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedLink {
    /// The inner link target (wiki inner before `|`, markdown link target, or the bare path).
    pub target: String,
    /// Display text (wiki alias or markdown link text), falling back to the target.
    pub text: String,
    /// Whether the value was a `[[wikilink]]`.
    pub is_wikilink: bool,
}

/// Context for resolving frontmatter relation targets (graph build + populate).
pub struct RelationContext {
    /// The set of known file paths (relative, forward slashes) used for `exists`.
    pub known_files: HashSet<String>,
    /// The loaded schema overlay, if any (source of per-field `target:` folders).
    pub overlay: Option<OverlaySchema>,
    /// Per-directory cache of field name → overlay-declared target folder (slash-less).
    target_cache: Mutex<HashMap<String, HashMap<String, String>>>,
}

impl RelationContext {
    /// Create a context from a known-file set and an optional overlay.
    pub fn new(known_files: HashSet<String>, overlay: Option<OverlaySchema>) -> Self {
        Self {
            known_files,
            overlay,
            target_cache: Mutex::new(HashMap::new()),
        }
    }

    /// An empty context (no known files, no overlay). Useful in tests.
    pub fn empty() -> Self {
        Self::new(HashSet::new(), None)
    }

    /// Overlay-declared target folder for `(source file, field)`, resolved via
    /// [`Schema::resolve_overlay_for_path`] on the source's directory.
    /// Returned slash-less; per-directory resolution is cached for large vaults.
    pub fn target_for(&self, source: &str, field: &str) -> Option<String> {
        let overlay = self.overlay.as_ref()?;
        let dir = source.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut cache = self.target_cache.lock();
        let targets = cache.entry(dir.to_string()).or_insert_with(|| {
            let prefix = if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            };
            Schema::resolve_overlay_for_path(overlay, Some(&prefix))
                .into_iter()
                .filter_map(|(name, f)| {
                    f.target
                        .map(|t| (name, t.trim_end_matches('/').to_string()))
                })
                .collect()
        });
        targets.get(field).cloned()
    }
}

/// Parse a whole-value link-shaped string into its target/text parts.
///
/// The entire trimmed string must be exactly one of:
/// - a wiki link `[[target]]` / `[[target|alias]]`
/// - a markdown link `[text](target)` whose target is not external or a bare `#anchor`
/// - a bare vault path ending in `.md` with no whitespace
///
/// `"See [[x]] for details"` is NOT link-shaped (that is a body-link concern).
pub fn parse_link_shaped(s: &str) -> Option<ParsedLink> {
    let t = s.trim();

    // Wiki link: [[target]] or [[target|alias]].
    if let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) {
        // Parity with the body extractor regex `\[\[([^\]]+)\]\]`: the inner
        // part must not contain brackets (also rejects "[[a]] x [[b]]").
        if inner.is_empty() || inner.contains('[') || inner.contains(']') {
            return None;
        }
        let (target, text) = match inner.find('|') {
            Some(pos) => (&inner[..pos], &inner[pos + 1..]),
            None => (inner, inner),
        };
        let target = target.trim();
        let text = text.trim();
        if target.is_empty() || crate::parser::is_external_or_anchor(target) {
            return None;
        }
        return Some(ParsedLink {
            target: target.to_string(),
            text: if text.is_empty() { target } else { text }.to_string(),
            is_wikilink: true,
        });
    }

    // Markdown link: [text](target).
    if t.starts_with('[') && t.ends_with(')') {
        let idx = t.find("](")?;
        let text = &t[1..idx];
        let target = &t[idx + 2..t.len() - 1];
        if text.contains(']') || target.contains('(') || target.contains(')') {
            return None;
        }
        let target = target.trim();
        let text = text.trim();
        if target.is_empty() || crate::parser::is_external_or_anchor(target) {
            return None;
        }
        return Some(ParsedLink {
            target: target.to_string(),
            text: if text.is_empty() { target } else { text }.to_string(),
            is_wikilink: false,
        });
    }

    // Bare vault path: no whitespace, ends in `.md`, not external.
    if !t.is_empty()
        && !t.contains(char::is_whitespace)
        && t.ends_with(".md")
        && !crate::parser::is_external_or_anchor(t)
    {
        return Some(ParsedLink {
            target: t.to_string(),
            text: t.to_string(),
            is_wikilink: false,
        });
    }

    None
}

/// Whole-value link-shape predicate (single source of truth — schema inference,
/// parser extraction, and filter normalization all call this).
pub fn is_link_shaped(s: &str) -> bool {
    parse_link_shaped(s).is_some()
}

/// Filter normalization: inner link target, `#fragment` stripped, `\` → `/`,
/// trailing `.md` stripped. `None` if the value is not link-shaped.
pub(crate) fn relation_key(s: &str) -> Option<String> {
    let parsed = parse_link_shaped(s)?;
    let t = parsed
        .target
        .split('#')
        .next()
        .unwrap_or(&parsed.target)
        .replace('\\', "/");
    let t = t.trim().trim_start_matches('/');
    let t = t.strip_suffix(".md").unwrap_or(t);
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Resolve a frontmatter relation target using the phase-31 3-step order
/// (body-link resolution is unchanged and stays source-dir-relative):
///
/// 1. Target contains `/` → root-relative (normalize `.`/`..`, append `.md`);
///    if not known, fall back source-dir-relative; if neither exists, the
///    root-relative candidate is the reported path with `exists: false`.
/// 2. Else, if the field declares a `target` folder → `target/name.md`.
/// 3. Else → source-dir-relative (same as body links).
///
/// `#fragment` is stripped; `\` normalizes to `/`. Returns `(resolved_path, exists)`,
/// or `None` if the target is empty after fragment strip / trim.
pub fn resolve_relation_target(
    source: &str,
    target: &str,
    target_folder: Option<&str>,
    known_files: &HashSet<String>,
) -> Option<(String, bool)> {
    let t = target.trim();
    let t = t.split('#').next().unwrap_or(t);
    let t = t.replace('\\', "/");
    let t = t.trim().trim_start_matches('/');
    if t.is_empty() {
        return None;
    }

    if t.contains('/') {
        // Step 1: root-relative, with a source-dir-relative fallback.
        let normalized = crate::links::normalize_path(Path::new(t));
        let s = normalized.to_string_lossy().replace('\\', "/");
        let root_candidate = if s.ends_with(".md") {
            s
        } else {
            format!("{s}.md")
        };
        if known_files.contains(&root_candidate) {
            return Some((root_candidate, true));
        }
        let source_rel = crate::links::resolve_link(source, t);
        if !source_rel.is_empty() && known_files.contains(&source_rel) {
            return Some((source_rel, true));
        }
        return Some((root_candidate, false));
    }

    if let Some(folder) = target_folder {
        // Step 2: overlay-declared target folder.
        let folder = folder.trim_end_matches('/');
        let candidate = if t.ends_with(".md") {
            format!("{folder}/{t}")
        } else {
            format!("{folder}/{t}.md")
        };
        let exists = known_files.contains(&candidate);
        return Some((candidate, exists));
    }

    // Step 3: source-dir-relative (same as body links).
    let resolved = crate::links::resolve_link(source, t);
    if resolved.is_empty() {
        return None;
    }
    let exists = known_files.contains(&resolved);
    Some((resolved, exists))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{OverlayField, ScopeOverlay};

    fn known(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    // --- link-shape predicate matrix ---

    #[test]
    fn shape_wiki_link() {
        let p = parse_link_shaped("[[clients/acme]]").unwrap();
        assert_eq!(p.target, "clients/acme");
        assert_eq!(p.text, "clients/acme");
        assert!(p.is_wikilink);
    }

    #[test]
    fn shape_wiki_link_with_alias() {
        let p = parse_link_shaped("[[clients/acme|Acme Corp]]").unwrap();
        assert_eq!(p.target, "clients/acme");
        assert_eq!(p.text, "Acme Corp");
        assert!(p.is_wikilink);
    }

    #[test]
    fn shape_markdown_link() {
        let p = parse_link_shaped("[Acme](clients/acme.md)").unwrap();
        assert_eq!(p.target, "clients/acme.md");
        assert_eq!(p.text, "Acme");
        assert!(!p.is_wikilink);
    }

    #[test]
    fn shape_bare_md_path() {
        let p = parse_link_shaped("clients/acme.md").unwrap();
        assert_eq!(p.target, "clients/acme.md");
        assert!(!p.is_wikilink);
    }

    #[test]
    fn shape_whole_value_strictness() {
        // Embedded links are NOT relations.
        assert!(!is_link_shaped("See [[x]] for details"));
        assert!(!is_link_shaped("prefix [[x]]"));
        assert!(!is_link_shaped("[[a]] and [[b]]"));
        assert!(!is_link_shaped("[text](a.md) trailing"));
    }

    #[test]
    fn shape_surrounding_whitespace_ok() {
        assert!(is_link_shaped("  [[clients/acme]]  "));
        assert!(is_link_shaped("\tclients/acme.md"));
    }

    #[test]
    fn shape_rejects_external_and_anchor() {
        assert!(!is_link_shaped("[[https://example.com]]"));
        assert!(!is_link_shaped("[site](https://example.com)"));
        assert!(!is_link_shaped("[mail](mailto:x@y.com)"));
        assert!(!is_link_shaped("[sec](#heading)"));
        assert!(!is_link_shaped("http://example.com/x.md"));
    }

    #[test]
    fn shape_rejects_plain_and_malformed() {
        assert!(!is_link_shaped("acme")); // plain bare string, no .md
        assert!(!is_link_shaped("some plain.md text"));
        assert!(!is_link_shaped("[[]]"));
        assert!(!is_link_shaped("[[ ]]"));
        assert!(!is_link_shaped(""));
        assert!(!is_link_shaped("   "));
        assert!(!is_link_shaped("clients/acme .md"));
    }

    #[test]
    fn shape_bare_path_with_fragment_is_not_link_shaped() {
        // A bare path with a fragment no longer ends in .md.
        assert!(!is_link_shaped("clients/acme.md#top"));
        // But wiki/markdown targets may carry fragments.
        assert!(is_link_shaped("[[clients/acme#top]]"));
    }

    // --- relation_key normalization ---

    #[test]
    fn relation_key_variants() {
        assert_eq!(
            relation_key("[[clients/acme|Acme]]").as_deref(),
            Some("clients/acme")
        );
        assert_eq!(
            relation_key("[[clients/acme#top]]").as_deref(),
            Some("clients/acme")
        );
        assert_eq!(
            relation_key("[Acme](clients/acme.md)").as_deref(),
            Some("clients/acme")
        );
        assert_eq!(
            relation_key("clients/acme.md").as_deref(),
            Some("clients/acme")
        );
        assert_eq!(
            relation_key("[[clients\\acme]]").as_deref(),
            Some("clients/acme")
        );
        assert_eq!(relation_key("plain string"), None);
        assert_eq!(relation_key("[[#top]]"), None);
    }

    // --- resolution-order matrix ---

    #[test]
    fn resolve_root_relative_hit() {
        let kf = known(&["clients/acme.md", "invoices/i1.md"]);
        let r = resolve_relation_target("invoices/i1.md", "clients/acme", None, &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
    }

    #[test]
    fn resolve_root_miss_falls_back_source_dir() {
        // "sub/note" from invoices/i1.md: root-relative "sub/note.md" is unknown,
        // but "invoices/sub/note.md" exists → source-dir fallback wins.
        let kf = known(&["invoices/sub/note.md", "invoices/i1.md"]);
        let r = resolve_relation_target("invoices/i1.md", "sub/note", None, &kf);
        assert_eq!(r, Some(("invoices/sub/note.md".to_string(), true)));
    }

    #[test]
    fn resolve_neither_reports_root_candidate_missing() {
        let kf = known(&["invoices/i1.md"]);
        let r = resolve_relation_target("invoices/i1.md", "clients/ghost", None, &kf);
        assert_eq!(r, Some(("clients/ghost.md".to_string(), false)));
    }

    #[test]
    fn resolve_target_folder() {
        let kf = known(&["clients/acme.md"]);
        let r = resolve_relation_target("invoices/i1.md", "acme", Some("clients"), &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
        // Trailing slash on the folder is normalized away.
        let r = resolve_relation_target("invoices/i1.md", "acme", Some("clients/"), &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
        // Missing target under the folder → exists false.
        let r = resolve_relation_target("invoices/i1.md", "ghost", Some("clients"), &kf);
        assert_eq!(r, Some(("clients/ghost.md".to_string(), false)));
    }

    #[test]
    fn resolve_bare_no_target_folder_is_source_dir_relative() {
        let kf = known(&["invoices/other.md"]);
        let r = resolve_relation_target("invoices/i1.md", "other", None, &kf);
        assert_eq!(r, Some(("invoices/other.md".to_string(), true)));
    }

    #[test]
    fn resolve_fragment_stripped() {
        let kf = known(&["clients/acme.md"]);
        let r = resolve_relation_target("invoices/i1.md", "clients/acme#top", None, &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
    }

    #[test]
    fn resolve_empty_and_fragment_only_are_none() {
        let kf = known(&[]);
        assert_eq!(resolve_relation_target("a.md", "", None, &kf), None);
        assert_eq!(resolve_relation_target("a.md", "   ", None, &kf), None);
        assert_eq!(resolve_relation_target("a.md", "#top", None, &kf), None);
    }

    #[test]
    fn resolve_leading_slash_treated_root_relative() {
        let kf = known(&["clients/acme.md"]);
        let r = resolve_relation_target("invoices/i1.md", "/clients/acme", None, &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
    }

    #[test]
    fn resolve_backslash_normalized() {
        let kf = known(&["clients/acme.md"]);
        let r = resolve_relation_target("invoices/i1.md", "clients\\acme", None, &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
    }

    #[test]
    fn resolve_dot_components_normalized() {
        let kf = known(&["clients/acme.md"]);
        let r = resolve_relation_target("invoices/i1.md", "clients/../clients/./acme", None, &kf);
        assert_eq!(r, Some(("clients/acme.md".to_string(), true)));
    }

    #[test]
    fn resolve_self_reference_returns_source() {
        // resolve does NOT skip self-links — callers (graph build, populate) do.
        let kf = known(&["invoices/i1.md"]);
        let r = resolve_relation_target("invoices/i1.md", "invoices/i1", None, &kf);
        assert_eq!(r, Some(("invoices/i1.md".to_string(), true)));
    }

    // --- RelationContext::target_for ---

    fn overlay_with_scope_target() -> OverlaySchema {
        let mut scope_fields = HashMap::new();
        scope_fields.insert(
            "client".to_string(),
            OverlayField {
                description: None,
                field_type: Some("relation".to_string()),
                allowed_values: None,
                required: None,
                target: Some("clients/".to_string()),
            },
        );
        let mut scopes = HashMap::new();
        scopes.insert(
            "invoices/".to_string(),
            ScopeOverlay {
                fields: scope_fields,
            },
        );
        OverlaySchema {
            fields: HashMap::new(),
            scopes,
        }
    }

    #[test]
    fn target_for_resolves_scoped_overlay_slashless() {
        let ctx = RelationContext::new(HashSet::new(), Some(overlay_with_scope_target()));
        assert_eq!(
            ctx.target_for("invoices/i1.md", "client").as_deref(),
            Some("clients")
        );
        // Field without a target, or file outside the scope → None.
        assert_eq!(ctx.target_for("invoices/i1.md", "other"), None);
        assert_eq!(ctx.target_for("notes/n1.md", "client"), None);
        // Root-level files see global fields only.
        assert_eq!(ctx.target_for("root.md", "client"), None);
    }

    #[test]
    fn target_for_without_overlay_is_none() {
        let ctx = RelationContext::empty();
        assert_eq!(ctx.target_for("invoices/i1.md", "client"), None);
    }
}
