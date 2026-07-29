//! Atomic, lossless top-level frontmatter patches for built-in modules.
//!
//! Formula expressions never receive filesystem access. They return a patch,
//! and the module runner applies that patch here after verifying the exact
//! source hash it evaluated.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path};
use std::str::FromStr;

use serde_json::Value as JsonValue;
use yaml_edit::{Document, YamlNode};

use crate::error::{Error, Result};
use crate::parser::{compute_content_hash, parse_markdown_file, MarkdownFile};

#[derive(Debug)]
pub struct WritebackResult {
    pub file: MarkdownFile,
    pub changed: bool,
}

#[derive(Debug)]
struct FrontmatterBounds {
    yaml_start: usize,
    yaml_end: usize,
    body_start: usize,
    newline: &'static str,
}

fn frontmatter_bounds(content: &str, relative_path: &Path) -> Result<Option<FrontmatterBounds>> {
    let bom_len = usize::from(content.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let source = &content[bom_len..];
    if !source.starts_with("---") {
        return Ok(None);
    }

    let opening_end = source.find('\n').ok_or_else(|| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "frontmatter opening delimiter has no closing delimiter".to_string(),
    })? + 1;
    let opening_line = &source[..opening_end];
    if opening_line
        .trim_end_matches(['\r', '\n'])
        .trim()
        != "---"
    {
        return Ok(None);
    }
    let newline = if opening_line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let yaml_start = bom_len + opening_end;
    let mut cursor = yaml_start;
    while cursor < content.len() {
        let remaining = &content[cursor..];
        let line_len = remaining.find('\n').map_or(remaining.len(), |index| index + 1);
        let line = &remaining[..line_len];
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            return Ok(Some(FrontmatterBounds {
                yaml_start,
                yaml_end: cursor,
                body_start: cursor + line_len,
                newline,
            }));
        }
        cursor += line_len;
    }

    Err(Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "frontmatter opening delimiter has no closing delimiter".to_string(),
    })
}

struct EditableMapping {
    document: Document,
    prefix: String,
    suffix: String,
}

fn parse_mapping(source: &str, relative_path: &Path) -> Result<EditableMapping> {
    // Validate semantics with the same YAML implementation used by the Markdown
    // parser, then use yaml-edit's concrete syntax tree for lossless mutation.
    let semantic =
        serde_yaml::from_str::<serde_yaml::Value>(source).map_err(|error| {
            Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: format!("malformed frontmatter: {error}"),
            }
        })?;
    if !semantic.is_mapping() && !semantic.is_null() {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter must be a top-level mapping".to_string(),
        });
    }

    let document = if semantic.is_null() {
        Document::new_mapping()
    } else {
        Document::from_str(source).map_err(|error| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: format!("malformed frontmatter: {error}"),
        })?
    };
    let Some(mapping) = document.as_mapping() else {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter must be a top-level mapping".to_string(),
        });
    };
    let range = mapping.byte_range();
    Ok(EditableMapping {
        document,
        prefix: source[..range.start as usize].to_string(),
        suffix: source[range.end as usize..].to_string(),
    })
}

fn json_value_node(value: &JsonValue, relative_path: &Path) -> Result<YamlNode> {
    let source = serde_json::to_string(value)
        .map_err(|error| Error::Serialization(format!("formula value: {error}")))?;
    let document = Document::from_str(&source).map_err(|error| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: format!("formula value cannot be represented as YAML: {error}"),
    })?;
    if let Some(value) = document.as_scalar() {
        Ok(YamlNode::Scalar(value))
    } else if let Some(value) = document.as_sequence() {
        Ok(YamlNode::Sequence(value))
    } else if let Some(value) = document.as_mapping() {
        Ok(YamlNode::Mapping(value))
    } else {
        Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "formula value has no YAML representation".to_string(),
        })
    }
}

fn normalize_newlines(source: &str, newline: &str) -> String {
    let normalized = source.replace("\r\n", "\n");
    if newline == "\n" {
        normalized
    } else {
        normalized.replace('\n', "\r\n")
    }
}

fn render_patch(
    original: &str,
    relative_path: &Path,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<String> {
    let bounds = frontmatter_bounds(original, relative_path)?;
    if bounds.is_none() && set.is_empty() {
        return Ok(original.to_string());
    }

    let existing_values = crate::parser::extract_frontmatter(original)
        .0
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let (editable, newline) = if let Some(bounds) = &bounds {
        (
            parse_mapping(&original[bounds.yaml_start..bounds.yaml_end], relative_path)?,
            bounds.newline,
        )
    } else {
        (
            EditableMapping {
                document: Document::new_mapping(),
                prefix: String::new(),
                suffix: String::new(),
            },
            if original.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            },
        )
    };

    for field in unset {
        if set.contains_key(field) {
            continue;
        }
        editable.document.remove(field.as_str());
    }
    for (field, value) in set {
        if existing_values.get(field) == Some(value) {
            continue;
        }
        editable
            .document
            .set(field.as_str(), json_value_node(value, relative_path)?);
    }

    if let Some(bounds) = bounds {
        if editable.document.is_empty() {
            let bom = &original[..bounds.yaml_start - (4 + usize::from(bounds.newline == "\r\n"))];
            return Ok(format!("{bom}{}", &original[bounds.body_start..]));
        }

        let mapping = editable
            .document
            .as_mapping()
            .expect("validated mapping remains a mapping");
        let edited_yaml = format!("{}{}{}", editable.prefix, mapping, editable.suffix);
        let yaml = normalize_newlines(
            edited_yaml.trim_end_matches(['\r', '\n']),
            bounds.newline,
        );
        return Ok(format!(
            "{}{}{}{}",
            &original[..bounds.yaml_start],
            yaml,
            bounds.newline,
            &original[bounds.yaml_end..]
        ));
    }

    let bom_len = usize::from(original.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let mapping = editable
        .document
        .as_mapping()
        .expect("new formula frontmatter is a mapping");
    let yaml = normalize_newlines(
        mapping.to_string().trim_end_matches(['\r', '\n']),
        newline,
    );
    Ok(format!(
        "{}---{newline}{yaml}{newline}---{newline}{}",
        &original[..bom_len],
        &original[bom_len..]
    ))
}

/// Apply one module patch atomically after verifying the exact bytes evaluated.
pub fn apply_frontmatter_patch(
    project_root: &Path,
    relative_path: &Path,
    expected_content_hash: &str,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<WritebackResult> {
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Error::Config(format!(
            "module source path must stay inside the project: {}",
            relative_path.display()
        )));
    }

    let full_path = project_root.join(relative_path);
    let bytes = std::fs::read(&full_path)?;
    let original = String::from_utf8(bytes).map_err(|_| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "file is not valid UTF-8".to_string(),
    })?;
    if compute_content_hash(&original) != expected_content_hash {
        return Err(Error::SourceChanged {
            path: relative_path.to_path_buf(),
        });
    }

    let rendered = render_patch(&original, relative_path, set, unset)?;
    if rendered == original {
        return Ok(WritebackResult {
            file: parse_markdown_file(project_root, relative_path)?,
            changed: false,
        });
    }

    let parent = full_path.parent().ok_or_else(|| {
        Error::Config(format!(
            "module source has no parent directory: {}",
            full_path.display()
        ))
    })?;
    let metadata = std::fs::metadata(&full_path)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".mdvdb-formula-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temporary.as_file().set_permissions(metadata.permissions())?;
    temporary.write_all(rendered.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&full_path)
        .map_err(|error| Error::Io(error.error))?;

    // Persist the directory entry as well as the file contents where supported.
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }

    Ok(WritebackResult {
        file: parse_markdown_file(project_root, relative_path)?,
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, value: &str) {
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn writes_exact_json_numbers_and_preserves_unrelated_yaml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invoice.md");
        let original = "---\n# keep me\nprice: 0.10 # original\ntags:\n  - one\ntotal: 0 # formula note\n---\n# Body\n";
        write(&path, original);
        let expected = compute_content_hash(original);
        let exact: JsonValue = serde_json::from_str("0.3000000000000000000000000001").unwrap();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &expected,
            &BTreeMap::from([("total".to_string(), exact)]),
            &BTreeSet::new(),
        )
        .unwrap();

        assert!(result.changed);
        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.contains("# keep me"), "{rewritten:?}");
        assert!(rewritten.contains("price: 0.10 # original"));
        assert!(rewritten.contains("tags:\n  - one"));
        assert!(rewritten.contains("total: 0.3000000000000000000000000001"));
        assert!(rewritten.contains("# formula note"));
        assert!(rewritten.ends_with("---\n# Body\n"));
    }

    #[test]
    fn no_op_does_not_rewrite_and_removal_cleans_last_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("generated.md");
        let original = "---\ntotal: 3\n---\nBody";
        write(&path, original);
        let expected = compute_content_hash(original);
        let value = serde_json::json!(3);

        let no_op = apply_frontmatter_patch(
            dir.path(),
            Path::new("generated.md"),
            &expected,
            &BTreeMap::from([("total".to_string(), value)]),
            &BTreeSet::from(["total".to_string()]),
        )
        .unwrap();
        assert!(!no_op.changed, "{:?}", no_op.file.frontmatter);

        let removed = apply_frontmatter_patch(
            dir.path(),
            Path::new("generated.md"),
            &expected,
            &BTreeMap::new(),
            &BTreeSet::from(["total".to_string()]),
        )
        .unwrap();
        assert!(removed.changed);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Body");
    }

    #[test]
    fn refuses_stale_or_malformed_sources() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join("stale.md"), "value");
        let stale = apply_frontmatter_patch(
            dir.path(),
            Path::new("stale.md"),
            "wrong",
            &BTreeMap::from([("total".to_string(), serde_json::json!(1))]),
            &BTreeSet::new(),
        );
        assert!(matches!(stale, Err(Error::SourceChanged { .. })));

        let malformed = "---\nitems: [\n---\nBody";
        write(&dir.path().join("malformed.md"), malformed);
        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("malformed.md"),
            &compute_content_hash(malformed),
            &BTreeMap::from([("total".to_string(), serde_json::json!(1))]),
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(Error::MarkdownParse { .. })));
    }

    #[test]
    fn preserves_bom_crlf_body_and_writes_nested_values() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested.md");
        let original = "\u{feff}---\r\n# settings\r\nname: Example\r\n---\r\n# Body\r\nunchanged\r\n";
        write(&path, original);
        let nested: JsonValue = serde_json::from_str(
            r#"{"amounts":[0.1000000000000000000000000001,2],"active":true}"#,
        )
        .unwrap();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("nested.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("summary".to_string(), nested.clone())]),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(result.changed);

        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.starts_with("\u{feff}---\r\n# settings\r\n"));
        assert!(rewritten.ends_with("---\r\n# Body\r\nunchanged\r\n"));
        assert!(!rewritten.replace("\r\n", "").contains('\n'));
        assert_eq!(result.file.frontmatter.unwrap()["summary"], nested);
    }
}
