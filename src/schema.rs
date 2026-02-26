use std::collections::{HashMap, HashSet};
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
        _project_root: &Path,
    ) -> crate::Result<Option<HashMap<String, OverlayField>>> {
        todo!()
    }

    /// Merge an inferred schema with an optional overlay.
    pub fn merge(_inferred: Self, _overlay: Option<HashMap<String, OverlayField>>) -> Self {
        todo!()
    }

    /// Look up a field by name.
    pub fn get_field(&self, _name: &str) -> Option<&SchemaField> {
        todo!()
    }

    /// Return all field names in the schema.
    pub fn field_names(&self) -> Vec<&str> {
        todo!()
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
}
