use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use tracing::debug;

use crate::parser::MarkdownFile;

/// The type of a frontmatter field, inferred from values across files.
#[derive(Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub enum FieldType {
    String,
    Number,
    Boolean,
    List,
    Date,
    Mixed,
}

/// Intermediate type used during inference to accumulate field information.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct InferredField {
    /// Field name (frontmatter key).
    pub name: String,
    /// Inferred type from observed values.
    pub field_type: FieldType,
    /// Number of files containing this field.
    pub occurrence_count: usize,
    /// Up to 20 unique stringified sample values.
    pub sample_values: Vec<String>,
}

/// A field definition from the user-provided overlay file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OverlayField {
    /// Human-readable description of the field.
    pub description: Option<String>,
    /// Type override as a string (e.g. "string", "number", "date").
    pub field_type: Option<String>,
    /// List of allowed values for this field.
    pub allowed_values: Option<Vec<String>>,
    /// Whether this field is considered required.
    pub required: Option<bool>,
}

/// Top-level structure for `.markdownvdb.schema.yml`.
#[derive(Debug, serde::Deserialize)]
pub struct OverlaySchema {
    /// Map from field name to overlay configuration.
    pub fields: HashMap<String, OverlayField>,
}

/// A merged schema field combining inferred data with overlay annotations.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct SchemaField {
    /// Field name.
    pub name: String,
    /// Resolved field type (inferred or overridden by overlay).
    pub field_type: FieldType,
    /// Human-readable description from overlay.
    pub description: Option<String>,
    /// Number of files containing this field.
    pub occurrence_count: usize,
    /// Up to 20 unique sample values.
    pub sample_values: Vec<String>,
    /// Allowed values from overlay.
    pub allowed_values: Option<Vec<String>>,
    /// Whether this field is required (from overlay, defaults to false).
    pub required: bool,
}

/// The complete metadata schema, persisted in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct Schema {
    /// Schema fields sorted alphabetically by name.
    pub fields: Vec<SchemaField>,
    /// Unix timestamp of when this schema was generated.
    pub last_updated: u64,
}

/// Check if a string matches YYYY-MM-DD with optional T suffix for datetime.
fn is_date_string(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 10 {
        return false;
    }
    bytes[0..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
        && (bytes.len() == 10 || bytes[10] == b'T')
}

/// Infer a `FieldType` from a single `serde_json::Value`.
fn infer_field_type(value: &serde_json::Value) -> FieldType {
    match value {
        serde_json::Value::Bool(_) => FieldType::Boolean,
        serde_json::Value::Number(_) => FieldType::Number,
        serde_json::Value::String(s) => {
            if is_date_string(s) {
                FieldType::Date
            } else {
                FieldType::String
            }
        }
        serde_json::Value::Array(_) => FieldType::List,
        serde_json::Value::Object(_) => FieldType::String, // treat objects as string
        serde_json::Value::Null => FieldType::String,      // null defaults to string
    }
}

/// Stringify a `serde_json::Value` for sample collection.
fn value_to_sample_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Convert an overlay type string to a `FieldType`.
fn parse_field_type_str(s: &str) -> Option<FieldType> {
    match s.to_lowercase().as_str() {
        "string" => Some(FieldType::String),
        "number" => Some(FieldType::Number),
        "boolean" | "bool" => Some(FieldType::Boolean),
        "list" | "array" => Some(FieldType::List),
        "date" => Some(FieldType::Date),
        "mixed" => Some(FieldType::Mixed),
        _ => None,
    }
}

impl Schema {
    /// Auto-infer a schema from frontmatter across all provided files.
    pub fn infer(files: &[MarkdownFile]) -> Self {
        // Track per-field: types seen, occurrence count, sample values
        let mut field_types: HashMap<String, HashSet<std::mem::Discriminant<FieldType>>> =
            HashMap::new();
        let mut field_type_values: HashMap<String, FieldType> = HashMap::new();
        let mut occurrence_counts: HashMap<String, usize> = HashMap::new();
        let mut sample_values: HashMap<String, HashSet<String>> = HashMap::new();

        for file in files {
            let frontmatter = match &file.frontmatter {
                Some(serde_json::Value::Object(map)) => map,
                _ => continue,
            };

            for (key, value) in frontmatter {
                // Skip null values for type inference
                if value.is_null() {
                    continue;
                }

                let ft = infer_field_type(value);
                let discriminant = std::mem::discriminant(&ft);

                let types = field_types.entry(key.clone()).or_default();
                types.insert(discriminant);

                // Store the latest type; if multiple types seen, it becomes Mixed
                field_type_values.insert(key.clone(), ft);

                *occurrence_counts.entry(key.clone()).or_insert(0) += 1;

                let samples = sample_values.entry(key.clone()).or_default();
                if samples.len() < 20 {
                    samples.insert(value_to_sample_string(value));
                }
            }
        }

        // Build fields sorted alphabetically
        let mut field_names: Vec<String> = field_types.keys().cloned().collect();
        field_names.sort();

        let fields = field_names
            .into_iter()
            .map(|name| {
                let types = &field_types[&name];
                let field_type = if types.len() > 1 {
                    FieldType::Mixed
                } else {
                    field_type_values.get(&name).cloned().unwrap_or(FieldType::String)
                };

                let mut samples: Vec<String> = sample_values
                    .remove(&name)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                samples.sort();

                SchemaField {
                    name: name.clone(),
                    field_type,
                    description: None,
                    occurrence_count: occurrence_counts.get(&name).copied().unwrap_or(0),
                    sample_values: samples,
                    allowed_values: None,
                    required: false,
                }
            })
            .collect();

        let last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        debug!(field_count = field_types.len(), "schema inferred from frontmatter");

        Schema {
            fields,
            last_updated,
        }
    }

    /// Load an optional overlay from `.markdownvdb.schema.yml` in the project root.
    pub fn load_overlay(
        project_root: &Path,
    ) -> crate::Result<Option<HashMap<String, OverlayField>>> {
        let path = project_root.join(".markdownvdb.schema.yml");
        if !path.exists() {
            debug!("no schema overlay file found at {}", path.display());
            return Ok(None);
        }

        let contents = std::fs::read_to_string(&path)?;
        let overlay: OverlaySchema = serde_yaml::from_str(&contents).map_err(|e| {
            crate::error::Error::Config(format!(
                "failed to parse {}: {e}",
                path.display()
            ))
        })?;

        debug!(field_count = overlay.fields.len(), "loaded schema overlay");
        Ok(Some(overlay.fields))
    }

    /// Merge an inferred schema with an optional overlay.
    pub fn merge(inferred: Self, overlay: Option<HashMap<String, OverlayField>>) -> Self {
        let overlay = match overlay {
            Some(o) => o,
            None => return inferred,
        };

        // Start with inferred fields in a BTreeMap for alphabetical ordering
        let mut merged: BTreeMap<String, SchemaField> = BTreeMap::new();
        for field in inferred.fields {
            merged.insert(field.name.clone(), field);
        }

        // Apply overlay
        for (name, ov) in &overlay {
            if let Some(field) = merged.get_mut(name) {
                // Apply overlay to existing inferred field
                if let Some(desc) = &ov.description {
                    field.description = Some(desc.clone());
                }
                if let Some(type_str) = &ov.field_type {
                    if let Some(ft) = parse_field_type_str(type_str) {
                        field.field_type = ft;
                    }
                }
                if let Some(av) = &ov.allowed_values {
                    field.allowed_values = Some(av.clone());
                }
                if let Some(req) = ov.required {
                    field.required = req;
                }
            } else {
                // Overlay-only field: not seen in any file
                let field_type = ov
                    .field_type
                    .as_deref()
                    .and_then(parse_field_type_str)
                    .unwrap_or(FieldType::String);

                merged.insert(
                    name.clone(),
                    SchemaField {
                        name: name.clone(),
                        field_type,
                        description: ov.description.clone(),
                        occurrence_count: 0,
                        sample_values: vec![],
                        allowed_values: ov.allowed_values.clone(),
                        required: ov.required.unwrap_or(false),
                    },
                );
            }
        }

        let fields: Vec<SchemaField> = merged.into_values().collect();

        debug!(field_count = fields.len(), "schema merged with overlay");

        Schema {
            fields,
            last_updated: inferred.last_updated,
        }
    }

    /// Look up a field by name.
    pub fn get_field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Return all field names in the schema.
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_date_string_valid_date() {
        assert!(is_date_string("2024-01-15"));
    }

    #[test]
    fn is_date_string_valid_datetime() {
        assert!(is_date_string("2024-01-15T10:30:00"));
    }

    #[test]
    fn is_date_string_not_a_date() {
        assert!(!is_date_string("not-a-date"));
    }

    #[test]
    fn is_date_string_too_short() {
        assert!(!is_date_string("2024-01"));
    }

    #[test]
    fn parse_field_type_str_variants() {
        assert_eq!(parse_field_type_str("string"), Some(FieldType::String));
        assert_eq!(parse_field_type_str("number"), Some(FieldType::Number));
        assert_eq!(parse_field_type_str("boolean"), Some(FieldType::Boolean));
        assert_eq!(parse_field_type_str("bool"), Some(FieldType::Boolean));
        assert_eq!(parse_field_type_str("list"), Some(FieldType::List));
        assert_eq!(parse_field_type_str("array"), Some(FieldType::List));
        assert_eq!(parse_field_type_str("date"), Some(FieldType::Date));
        assert_eq!(parse_field_type_str("mixed"), Some(FieldType::Mixed));
        assert_eq!(parse_field_type_str("unknown"), None);
    }

    #[test]
    fn field_type_clone_and_eq() {
        let a = FieldType::Date;
        let b = a.clone();
        assert_eq!(a, b);
    }

    fn make_file(frontmatter: serde_json::Value) -> MarkdownFile {
        MarkdownFile {
            path: std::path::PathBuf::from("test.md"),
            frontmatter: Some(frontmatter),
            headings: vec![],
            body: String::new(),
            content_hash: String::new(),
            file_size: 0,
            links: Vec::new(),
            modified_at: 0,
        }
    }

    #[test]
    fn infer_field_type_from_value() {
        assert_eq!(infer_field_type(&serde_json::json!(true)), FieldType::Boolean);
        assert_eq!(infer_field_type(&serde_json::json!(42)), FieldType::Number);
        assert_eq!(infer_field_type(&serde_json::json!("hello")), FieldType::String);
        assert_eq!(infer_field_type(&serde_json::json!("2024-01-15")), FieldType::Date);
        assert_eq!(infer_field_type(&serde_json::json!(["a", "b"])), FieldType::List);
    }

    #[test]
    fn infer_empty_files() {
        let schema = Schema::infer(&[]);
        assert!(schema.fields.is_empty());
    }

    #[test]
    fn infer_basic_fields() {
        let files = vec![
            make_file(serde_json::json!({"title": "Hello", "count": 5})),
            make_file(serde_json::json!({"title": "World", "tags": ["a"]})),
        ];
        let schema = Schema::infer(&files);
        assert_eq!(schema.fields.len(), 3);
        // Alphabetically sorted
        assert_eq!(schema.fields[0].name, "count");
        assert_eq!(schema.fields[0].field_type, FieldType::Number);
        assert_eq!(schema.fields[0].occurrence_count, 1);
        assert_eq!(schema.fields[1].name, "tags");
        assert_eq!(schema.fields[1].field_type, FieldType::List);
        assert_eq!(schema.fields[2].name, "title");
        assert_eq!(schema.fields[2].field_type, FieldType::String);
        assert_eq!(schema.fields[2].occurrence_count, 2);
    }

    #[test]
    fn infer_mixed_types() {
        let files = vec![
            make_file(serde_json::json!({"value": "text"})),
            make_file(serde_json::json!({"value": 42})),
        ];
        let schema = Schema::infer(&files);
        assert_eq!(schema.fields[0].field_type, FieldType::Mixed);
    }

    #[test]
    fn infer_date_field() {
        let files = vec![
            make_file(serde_json::json!({"created": "2024-01-15"})),
            make_file(serde_json::json!({"created": "2024-06-01T10:00:00"})),
        ];
        let schema = Schema::infer(&files);
        assert_eq!(schema.fields[0].field_type, FieldType::Date);
    }

    #[test]
    fn infer_sample_values_capped() {
        let files: Vec<MarkdownFile> = (0..30)
            .map(|i| make_file(serde_json::json!({"tag": format!("val-{i}")})))
            .collect();
        let schema = Schema::infer(&files);
        assert!(schema.fields[0].sample_values.len() <= 20);
    }

    #[test]
    fn infer_skips_no_frontmatter() {
        let mut file = make_file(serde_json::json!(null));
        file.frontmatter = None;
        let schema = Schema::infer(&[file]);
        assert!(schema.fields.is_empty());
    }

    #[test]
    fn infer_sample_values_deduplicated() {
        let files = vec![
            make_file(serde_json::json!({"status": "draft"})),
            make_file(serde_json::json!({"status": "draft"})),
            make_file(serde_json::json!({"status": "published"})),
        ];
        let schema = Schema::infer(&files);
        let field = &schema.fields[0];
        assert_eq!(field.occurrence_count, 3);
        assert_eq!(field.sample_values.len(), 2);
        assert!(field.sample_values.contains(&"draft".to_string()));
        assert!(field.sample_values.contains(&"published".to_string()));
    }

    #[test]
    fn infer_null_values_skipped() {
        let files = vec![
            make_file(serde_json::json!({"title": "Hello", "opt": null})),
        ];
        let schema = Schema::infer(&files);
        // "opt" field has null value, which is skipped entirely
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "title");
    }

    #[test]
    fn infer_nested_objects_as_string() {
        let files = vec![
            make_file(serde_json::json!({"meta": {"nested": "value"}})),
        ];
        let schema = Schema::infer(&files);
        assert_eq!(schema.fields[0].name, "meta");
        assert_eq!(schema.fields[0].field_type, FieldType::String);
    }

    #[test]
    fn load_overlay_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = Schema::load_overlay(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_overlay_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
fields:
  title:
    description: "The document title"
    field_type: string
    required: true
  status:
    allowed_values: ["draft", "published"]
"#;
        std::fs::write(dir.path().join(".markdownvdb.schema.yml"), yaml).unwrap();
        let result = Schema::load_overlay(dir.path()).unwrap().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result["title"].description.as_deref(), Some("The document title"));
        assert_eq!(result["title"].required, Some(true));
        assert!(result["status"].allowed_values.is_some());
    }

    #[test]
    fn load_overlay_invalid_yaml_returns_config_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".markdownvdb.schema.yml"), "not: [valid: yaml: !!").unwrap();
        let err = Schema::load_overlay(dir.path()).unwrap_err();
        assert!(matches!(err, crate::error::Error::Config(_)));
    }

    #[test]
    fn merge_without_overlay_returns_inferred() {
        let files = vec![make_file(serde_json::json!({"title": "Hello"}))];
        let inferred = Schema::infer(&files);
        let merged = Schema::merge(inferred.clone(), None);
        assert_eq!(merged.fields.len(), 1);
        assert_eq!(merged.fields[0].name, "title");
    }

    #[test]
    fn merge_applies_overlay_to_existing_field() {
        let files = vec![make_file(serde_json::json!({"title": "Hello"}))];
        let inferred = Schema::infer(&files);
        let mut overlay = HashMap::new();
        overlay.insert("title".to_string(), OverlayField {
            description: Some("Doc title".to_string()),
            field_type: None,
            allowed_values: None,
            required: Some(true),
        });
        let merged = Schema::merge(inferred, Some(overlay));
        assert_eq!(merged.fields[0].description.as_deref(), Some("Doc title"));
        assert!(merged.fields[0].required);
    }

    #[test]
    fn merge_adds_overlay_only_fields() {
        let files = vec![make_file(serde_json::json!({"title": "Hello"}))];
        let inferred = Schema::infer(&files);
        let mut overlay = HashMap::new();
        overlay.insert("category".to_string(), OverlayField {
            description: Some("Content category".to_string()),
            field_type: Some("string".to_string()),
            allowed_values: Some(vec!["blog".to_string(), "docs".to_string()]),
            required: Some(false),
        });
        let merged = Schema::merge(inferred, Some(overlay));
        assert_eq!(merged.fields.len(), 2);
        // alphabetical: category before title
        assert_eq!(merged.fields[0].name, "category");
        assert_eq!(merged.fields[0].occurrence_count, 0);
        assert_eq!(merged.fields[1].name, "title");
    }

    #[test]
    fn merge_type_override() {
        let files = vec![make_file(serde_json::json!({"count": "42"}))];
        let inferred = Schema::infer(&files);
        assert_eq!(inferred.fields[0].field_type, FieldType::String);
        let mut overlay = HashMap::new();
        overlay.insert("count".to_string(), OverlayField {
            description: None,
            field_type: Some("number".to_string()),
            allowed_values: None,
            required: None,
        });
        let merged = Schema::merge(inferred, Some(overlay));
        assert_eq!(merged.fields[0].field_type, FieldType::Number);
    }

    #[test]
    fn get_field_found_and_not_found() {
        let files = vec![make_file(serde_json::json!({"title": "Hello", "tags": ["a"]}))];
        let schema = Schema::infer(&files);
        assert!(schema.get_field("title").is_some());
        assert!(schema.get_field("nonexistent").is_none());
    }

    #[test]
    fn field_names_returns_all() {
        let files = vec![make_file(serde_json::json!({"b": 1, "a": 2}))];
        let schema = Schema::infer(&files);
        let names = schema.field_names();
        assert_eq!(names, vec!["a", "b"]);
    }
}
