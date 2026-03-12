# PRD: Phase 23 — Path-Scoped Schema

## Overview

Extend the schema system to support path-scoped schemas — separate schema inferences per directory subtree — alongside the existing global schema. Scopes are auto-discovered from top-level directories and optionally refined via `.markdownvdb.schema.yml`. Scoped schemas are persisted in the index during ingest. The CLI gains a `--path` flag on `mdvdb schema`.

## Problem Statement

The current schema system infers one global schema across all files in a vault. Vaults with distinct content types in different folders (e.g., `blog/` with `status`, `category`, `date` fields vs `persons/` with `name`, `role`, `company` fields) produce a noisy merged schema where every field from every folder appears. When an agent or UI queries the schema to discover available fields for a directory, it gets irrelevant fields from unrelated directories mixed in, making suggestions unhelpful and filtering imprecise.

## Goals

- Auto-discover scopes from top-level directories (every first-level subdirectory gets its own scope)
- Support explicit scope configuration in `.markdownvdb.schema.yml` with `scopes:` section
- Union resolution: scoped queries return global fields + all matching scope fields, with more specific scopes overriding broader ones
- Persist scoped schemas in the index during ingest (computed alongside global schema)
- CLI: `mdvdb schema --path blog/` returns blog-specific schema with scope header
- Library API: `MarkdownVdb::schema_scoped(path_prefix)` for programmatic access
- Bump index version (v1 → v2), requiring `mdvdb ingest --full` for existing indexes

## Non-Goals

- No strict schema enforcement — schema remains advisory, not validation
- No nested frontmatter field support (only top-level keys)
- No schema enforcement during ingest (files missing fields are still indexed)
- No per-file schema (scoping is directory-level, not per-document)
- No schema diffing or change detection across ingests
- No recursive auto-discovery (only top-level directories auto-scope; deeper nesting requires overlay config)
- No app/UI changes (covered by a separate app PRD)

## Technical Design

### Scope Discovery & Resolution

**Auto-discovery:** During ingest, after file discovery, extract the set of unique first-level subdirectories from all discovered file paths. Each becomes an automatic scope. A file at `blog/2024/post.md` contributes the scope `blog/`. A file at `README.md` (root-level) does not create a scope.

**Overlay scopes:** Users can define scopes in `.markdownvdb.schema.yml` with field-level annotations (descriptions, allowed_values, required). These merge with auto-discovered scopes.

**Resolution order:** When querying with `--path blog/2024/`:

1. Start with global overlay fields (from `fields:` section)
2. Layer on all matching scopes, sorted by prefix length (shortest first): `blog/` matches, then `blog/2024/` if it exists
3. More specific scope fields override less specific ones
4. Inferred fields from files under the path are combined with resolved overlay fields

### Data Model Changes

**`ScopeOverlay` struct** — a scope's field overlay:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ScopeOverlay {
    pub fields: HashMap<String, OverlayField>,
}
```

**`OverlaySchema` struct** — extended with scopes:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct OverlaySchema {
    #[serde(default)]
    pub fields: HashMap<String, OverlayField>,
    #[serde(default)]
    pub scopes: HashMap<String, ScopeOverlay>,
}
```

**`ScopedSchema` struct** — a schema tagged with its scope:

```rust
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct ScopedSchema {
    /// Path prefix for this scope (e.g. "blog/").
    pub scope: String,
    /// The schema for files under this scope.
    pub schema: Schema,
}
```

**`IndexMetadata`** extension:

```rust
// Existing field (unchanged):
pub schema: Option<Schema>,
// New field:
pub scoped_schemas: Option<Vec<ScopedSchema>>,
```

**Index version bump:** `VERSION` in `src/index/storage.rs` changes from `1` to `2`. Old v1 indexes produce a clear error directing users to run `mdvdb ingest --full`.

### Overlay File Format Extension

```yaml
# .markdownvdb.schema.yml
fields:                         # Global fields (existing, unchanged)
  title:
    description: "Document title"
    required: true

scopes:                         # NEW — path-scoped field definitions
  blog/:
    fields:
      status:
        description: "Publication status"
        allowed_values: ["draft", "review", "published", "archived"]
        required: true
      category:
        allowed_values: ["tech", "personal", "tutorial"]
      date:
        field_type: date
        required: true
  persons/:
    fields:
      role:
        field_type: string
        description: "Job title or role"
      company:
        field_type: string
      linkedin:
        field_type: string
        description: "LinkedIn profile URL"
```

### Interface Changes

**`src/schema.rs` — new and modified methods:**

```rust
impl Schema {
    /// Load overlay, now returning the full OverlaySchema (with scopes).
    /// Changed return type from Option<HashMap<String, OverlayField>>.
    pub fn load_overlay(project_root: &Path) -> Result<Option<OverlaySchema>>;

    /// Resolve overlay fields for a given path prefix.
    /// Returns global fields merged with all matching scope fields.
    pub fn resolve_overlay_for_path(
        overlay: &OverlaySchema,
        path_prefix: Option<&str>,
    ) -> HashMap<String, OverlayField>;

    /// Infer schema from files, optionally filtered by path prefix.
    pub fn infer_scoped(files: &[MarkdownFile], path_prefix: Option<&str>) -> Self;

    /// Discover unique top-level directory scopes from file paths.
    pub fn discover_scopes(files: &[MarkdownFile]) -> Vec<String>;
}
```

**`src/lib.rs` — new method:**

```rust
impl MarkdownVdb {
    /// Return the schema scoped to a path prefix.
    /// Checks persisted scoped schemas first, falls back to on-the-fly computation.
    pub fn schema_scoped(&self, path_prefix: &str) -> Result<Schema>;
}
```

### New Commands / API

**CLI — `mdvdb schema --path <PREFIX>`:**

```
mdvdb schema                    # Global schema (unchanged)
mdvdb schema --path blog/       # Schema scoped to blog/
mdvdb schema --path blog/ --json # JSON output with scoped fields
```

Human-readable output includes a `Scope: blog/` header line when `--path` is provided.

### Migration Strategy

- **Index version bump:** `VERSION` changes from `1` to `2` in `src/index/storage.rs`
- **Old index detection:** When opening a v1 index, return `Error::IndexVersionMismatch` with message: "Index version 1 is outdated. Run `mdvdb ingest --full` to rebuild."
- **No automatic migration:** Users must run `mdvdb ingest --full` once. This is acceptable because ingest rebuilds everything from source markdown files — no data loss.
- **Overlay backward compatibility:** Existing `.markdownvdb.schema.yml` files without `scopes:` continue to work unchanged due to `#[serde(default)]`.

## Implementation Steps

1. **Extend overlay types** — `src/schema.rs`: Add `ScopeOverlay` struct. Add `#[serde(default)] pub scopes: HashMap<String, ScopeOverlay>` to `OverlaySchema`. Change `load_overlay` return type from `Option<HashMap<String, OverlayField>>` to `Option<OverlaySchema>`. Update all callers.

2. **Add scope resolution** — `src/schema.rs`: Implement `Schema::resolve_overlay_for_path(overlay, path_prefix)`. Start with global fields, layer matching scopes sorted by prefix length (shortest first), more specific overrides less specific. Implement `Schema::discover_scopes(files)` that extracts unique first path components from file paths.

3. **Add scoped inference** — `src/schema.rs`: Refactor `Schema::infer()` to extract core logic into `infer_from_iter(impl Iterator<Item = &MarkdownFile>)`. Add `Schema::infer_scoped(files, path_prefix)` that filters files by prefix then delegates to `infer_from_iter`.

4. **Add `ScopedSchema` type** — `src/schema.rs`: Define `ScopedSchema { scope: String, schema: Schema }` with rkyv + serde derives. Add to `IndexMetadata` in `src/index/types.rs` as `pub scoped_schemas: Option<Vec<ScopedSchema>>`.

5. **Bump index version** — `src/index/storage.rs`: Change `VERSION` from `1` to `2`. Add version check in index loading that returns `Error::IndexVersionMismatch` for v1 indexes. Add the error variant to `src/error.rs`.

6. **Add `Index` methods for scoped schemas** — `src/index/state.rs`: Add `get_scoped_schemas() -> Option<Vec<ScopedSchema>>`, `get_scoped_schema(prefix: &str) -> Option<Schema>`, and `set_scoped_schemas(schemas: Option<Vec<ScopedSchema>>)`.

7. **Integrate into ingest pipeline** — `src/lib.rs` in `ingest()`: After computing the global schema, call `Schema::discover_scopes(&parsed_files)` to get auto-discovered scopes. For each scope (auto-discovered + overlay-defined), compute `Schema::infer_scoped(&parsed, Some(scope))`, resolve overlay via `resolve_overlay_for_path`, merge, and collect into `Vec<ScopedSchema>`. Store via `index.set_scoped_schemas()`.

8. **Add `schema_scoped` API method** — `src/lib.rs`: Implement `MarkdownVdb::schema_scoped(path_prefix)`. First check `index.get_scoped_schema(prefix)`. If not found, fall back to on-the-fly computation (discover + parse + infer_scoped + overlay merge).

9. **Update existing `schema()` method** — `src/lib.rs`: When inferring on-the-fly (no stored schema), also apply overlay via `resolve_overlay_for_path(overlay, None)` before returning. Currently it returns raw inference without overlay.

10. **Update watcher** — `src/watcher.rs`: Update `load_overlay` call site to work with new `OverlaySchema` return type. Use `resolve_overlay_for_path(overlay, None)` for global schema in watcher context.

11. **CLI changes** — `src/main.rs`: Add `#[arg(long)] path: Option<String>` to `SchemaArgs`. In handler, dispatch to `vdb.schema_scoped(prefix)` when `--path` is provided. In human-readable output, print `Scope: <prefix>` header line when scoped. Add `--path` to shell completion definitions.

12. **Update `format.rs`** — `src/format.rs`: If a scope-header display function exists for schema output, add scope prefix. Otherwise add it to the schema printing logic in `main.rs`.

13. **Re-exports** — `src/lib.rs`: Add `ScopedSchema` to the public re-exports if needed by external consumers.

14. **Unit tests** — `src/schema.rs` `#[cfg(test)]` block:
    - `test_overlay_with_scopes_parses` — YAML with `fields` + `scopes` deserializes correctly
    - `test_overlay_backward_compat` — YAML without `scopes` still works
    - `test_resolve_no_prefix_returns_global_only`
    - `test_resolve_matching_scope` — global + scope fields returned
    - `test_resolve_scope_overrides_global` — scope field overrides same-named global
    - `test_resolve_nested_path_matches_parent_scope` — `blog/2024/` matches `blog/`
    - `test_resolve_multiple_scopes_union` — `blog/2024/` matches both `blog/` and `blog/2024/`
    - `test_resolve_no_matching_scope` — non-matching prefix returns only global
    - `test_infer_scoped_filters_by_path`
    - `test_infer_scoped_none_equivalent_to_infer`
    - `test_discover_scopes` — returns unique top-level dirs
    - `test_discover_scopes_root_files_excluded` — root-level files don't create scopes

15. **Integration tests** — `tests/schema_test.rs`:
    - `test_schema_scoped_api` — `vdb.schema_scoped("blog/")` returns blog-relevant fields
    - `test_schema_scoped_persisted_after_ingest` — scoped schemas available without re-parsing
    - `test_schema_cli_with_path_flag` — CLI `schema --path blog/ --json` returns scoped output

## Validation Criteria

- [ ] `cargo test` passes — all existing tests plus new scoped schema unit + integration tests
- [ ] `cargo clippy --all-targets` passes with zero warnings
- [ ] Opening a v1 index produces clear error directing user to `mdvdb ingest --full`
- [ ] After `mdvdb ingest --full`, scoped schemas are persisted in the index
- [ ] `mdvdb schema` returns global schema (unchanged behavior)
- [ ] `mdvdb schema --path blog/` returns only blog-relevant fields with `Scope: blog/` header
- [ ] `mdvdb schema --path blog/ --json` returns JSON with scoped fields
- [ ] Auto-discovered scopes: every top-level subdirectory gets its own scope
- [ ] Overlay `scopes:` section merges with auto-discovered scopes
- [ ] Scope resolution: `blog/2024/` matches both `blog/` scope and `blog/2024/` scope (union)
- [ ] More specific scope fields override less specific ones
- [ ] Overlay files without `scopes:` section still parse correctly (backward compatible)
- [ ] Watcher still updates global schema correctly with new overlay return type

## Anti-Patterns to Avoid

- **Do NOT enforce schema constraints** — Schema remains advisory. Files with missing or wrong-typed fields must still be indexed. The schema powers discovery, not validation.

- **Do NOT store all observed values in scoped schemas** — Cap `sample_values` at 20 per field per scope, same as global. Fields like `title` would explode the index size.

- **Do NOT auto-discover scopes recursively** — Only top-level directories get auto-scoped. `blog/2024/` does NOT get its own auto-scope unless explicitly configured in the overlay. Recursive scoping creates too many tiny scopes.

- **Do NOT block ingest on overlay parsing errors** — If `.markdownvdb.schema.yml` has invalid YAML, log a warning and continue with inference-only schema. Never fail ingest because of overlay issues.

- **Do NOT change the existing `Schema` struct shape** — The global schema JSON output must remain backward compatible. The `ScopedSchema` wrapper is additive.

- **Do NOT use `#[serde(flatten)]` on overlay types** — This can cause ambiguous deserialization. Keep `fields` and `scopes` as explicit named fields.

## Patterns to Follow

- **`--path` flag pattern** — Mirror `SearchArgs::path` in `src/main.rs` (line 427) and `graph_data(path_filter)` in `src/lib.rs` (line 1252). Same `#[arg(long)] path: Option<String>` pattern, same `starts_with` prefix matching.

- **Schema inference pattern** — `Schema::infer()` in `src/schema.rs` (line 140) is the template. `infer_scoped` should refactor the core loop into `infer_from_iter` and filter before iterating.

- **Overlay merge pattern** — `Schema::merge()` in `src/schema.rs` (line 247) shows how to combine inferred + overlay with BTreeMap ordering. Scope resolution follows the same merge logic but resolves which overlay fields apply first.

- **Index metadata extension** — `IndexMetadata` in `src/index/types.rs` (line 65) already has `Option<Schema>`, `Option<ClusterState>`, `Option<LinkGraph>`. Adding `Option<Vec<ScopedSchema>>` follows the same optional-field pattern.

- **Watcher schema update** — `src/watcher.rs` (line 318-330) shows the load-overlay → merge → set-schema pattern. Update to use `resolve_overlay_for_path` for the new return type.
