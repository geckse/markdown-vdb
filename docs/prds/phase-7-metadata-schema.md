# PRD: Phase 7 — Metadata Schema System

## Overview

Implement the infer-then-overlay schema system that automatically discovers metadata field names, types, and observed values from frontmatter during ingestion, then optionally merges with a user-provided `.markdownvdb.schema.yml` overlay file. The merged schema is persisted in the index so agents can introspect what filterable fields exist before searching.

## Problem Statement

When an agent queries the system, it doesn't know what metadata fields are available for filtering. Without a schema, the agent must either guess field names (error-prone) or search without filters (less precise). The schema system solves this by auto-discovering what fields exist across all indexed files and what types/values they have, while allowing users to annotate or constrain the inferred schema with an overlay file.

## Goals

- Auto-infer schema from frontmatter during ingestion: discover field names, infer types (string, number, boolean, list, date), collect observed values
- Support user overlay via `.markdownvdb.schema.yml` that can annotate, override, or extend the inferred schema
- Merged schema (inferred + overlay) persisted in the index file
- Agents can query the schema to discover filterable fields, types, and example values
- Schema is documentation, not enforcement — files missing fields are still indexed
- Schema updates automatically when new files with new fields are ingested

## Non-Goals

- No strict schema enforcement (files with missing or wrong-typed fields are not rejected)
- No schema migration (changing types doesn't require re-indexing)
- No nested field support (only top-level frontmatter keys)
- No schema validation of values during ingest (only type inference)

## Technical Design

### Data Model Changes

**`InferredField` struct** — auto-discovered field metadata:

```rust
#[derive(Archive, Serialize, Deserialize, Clone)]
pub struct InferredField {
    /// Field name as it appears in frontmatter
    pub name: String,
    /// Inferred type
    pub field_type: FieldType,
    /// Number of files that have this field
    pub occurrence_count: usize,
    /// Sample of observed values (up to 20 unique values)
    pub sample_values: Vec<String>,
}

#[derive(Archive, Serialize, Deserialize, Clone)]
pub enum FieldType {
    String,
    Number,
    Boolean,
    List,
    Date,
    Mixed, // multiple types observed for the same field
}
```

**`OverlayField` struct** — user-provided schema annotations:

```rust
#[derive(serde::Deserialize)]
pub struct OverlayField {
    /// Human-readable description of the field
    pub description: Option<String>,
    /// Override the inferred type
    pub field_type: Option<FieldType>,
    /// Constrain allowed values (advisory, not enforced)
    pub allowed_values: Option<Vec<String>>,
    /// Whether the field is expected on all files (advisory)
    pub required: Option<bool>,
}
```

**`SchemaField` struct** — merged result:

```rust
#[derive(Archive, Serialize, Deserialize, Clone, serde::Serialize)]
pub struct SchemaField {
    pub name: String,
    pub field_type: FieldType,
    pub description: Option<String>,
    pub occurrence_count: usize,
    pub sample_values: Vec<String>,
    pub allowed_values: Option<Vec<String>>,
    pub required: bool,
}
```

**`Schema` struct** — the full schema:

```rust
#[derive(Archive, Serialize, Deserialize, Clone, serde::Serialize)]
pub struct Schema {
    pub fields: Vec<SchemaField>,
    /// When the schema was last updated
    pub last_updated: u64,
}
```

### Overlay File Format

`.markdownvdb.schema.yml`:

```yaml
fields:
  title:
    description: "Document title"
    field_type: string
    required: true
  tags:
    description: "Categorization tags"
    field_type: list
    allowed_values: ["rust", "python", "javascript", "devops", "tutorial"]
  status:
    description: "Document lifecycle status"
    field_type: string
    allowed_values: ["draft", "review", "published", "archived"]
  priority:
    description: "Priority level (1-5)"
    field_type: number
  custom_field:
    description: "A field that doesn't exist yet but will be used"
    field_type: string
```

### Interface Changes

```rust
impl Schema {
    /// Infer schema from a collection of parsed markdown files
    pub fn infer(files: &[MarkdownFile]) -> Self;

    /// Load overlay from .markdownvdb.schema.yml if it exists
    pub fn load_overlay(project_root: &Path) -> Result<Option<HashMap<String, OverlayField>>>;

    /// Merge inferred schema with overlay, returning the combined schema
    pub fn merge(inferred: Self, overlay: Option<HashMap<String, OverlayField>>) -> Self;

    /// Get a specific field by name
    pub fn get_field(&self, name: &str) -> Option<&SchemaField>;

    /// List all field names
    pub fn field_names(&self) -> Vec<&str>;
}
```

**Extend `IndexMetadata`** (from Phase 5):

```rust
// Add to IndexMetadata struct:
pub schema: Schema,
```

### Type Inference Rules

For each frontmatter field across all files:

| YAML Value | Inferred Type |
|---|---|
| `"hello"`, `"2024-01-01"` (non-date string) | `String` |
| `42`, `3.14`, `-1` | `Number` |
| `true`, `false` | `Boolean` |
| `[a, b, c]` | `List` |
| `2024-01-15`, `2024-01-15T10:30:00` | `Date` |
| Multiple types for same field across files | `Mixed` |

Date detection: if a string matches `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS` pattern, infer as `Date`.

### Migration Strategy

The `Schema` field is added to `IndexMetadata`. Existing indexes without a schema will have an empty schema populated on next ingest. No data migration required.

## Implementation Steps

1. **Create `src/schema.rs`** — Implement the schema module:
   - Define `InferredField`, `FieldType`, `OverlayField`, `SchemaField`, `Schema` structs
   - Derive `rkyv::Archive`, `rkyv::Serialize`, `rkyv::Deserialize` on types stored in the index
   - Derive `serde::Serialize` on types returned via the API
   - Derive `serde::Deserialize` on `OverlayField` for YAML parsing

2. **Implement `Schema::infer(files)`:**
   - Iterate all files, for each file's frontmatter (if present):
     - Iterate top-level keys in the `serde_json::Value` object
     - For each key, determine the type from the JSON value:
       - `Value::String(s)` → check if `s` matches date regex `^\d{4}-\d{2}-\d{2}` → `Date` or `String`
       - `Value::Number(_)` → `Number`
       - `Value::Bool(_)` → `Boolean`
       - `Value::Array(_)` → `List`
       - `Value::Object(_)` → `String` (serialize to string, don't recurse)
       - `Value::Null` → skip
     - Track: occurrence count per field, set of observed types, sample values (dedup, cap at 20)
   - If a field has multiple types across files → `FieldType::Mixed`
   - Build `Vec<InferredField>`, sorted by field name

3. **Implement `Schema::load_overlay(project_root)`:**
   - Check for `.markdownvdb.schema.yml` in project root
   - If not found, return `Ok(None)`
   - If found, read and parse with `serde_yaml::from_str::<OverlaySchema>()` where `OverlaySchema` has `fields: HashMap<String, OverlayField>`
   - On parse error, return `Error::Config` with the YAML error message

4. **Implement `Schema::merge(inferred, overlay)`:**
   - Start with all inferred fields
   - For each field in the overlay:
     - If field exists in inferred: apply overlay's `description`, `field_type` (override), `allowed_values`, `required`
     - If field doesn't exist in inferred: add it as a new `SchemaField` with `occurrence_count: 0` and empty `sample_values`
   - Sort merged fields by name
   - Return new `Schema`

5. **Extend `IndexMetadata`** — In `src/index/types.rs`:
   - Add `pub schema: Schema` field to `IndexMetadata`
   - Update `Index::upsert()` to accept an optional `Schema` and store it
   - Add `Index::get_schema(&self) -> Result<Schema>` method

6. **Update `src/lib.rs`** — Add `pub mod schema;`

7. **Write inference tests** — In `src/schema.rs` `#[cfg(test)] mod tests`:
   - Test: single file with `title: "Hello"` infers `title` as `String`
   - Test: single file with `count: 42` infers `count` as `Number`
   - Test: single file with `draft: true` infers `draft` as `Boolean`
   - Test: single file with `tags: [a, b]` infers `tags` as `List`
   - Test: single file with `date: 2024-01-15` infers `date` as `Date`
   - Test: field with `String` in file A and `Number` in file B infers `Mixed`
   - Test: occurrence count is correct across multiple files
   - Test: sample values are deduplicated and capped at 20
   - Test: files without frontmatter are skipped (no error)
   - Test: empty file list produces empty schema

8. **Write overlay tests:**
   - Test: overlay adds description to inferred field
   - Test: overlay overrides field_type from `String` to `Date`
   - Test: overlay adds a new field not present in inferred
   - Test: overlay sets `required: true`
   - Test: overlay sets `allowed_values`
   - Test: missing overlay file returns `None` (not error)
   - Test: invalid YAML in overlay file returns `Error::Config`

9. **Write merge tests:**
   - Test: merge with no overlay returns inferred schema unchanged
   - Test: merge applies overlay description to existing field
   - Test: merge adds overlay-only fields with occurrence_count 0
   - Test: merged fields are sorted by name

## Validation Criteria

- [ ] `Schema::infer()` correctly identifies String, Number, Boolean, List, and Date types from frontmatter
- [ ] Date strings matching `YYYY-MM-DD` are inferred as `Date`, not `String`
- [ ] A field with different types across files is inferred as `Mixed`
- [ ] `occurrence_count` accurately reflects how many files have each field
- [ ] `sample_values` contains up to 20 unique values per field
- [ ] `.markdownvdb.schema.yml` overlay is loaded and merged correctly
- [ ] Overlay can add description, override type, set allowed_values, and mark required
- [ ] Overlay can define fields that don't exist in any file (occurrence_count: 0)
- [ ] Missing overlay file is handled gracefully (no error)
- [ ] Invalid overlay YAML returns a descriptive `Error::Config`
- [ ] Schema is persisted in the index and recoverable after reopen
- [ ] `Index::get_schema()` returns the stored schema
- [ ] Schema fields are sorted alphabetically by name
- [ ] `cargo test` passes all schema tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT enforce schema constraints during ingest** — The schema is documentation, not validation. Files with missing or wrong-typed fields must still be indexed normally. Agents use the schema to build queries; the system never rejects files.
- **Do NOT recurse into nested YAML objects** — Only infer types for top-level frontmatter keys. Nested objects are serialized to string. Deep schema inference adds complexity without clear value.
- **Do NOT store all observed values** — Cap `sample_values` at 20 unique values per field. Fields like `title` would have thousands of unique values; storing all of them bloats the schema.
- **Do NOT re-infer schema on every search** — Schema inference runs during ingest and the result is stored in the index. Search reads the stored schema.
- **Do NOT require the overlay file** — The system must work without `.markdownvdb.schema.yml`. Auto-inference provides a baseline; the overlay is optional enhancement.

## Patterns to Follow

- **Infer + overlay pattern:** Auto-discover from data, optionally refine with user input — this is the pattern specified in PROJECT.md §9
- **Serialization:** Types stored in index derive `rkyv` traits; types returned via API derive `serde::Serialize`; overlay types derive `serde::Deserialize`
- **Error handling:** Parse errors in the overlay file produce `Error::Config` with context; inference never errors (just produces best-effort results)
- **Module structure:** Single `src/schema.rs` file since the schema system is cohesive and not large enough to warrant submodules
