use std::collections::HashMap;
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
    pub fn infer(_files: &[MarkdownFile]) -> Self {
        todo!()
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
}
