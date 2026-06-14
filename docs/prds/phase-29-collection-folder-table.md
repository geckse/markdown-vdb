# PRD: Phase 29 — Collection (Folder Table View Backend)

## Overview

Add a single read-only command and library method, `collection`, that returns every Markdown document in a folder as table **rows** (with full frontmatter) together with the **column** definitions (from the path-scoped schema, unioned with any frontmatter keys the schema missed) in **one call**. This is the backend for a NocoDB/Airtable-style table view in the Tesseract app (app PRD: `app/docs/prds/phase-39-database-table-view.md`): clicking a folder renders rows = documents, columns = frontmatter fields. The command supports direct-children vs recursive scoping, server-side sorting by a frontmatter field, server-side filtering (reusing the existing `MetadataFilter` enum), and pagination with a total count. It is strictly read-only — mdvdb never writes Markdown files; cell editing is the app's responsibility.

This PRD and the app PRD share **one canonical JSON contract** (below). The app mirrors these exact field names and types in `types/cli.ts`. Any divergence breaks the integration, so the contract section is authoritative.

## Problem Statement

The app currently has to assemble a folder table from multiple mdvdb calls: `file_tree()` to enumerate files and their sync state, then one `get_document(path)` per file to fetch frontmatter, then `schema_scoped(path)` for columns. That is an N+1 pattern (one tree call + one CLI process spawn per row), and the app spawns the CLI per call (`app/src/main/cli.ts`), so a 200-file folder means ~200 process launches. There is also no server-side sort/filter/pagination: the app would have to pull every document and sort/filter in the renderer, which does not scale and duplicates logic that already exists in `src/search.rs` (`MetadataFilter` evaluation). Finally, the index does not store a document title, so each consumer reinvents title derivation. A dedicated `collection` command consolidates all of this into one process invocation that returns rows + columns + total count, and makes title derivation a single server-side source of truth.

## Canonical JSON Contract

`mdvdb collection <PATH> --json` returns exactly this shape (both this PRD and the app PRD cite it verbatim):

```jsonc
{
  "scope": "blog/",                 // normalized prefix; "." => whole vault (root sentinel is ".", not "")
  "recursive": false,
  "columns": [
    {
      "name": "status",             // column name == frontmatter key (matches SchemaField.name)
      "field_type": "String",       // PascalCase: String|Number|Boolean|List|Date|Mixed (reuses schema::FieldType)
      "description": "Publication status", // string | null (from overlay)
      "occurrence_count": 12,        // 0 when in_schema:false
      "sample_values": ["draft","published"],   // [] when in_schema:false
      "allowed_values": ["draft","published"],   // string[] | null (usually null — overlay-declared only)
      "required": true,             // false when in_schema:false
      "in_schema": true             // false = key discovered from a row's frontmatter, not the scoped schema
    }
  ],
  "rows": [
    {
      "path": "blog/launch.md",     // relative, forward slashes — stable row id
      "title": "Launch Announcement", // derived SERVER-SIDE, never empty (single source of truth)
      "title_source": "frontmatter",  // "frontmatter" | "filename"
      "frontmatter": { "title": "Launch Announcement", "status": "published", "tags": ["news"] }, // ALWAYS object; {} never null
      "content_hash": "abc123…",    // string | null (null for state:"new")
      "file_size": 2048,
      "modified_at": 1718000000,    // number | null
      "indexed_at": 1718000000,     // number | null (null for state:"new")
      "state": "indexed"            // "indexed"|"modified"|"new"|"deleted" (lowercase)
    }
  ],
  "total_rows": 37,                 // post-filter, pre-limit/offset
  "limit": 50,                      // number | omitted (None => key absent)
  "offset": 0
}
```

**Hard guarantees the app relies on:**
- `columns[].field_type` ∈ `{"String","Number","Boolean","List","Date","Mixed"}` (**PascalCase** — `schema::FieldType` serializes PascalCase today; do NOT add `rename_all` to it, that would break the existing `mdvdb schema --json` the app already consumes).
- `columns[].name` is the frontmatter key (NOT `key`). `columns[].in_schema` is `false` for keys present in some row's frontmatter but absent from the scoped schema.
- `rows[].frontmatter` is **always a JSON object** (`{}` when the file has no frontmatter), **never `null`**.
- `rows[].title` is always a **non-empty** string; `rows[].title_source` ∈ `{"frontmatter","filename"}`.
- `rows[].state` ∈ `{"indexed","modified","new","deleted"}` (lowercase, reuses `tree::FileState`).
- `rows[].content_hash` and `rows[].indexed_at` are `null` for `state:"new"` rows; `rows[].modified_at` is `null` when unknown.
- `total_rows` is the count **after** filtering but **before** `limit`/`offset`.

## Goals

- One call returns: column definitions, the page of rows (full frontmatter each), and the total row count after filtering.
- Strictly read-only: no Markdown writes, no index version bump, no new index fields.
- Row scope toggle: direct children of the folder (default) vs recursive (all nested subfolders) via `--recursive`.
- Server-side sort by any frontmatter field, `asc`/`desc`, with type-aware ordering and a deterministic **nulls-last** rule.
- Server-side filtering that **reuses** `MetadataFilter` (`Equals`/`In`/`Range`/`Exists`) from `src/search.rs` — no new filter logic.
- Server-side pagination (`--limit`/`--offset`) returning `total_rows` (post-filter, pre-pagination).
- Columns = scoped-schema fields **unioned** with frontmatter keys actually present in the returned folder that the schema missed (flagged `in_schema: false`).
- Each row includes the contract fields above, with title derived server-side (single source of truth) and `title_source` exposed.
- Human-readable output for terminal use; `--json` for the app contract.
- Full test coverage (unit + CLI integration) per project testing requirements, including a golden-JSON test that pins field names + casing.

## Non-Goals

- **No Markdown writes.** Editing cells / adding / deleting rows is the app's job (app PRD). mdvdb stays read-only and the frontmatter contract is read-only.
- **No new index fields and no index version bump.** This feature reads existing `StoredFile`/`scoped_schemas` data only.
- **No persisted title.** Title is derived at query time (see Title Derivation). We do **not** add a `title` field to `StoredFile` in v1.
- **No saved views, column show/hide/reorder/resize, group-by collapsing.** These are pure app-side view state (app PRD). The backend returns all columns + all (paginated) rows; the app decides presentation.
- **No semantic ranking.** `collection` is a deterministic metadata listing, not a search. It does not embed, call HNSW, or use BM25.
- **No multi-field sort.** v1 sorts by a single field. Multi-key sort is deferred.
- **No first-H1 title fallback in v1** (see Title Derivation tradeoff — explicitly deferred).
- **No new-file frontmatter reading.** Files on disk but not in the index (`state: "new"`) are listed with empty frontmatter; their content is not parsed (see Row Gathering).
- **No richer CLI `--filter` grammar.** The CLI `--filter` exposes `Equals` (`KEY=VALUE`) only, matching `search`'s CLI surface; `Range`/`In`/`Exists` are reachable via the library API but not the CLI flag in v1.

## Technical Design

### Data Model Changes

**None to the index.** No changes to `StoredFile`, `IndexMetadata`, or `src/index/storage.rs::VERSION` (currently `1`). The feature is purely additive at the API layer and reads existing data: `StoredFile.{frontmatter (JSON string), file_size, relative_path, content_hash, indexed_at}`, the persisted `scoped_schemas`, and `file_mtimes`. This is why **no migration is required** (see Migration Strategy).

**New serde `Serialize` response types** (live in `src/lib.rs` alongside `DocumentInfo` near line 199, mirroring its derive style — `#[derive(Debug, Clone, Serialize)]`). These are the app contract; field names and types are exact:

```rust
/// Query options for `MarkdownVdb::collection`.
#[derive(Debug, Clone)]
pub struct CollectionQuery {
    /// Folder path prefix (relative to project root). "" or "." means the whole vault.
    pub path: String,
    /// If true, include files in all nested subfolders. If false, only direct children.
    pub recursive: bool,
    /// Frontmatter field name to sort rows by. None = sort by path ascending.
    pub sort_by: Option<String>,
    /// Sort direction.
    pub order: SortOrder,
    /// Metadata filters (AND logic), reusing the search engine's MetadataFilter.
    pub filters: Vec<MetadataFilter>,
    /// Max rows to return after filtering+sorting. None = all rows.
    pub limit: Option<usize>,
    /// Number of rows to skip (for pagination).
    pub offset: usize,
}

/// Sort direction for collection rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

/// How a row's title was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleSource {
    /// From the frontmatter `title` field.
    Frontmatter,
    /// From the filename stem (no usable frontmatter title).
    Filename,
}

/// Top-level response: columns + the paginated page of rows + total count.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionResponse {
    /// The resolved scope prefix (normalized, e.g. "blog/").
    pub scope: String,
    /// Whether nested subfolders were included.
    pub recursive: bool,
    /// Column definitions (scoped schema fields ∪ present-but-unscoped frontmatter keys).
    pub columns: Vec<CollectionColumn>,
    /// The page of rows after filtering, sorting, and limit/offset.
    pub rows: Vec<CollectionRow>,
    /// Total rows after filtering, BEFORE limit/offset (for pagination UIs).
    pub total_rows: usize,
    /// Echo of the applied limit (key omitted from JSON if None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Echo of the applied offset.
    pub offset: usize,
}

/// One table column. Derived from the scoped Schema's SchemaField, plus `in_schema`.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionColumn {
    /// Frontmatter key / column name. (NOT "key" — matches SchemaField.name.)
    pub name: String,
    /// Inferred or overlay-declared type. Reuses schema::FieldType (PascalCase JSON).
    pub field_type: schema::FieldType,
    /// Human-readable description from overlay, if any.
    pub description: Option<String>,
    /// Number of files (in this scope) that have this field. 0 for present-but-unscoped keys.
    pub occurrence_count: usize,
    /// Up to 20 sample values from the schema (empty for present-but-unscoped keys).
    pub sample_values: Vec<String>,
    /// Allowed values from overlay, if declared (usually None).
    pub allowed_values: Option<Vec<String>>,
    /// Whether the field is marked required in the overlay.
    pub required: bool,
    /// True if the column came from the scoped Schema; false if it was discovered
    /// only because a returned row's frontmatter contained the key.
    pub in_schema: bool,
}

/// One table row = one Markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionRow {
    /// Relative path (project-root-relative, forward slashes). Stable row id.
    pub path: String,
    /// Derived display title (frontmatter.title -> filename stem). Never empty.
    pub title: String,
    /// How the title was derived.
    pub title_source: TitleSource,
    /// Full frontmatter as a JSON object. Always an object ({} if none), never null.
    pub frontmatter: serde_json::Value,
    /// SHA-256 content hash from the index. None for files not in the index (state == New).
    pub content_hash: Option<String>,
    /// File size in bytes.
    pub file_size: u64,
    /// Filesystem modification time (Unix seconds), if known.
    pub modified_at: Option<u64>,
    /// When this file was last indexed (Unix seconds). None for state == New.
    pub indexed_at: Option<u64>,
    /// Sync state relative to the index.
    pub state: tree::FileState,
}
```

`CollectionColumn` reuses `schema::FieldType` (the same enum the app already sees in `mdvdb schema --json`, serialized PascalCase) and `CollectionRow.state` reuses `tree::FileState` (the same lowercase-serialized enum the app already sees in `mdvdb tree --json`), so the app's existing type knowledge transfers directly.

> **Contract note (from cross-PRD review):** `frontmatter` is typed `serde_json::Value`, **not** `Option<Value>`. The existing `DocumentInfo.frontmatter` is `Option<Value>` and serializes as `null`; do **not** copy that. Build `frontmatter` as `serde_json::Value::Object`, defaulting to `serde_json::json!({})`, so the app can index `rows[].frontmatter[col]` without null-checking.

### Title Derivation

**v1 rule (chosen): `frontmatter.title` → filename stem.** Title is derived **server-side only** — it is the single source of truth, so the app must consume `row.title`/`row.title_source` rather than re-derive.

1. If `frontmatter` is an object containing a `title` key whose value is a non-empty string, use that string → `title_source = Frontmatter`.
2. Otherwise use the filename stem of `path` (last path segment with the `.md`/`.markdown` extension removed). E.g. `blog/2024/my-post.md` → `my-post` → `title_source = Filename`.
3. The result is guaranteed non-empty (a `.md` file always has a stem).

**First-H1 fallback — explicitly deferred, with the tradeoff documented:** the index does **not** store H1 headings or any title (`StoredFile` in `src/index/types.rs` has no such field). Adding an H1 fallback would require either (a) reading + parsing each file at query time (defeats the "one cheap call" goal on recursive scopes and forces filesystem content reads), or (b) persisting a `title` on `StoredFile` at ingest — a data-model + index-version change, exactly the Non-Goal this phase avoids. **Decision for v1:** ship `frontmatter.title → stem`; the `title`/`title_source` fields are forward-compatible if a future phase adds option (b).

### Row Gathering, Scope, and Sync State

Rows are gathered from the index's `StoredFile` map plus the on-disk folder, reusing the exact classification logic that `tree::build_file_tree` already implements (`src/tree.rs:47`). Concretely:

1. **Normalize the scope prefix.** Trim a leading `./`; treat `""`/`"."`/`"./"` as the whole-vault scope. If non-empty and not ending in `/`, append `/` so prefix matching is path-segment-safe (mirrors `SearchQuery.path_prefix` `starts_with` usage in `src/search.rs:708`). A file matches the scope when `relative_path.starts_with(scope)` (or always, for the empty scope). The CLI positional defaults to `.` (the agreed root sentinel — the app sends `.`, never `""`).
2. **Depth filter.** After the prefix match, for **non-recursive** (default), keep a file only if the path **after the scope prefix** contains no further `/` (i.e., it is a direct child). For **recursive**, keep all prefix matches. (Direct-children check: `remainder.find('/').is_none()`.)
3. **Sync state per file** — same four-way classification as `build_file_tree` (`src/tree.rs:64-91`), so the app sees consistent states everywhere:
   - **Indexed:** on disk + in index, content hash matches. `content_hash`/`indexed_at` from `StoredFile`.
   - **Modified:** on disk + in index, hash differs. `content_hash`/`indexed_at` from `StoredFile`.
   - **New:** on disk, not in index. Frontmatter = `{}` (we do NOT parse new files in v1 — they have no `StoredFile`). `content_hash`/`indexed_at` = `null`. `file_size` from `std::fs::metadata`; `modified_at` from fs mtime if cheaply available, else `None`; `title` from filename stem.
   - **Deleted:** in index, not on disk. Included so the app can show stale rows. Frontmatter/`content_hash`/`indexed_at` from the stored `StoredFile`; `state: "deleted"`.
4. **Build a `CollectionRow`** for each kept file: `frontmatter` parsed from `StoredFile.frontmatter` JSON string (`serde_json::from_str`, falling back to `{}` on parse error or `None` — never `null`), `file_size` from `StoredFile.file_size` (or fs for new files), `content_hash`/`indexed_at` from `StoredFile` (None for new), `modified_at` from `Index::get_file_mtime` (mirrors `get_document`, `src/lib.rs:2107`), `title`/`title_source` per the rule above, `state` from classification.

This reuses the index accessors `get_file_hashes` (`src/index/state.rs:351`), `get_file` (`:345`), `get_indexed_file_paths` (`:675`), `get_file_mtime` (`:432`), and the discovery + hash-compare flow already proven in `tree.rs`. To avoid re-reading every file for hashing on large recursive scopes, prefer comparing against `get_file_hashes` and only `std::fs::read_to_string` + `compute_content_hash` for files that are present in both disk and index within the scope (same as `tree.rs:67-78`).

> **Honest performance note (from cross-PRD review):** sync-state classification reuses `build_file_tree`'s read+hash of each in-scope indexed file. This is fine for a single non-recursive folder but is N reads+hashes for `--recursive` over a large vault. Document this cost; recommend the app default to **non-recursive**. A cheaper path — compare fs `mtime` against `indexed_at` and only rehash when `mtime` is newer — is a worthwhile **follow-up (Phase 29b)**, not required for v1.

### Filtering, Sorting, Pagination Order of Operations

Applied in this exact order so `total_rows` is well-defined:

1. **Gather + scope/depth filter** (above) → candidate rows.
2. **Metadata filter:** reuse `MetadataFilter` evaluation from `src/search.rs`. The filter logic there (`evaluate_filters` / `evaluate_single_filter`, `src/search.rs:1196-1257`) is currently private. Make `evaluate_filters(filters: &[MetadataFilter], frontmatter: Option<&Value>) -> bool` `pub(crate)` and call it per row against the row's `frontmatter`. **Do not** reimplement filtering. A row with `frontmatter == {}` and a non-empty filter set fails (consistent with search semantics where `None`/empty frontmatter + any filter → false) — see the `New`-rows caveat below.
3. **Compute `total_rows`** = number of rows surviving step 2 (before sort; before limit/offset).
4. **Sort** by `sort_by` (type-aware, nulls-last; see below). If `sort_by` is `None`, sort by `path` ascending for deterministic output.
5. **Paginate:** skip `offset`, take `limit` (or all if `None`).
6. **Compute columns** (below) from the scoped schema + the keys present across the **full filtered set** (not just the page), so the column set is stable across pages.

> **`New`/unindexed rows + filters (from cross-PRD review):** because a `{}`-frontmatter row fails any non-empty filter set, **any active server-side filter drops `New`/`Deleted`-with-empty rows.** This is intended and consistent with `search`. The app PRD documents that when it needs `New` rows visible under a filter, it applies that filter **client-side** instead of via `--filter`.

### Type-Aware Sort + Nulls-Last Rule

Sort key for a row = the value of `frontmatter[sort_by]`:

- A row is "null" for sorting if `frontmatter` lacks the key or the value is `Value::Null`. **All null rows sort after all non-null rows, regardless of `asc`/`desc`** (nulls-last). Final ordering among nulls is broken by `path` ascending for determinism.
- Non-null comparison must return a true `std::cmp::Ordering` (not a bool). The existing `compare_values(a, b, ordering) -> bool` (`src/search.rs:1261`) cannot express the less/equal/greater trichotomy a stable sort needs, so add a genuinely new `pub(crate) fn compare_json_for_sort(a: &Value, b: &Value) -> Ordering` in `src/search.rs` that uses the same numeric-then-lexicographic rule as the filter path (`as_f64` / `value_as_string`, `src/search.rs:1275-1285`): if both parse as numbers, compare as `f64`; booleans compare `false < true`; lists/objects fall back to their JSON-string form; otherwise compare as strings. Add a unit test proving it is **consistent** with `compare_values`' rule so sort and `Range`-filter ordering never disagree.
- Nulls-last is handled in the sort **wrapper**, not inside `compare_json_for_sort`. `desc` reverses only the non-null comparison; nulls remain last.

### Column Union

1. Resolve the scoped schema for the normalized scope via `MarkdownVdb::schema_scoped(scope)` (`src/lib.rs:1551`) — returns `ScopedSchema { scope, schema }`, preferring persisted `scoped_schemas` and falling back to on-the-fly inference. Each `SchemaField` (`src/schema.rs:69`) becomes a `CollectionColumn` with `in_schema: true`, copying `name`, `field_type`, `description`, `occurrence_count`, `sample_values`, `allowed_values`, `required`.
2. Collect the union of all top-level frontmatter keys present across the **filtered** row set. For any key not already a column, append a `CollectionColumn` with `in_schema: false`, `field_type` inferred from the first non-null observed value (reuse `schema::infer_field_type` — make it `pub(crate)` if needed), `occurrence_count: 0`, empty `sample_values`/`allowed_values`, `required: false`. This guarantees the app can render a column for every value it will encounter, while still distinguishing declared schema columns from ad-hoc ones.
3. Column order: schema fields first (the `Schema.fields` are already alphabetically sorted, `src/schema.rs:101`), then unscoped keys appended in sorted order, for determinism.

### Interface Changes

**`src/lib.rs` — new method on `MarkdownVdb`:**

```rust
impl MarkdownVdb {
    /// Return all documents under a folder as table rows, with column definitions,
    /// applying server-side filter/sort/pagination. Strictly read-only.
    pub fn collection(&self, opts: CollectionQuery) -> Result<CollectionResponse>;
}
```

Synchronous (like `preview`, `src/lib.rs:1420`, and `file_tree`/`get_document`) — it performs no embedding or network calls.

**`src/search.rs` — visibility/additions (no behavior change to existing callers):**
- `evaluate_filters` → `pub(crate)`.
- Add `pub(crate) fn compare_json_for_sort(a: &Value, b: &Value) -> Ordering` (new code; see Sort).

**`src/schema.rs`** — `infer_field_type` → `pub(crate)` if not already, for unscoped-column type inference.

**Re-exports (`src/lib.rs`):** add `CollectionQuery`, `CollectionResponse`, `CollectionColumn`, `CollectionRow`, `SortOrder`, `TitleSource` to the public re-exports (alongside `SearchQuery`, `MetadataFilter`, `FileState`, etc.).

### New Commands / API / UI

**Command name: `mdvdb collection <PATH>`** (with hidden alias `list`).

Justification: "collection" is the product-level noun for "a folder rendered as a table of documents" and does not collide with existing verbs (`search`, `tree`, `get`, `schema`). `list` is registered as a **hidden** alias for muscle-memory without cluttering help. The command sits naturally beside `tree` (structure) and `get` (single doc): `collection` is "the folder as a table".

CLI flags (mirroring `SearchArgs`/`SchemaArgs` in `src/main.rs:168,264`):

```
mdvdb collection <PATH> [--recursive] [--sort <FIELD>] [--order asc|desc]
                        [--filter KEY=VALUE]... [--limit <N>] [--offset <N>] [--json]
```

- `<PATH>` — positional, the folder prefix (relative). Required; defaults to `.` for the whole vault (agreed root sentinel).
- `--recursive` / `-r` — bool flag; include nested subfolders. Default off (direct children only).
- `--sort <FIELD>` — frontmatter field to sort by. Omitted → sort by path. (**Separate from `--order`** — do NOT accept a combined `"field:asc"` string.)
- `--order <asc|desc>` — `value_parser` over `SortOrder` (impl `FromStr` like `SearchMode`, `src/search.rs:30`), default `asc`.
- `--filter KEY=VALUE` — **repeatable** `Vec<String>`, parsed by the existing `parse_filter` helper (`src/main.rs:404`) into `MetadataFilter::Equals`. Each occurrence adds one filter (AND). (Range/In/Exists are available via the library API; the CLI exposes `Equals` only in v1, matching `search`.)
- `--limit <N>` / `--offset <N>` — `Option<usize>` / `usize` (default 0).
- `--json` — global flag (`Cli.json`, `src/main.rs:97`).

Handler (in `run()`'s match, beside `Commands::Get`, `src/main.rs:899`): open `MarkdownVdb::open_readonly_with_config(cwd, config)`, build `CollectionQuery`, call `vdb.collection(opts)?`. With `--json`, `serde_json::to_writer_pretty` the `CollectionResponse`. Without, call a new `format::print_collection(&resp)` that prints a compact table (scope header line like `print_schema`'s scope line `src/format.rs:538`, a column legend, and `title · path · state` rows with a `Showing N–M of total_rows` footer). Add the subcommand to shell completions.

### Migration Strategy

**No migration and no index version bump.** `collection` reads only existing index data (`StoredFile`, persisted `scoped_schemas`, `file_mtimes`) and the on-disk folder. `src/index/storage.rs::VERSION` (currently `1`) is **unchanged**. Existing indexes work immediately after upgrade; no `ingest --reindex` is required. Backward compatibility:

- If `scoped_schemas` is absent (older index), `schema_scoped` already falls back to on-the-fly inference (`src/lib.rs:1556-1571`), so columns still resolve.
- If `file_mtimes` is absent (pre-Phase-18 index), `get_file_mtime` returns `None` → rows carry `modified_at: null`, no error.
- The JSON contract is additive; no existing command's output changes.

### Open Item to Verify During Implementation

The app re-indexes a single file after a frontmatter edit (`ingest --file --reindex`) and then expects newly-added keys to appear as `in_schema:true` columns. **Verify whether single-file ingest recomputes the persisted `scoped_schemas`** (in `src/lib.rs`'s ingest path). If it only updates that file's chunks, a newly-added key stays `in_schema:false` until a full `ingest`. Either: (a) recompute the affected scope's schema on single-file ingest, or (b) document that new keys surface as `in_schema:false` columns until the next full ingest (the app still renders them — `collection`'s column union already includes present-but-unscoped keys). Record the chosen behavior here and in the app PRD.

> **RESOLVED (v1 → behavior (b)):** Verified that **single-file ingest does NOT recompute the persisted `scoped_schemas`**. The schema-recomputation block in the ingest path is explicitly gated on `if options.file.is_none()` (`src/lib.rs`, comment: *"For single-file ingest, skip schema recomputation"*) — a `--file` ingest only re-parses, re-embeds, and upserts that one file's chunks plus its `StoredFile` (frontmatter included); the persisted global/scoped schemas are left untouched. Consequently, a key added via a single-file re-index appears in that row's `frontmatter` but **not** in the scoped schema, so `collection` emits it as an **`in_schema:false`** column (`occurrence_count:0`, empty `sample_values`) until the next **full** `ingest` promotes it to `in_schema:true`. This is intentional: `collection`'s column union always includes present-but-unscoped keys, so the app renders the new column either way — it just won't carry schema metadata (description/allowed_values/required/occurrence_count) until the next full ingest. The integration test `test_collection_columns_union` (`tests/collection_test.rs`) pins this exact behavior. The app PRD should mirror this note.

## Implementation Steps

1. **Add `SortOrder` + `TitleSource`** — `src/lib.rs` (or `src/search.rs` for `SortOrder`): define both enums with `#[serde(rename_all = "lowercase")]`; `SortOrder::default() = Asc` + `impl FromStr` mirroring `SearchMode::from_str` (`src/search.rs:30`) for CLI parsing.

2. **Expose filter + add sort comparator** — `src/search.rs`: change `evaluate_filters` to `pub(crate)`. Add `pub(crate) fn compare_json_for_sort(a: &Value, b: &Value) -> Ordering` delegating to the existing numeric-then-string logic (`as_f64`/`value_as_string`, lines 1275-1285). Unit-test number/string/bool/list ordering and consistency with `compare_values`.

3. **Expose `infer_field_type`** — `src/schema.rs`: make `infer_field_type` `pub(crate)` (line 121) for unscoped-column typing. No behavior change.

4. **Define the response types** — `src/lib.rs`: add `CollectionQuery`, `CollectionResponse`, `CollectionColumn`, `CollectionRow` near `DocumentInfo` (line 199), with derives exactly as specified. Reuse `schema::FieldType` and `tree::FileState`. `frontmatter` is `serde_json::Value` (never `Option`).

5. **Implement gathering + classification** — `src/lib.rs`: add a private helper `gather_collection_rows(&self, scope: &str, recursive: bool) -> Result<Vec<CollectionRow>>` reusing the discovery + hash-compare classification from `tree::build_file_tree` (`src/tree.rs:47-91`). Normalize the scope (trim `./`, append trailing `/`, treat empty/`.` as whole vault), apply prefix + direct-children depth filter, build a `CollectionRow` per file with derived title/`title_source`, parsed frontmatter (`{}` fallback, never `null`), `file_size`, `content_hash`/`indexed_at` (None for `New`), `modified_at` (`Index::get_file_mtime`), and `state`. Do not parse `New` files.

6. **Implement `collection`** — `src/lib.rs`: `pub fn collection(&self, opts: CollectionQuery) -> Result<CollectionResponse>`. Pipeline exactly: gather rows → `search::evaluate_filters` per row → compute `total_rows` → sort (nulls-last wrapper + `compare_json_for_sort`, or path-asc when `sort_by` is `None`) → `offset`/`limit` → build columns (`schema_scoped` ∪ present unscoped keys from the full filtered set). Return `CollectionResponse` echoing `scope`, `recursive`, `limit`, `offset`.

7. **Title helper** — `src/lib.rs`: `fn derive_title(path: &str, frontmatter: &Value) -> (String, TitleSource)` implementing `frontmatter.title` (non-empty string) → filename stem. Unit-test directly.

8. **Re-exports** — `src/lib.rs`: re-export the new types + `SortOrder`/`TitleSource`.

9. **CLI subcommand** — `src/main.rs`: add `Collection(CollectionArgs)` to `enum Commands` (line 109) with doc comment "List a folder's documents as a table (rows = files, columns = frontmatter)"; register hidden alias `list` via `#[command(alias = "list")]`. Define `struct CollectionArgs { path: String (positional, default ".") , #[arg(short,long)] recursive: bool, #[arg(long)] sort: Option<String>, #[arg(long, default_value="asc")] order: SortOrder, #[arg(short,long)] filter: Vec<String>, #[arg(long)] limit: Option<usize>, #[arg(long, default_value="0")] offset: usize }` mirroring `SearchArgs` (line 168). `--sort` and `--order` are separate args; `--filter` is `Vec<String>`.

10. **CLI handler** — `src/main.rs` in `run()` (beside `Commands::Get`, line 899): open readonly, build `CollectionQuery` (parse each `--filter` via existing `parse_filter`, line 404), call `vdb.collection(opts)?`; `--json` → `to_writer_pretty`; else `format::print_collection(&resp)`.

11. **Human formatter** — `src/format.rs`: add `pub fn print_collection(resp: &CollectionResponse)` modeled on `print_schema` (line 528): scope header line, column legend (name + type + `[required]` tag, `in_schema:false` columns dimmed), then a row table of `title`/`path`/`state` with a `Showing N–M of total_rows` footer. Reuse existing color/bar helpers.

12. **Completions** — `src/main.rs`: ensure `collection` and its flags appear in generated shell completions.

13. **Unit tests** — `#[cfg(test)]` blocks: `derive_title` (frontmatter title vs stem vs nested path; `title_source` correct), `compare_json_for_sort` (numeric vs string vs bool ordering, consistency with `compare_values`), nulls-last invariant, scope normalization + direct-children-vs-recursive depth filter.

14. **Integration tests** — `tests/collection_test.rs` (new file), using `mock_config()` + `EmbeddingProviderType::Mock` (8 dims) + `tempfile::TempDir`, ingesting a fixture vault with `blog/` (frontmatter `title`,`status`,`date`) and nested `blog/2024/`:
    - `test_collection_direct_children_only` — non-recursive excludes `blog/2024/*`.
    - `test_collection_recursive_includes_nested`.
    - `test_collection_columns_union` — a frontmatter key absent from schema appears with `in_schema:false`.
    - `test_collection_title_derivation` — frontmatter title used (`title_source:"frontmatter"`); filename stem fallback otherwise (`"filename"`).
    - `test_collection_sort_asc_desc_nulls_last` — missing-field rows sort last in both directions.
    - `test_collection_filter_reuses_metadatafilter` — `Equals` narrows rows; `total_rows` reflects post-filter count; a `{}`-frontmatter `New` row is dropped under any filter.
    - `test_collection_pagination_total_rows` — `limit`/`offset` page correctly; `total_rows` independent of page.
    - `test_collection_frontmatter_always_object` — file with no frontmatter yields `{}`, not `null`.
    - `test_collection_new_and_deleted_states` — add an unindexed `.md` on disk → `state:"new"` with `{}` frontmatter and `content_hash`/`indexed_at` null; delete an indexed file → `state:"deleted"`.

15. **CLI integration + golden-JSON tests** — `tests/cli_test.rs` (extend), using `env!("CARGO_BIN_EXE_mdvdb")`: a **golden test** that pins the exact JSON field names + casing (`columns[].name`, `field_type:"String"`, `total_rows`, `title_source`, `content_hash`, `indexed_at`, `rows[].frontmatter` an object, `rows[].state` a known lowercase string) so the app can mirror types verbatim; plus `--recursive`, `--sort status --order desc`, `--filter status=published` (and a second `--filter` to prove repeatability), `--limit 1 --offset 1` behavior on stdout JSON.

## Validation Criteria

- [ ] `cargo test` passes — all existing tests plus the new `collection` unit + integration + CLI/golden tests.
- [ ] `cargo clippy --all-targets` passes with zero warnings.
- [ ] `mdvdb collection <PATH> --json` returns the exact canonical contract: `scope`, `recursive`, `columns[]` (with `name`/`field_type` PascalCase/`description`/`in_schema`), `rows[]` (with `title`/`title_source`/`frontmatter` object/`content_hash`/`indexed_at`/`state`), `total_rows`, `limit`, `offset`.
- [ ] No Markdown files are written or modified by any `collection` invocation (read-only verified).
- [ ] `src/index/storage.rs::VERSION` is unchanged; a pre-Phase-29 index serves `collection` with no `ingest --reindex`.
- [ ] Non-recursive returns only direct children; `--recursive` includes all nested subfolders.
- [ ] Filtering uses `search::MetadataFilter` (no duplicated filter logic); `total_rows` is the post-filter, pre-pagination count; repeated `--filter` flags AND together.
- [ ] Sorting is type-aware (numeric vs lexicographic) and nulls (missing/`null` field) sort last in both `asc` and `desc`; `compare_json_for_sort` is consistent with `compare_values`.
- [ ] `--limit`/`--offset` paginate correctly and `total_rows` is independent of the returned page size.
- [ ] `columns` = scoped-schema fields ∪ present-but-unscoped frontmatter keys; unscoped keys carry `in_schema:false`, `occurrence_count:0`.
- [ ] Every `rows[].title` is non-empty; `rows[].frontmatter` is always a JSON object (`{}` when absent), never `null`; `content_hash`/`indexed_at` are `null` exactly for `state:"new"`.
- [ ] `rows[].state` matches `mdvdb tree`'s classification (Indexed/Modified/New/Deleted).
- [ ] Pre-Phase-18 index (no `file_mtimes`) yields `modified_at: null` without error; index without persisted `scoped_schemas` still resolves columns via on-the-fly inference.
- [ ] The "single-file ingest recomputes scoped schemas?" open item is resolved and the chosen behavior is documented.

## Anti-Patterns to Avoid

- **Do NOT write to Markdown files or add a frontmatter writer.** mdvdb is strictly read-only (CLAUDE.md: "The system NEVER writes to markdown files"). Cell edits / add-row / delete-row are the app's job. Even a "convenience" writer here violates the architecture.
- **Do NOT bump the index version or add fields to `StoredFile`/`IndexMetadata`.** This feature reads existing data only. A version bump would force every user to `ingest --reindex` for a read-only listing.
- **Do NOT reimplement metadata filtering.** Reuse `evaluate_filters`/`MetadataFilter` from `src/search.rs`. Divergent filter semantics between `search` and `collection` would confuse the app, which uses the same `MetadataFilter` for both. Same for value comparison in sort — reuse the search comparison logic (but with a real `Ordering`-returning comparator, not a bool).
- **Do NOT let `frontmatter` serialize as `null`** and do NOT type it `Option<Value>`. The app indexes into `rows[].frontmatter[col]`; emit `{}` for files without frontmatter. Likewise `title` must never be empty.
- **Do NOT add `#[serde(rename_all)]` to `schema::FieldType`** to make casing lowercase. It serializes PascalCase today and the app already consumes that from `mdvdb schema --json`. The contract is PascalCase.
- **Do NOT use a combined `--sort field:dir` string or a single non-repeatable `--filter`.** `--sort`/`--order` are separate; `--filter` is repeatable `Vec<String>`. (Mismatching this silently sorts by a nonexistent field / drops all but one filter.)
- **Do NOT read/parse every file on disk for content.** For indexed rows, take frontmatter from `StoredFile.frontmatter`. Hash-compare only what `tree.rs` already hash-compares. Reading + parsing each file (e.g. for an H1 title) on a recursive scope reintroduces the N+1 cost this command eliminates.
- **Do NOT embed, call HNSW, or use BM25.** `collection` is a deterministic metadata listing. Keep it synchronous (like `preview`/`file_tree`) — no provider, no network, no async.
- **Do NOT make the column set depend on the returned page.** Compute columns from the full filtered set so the layout is stable as the app pages.
- **Do NOT put nulls first or order them inconsistently between asc/desc.** Nulls-last in both directions; tie-break by `path`.
- **Do NOT `unwrap()` in the library path.** Return `Result<_, Error>`; parse failures (e.g. bad stored frontmatter JSON) degrade to `{}`, not a panic.

## Patterns to Follow

- **Read-only open + handler shape** — `Commands::Get` handler at `src/main.rs:899-910`: `MarkdownVdb::open_readonly_with_config(cwd, config)?`, call the method, branch on `json` with `serde_json::to_writer_pretty` vs a `format::print_*`. The new `collection` handler mirrors it.
- **Args derive + positional path + flags** — `SearchArgs` (`src/main.rs:168-239`) and `SchemaArgs` (`:264-268`) show `#[arg(short, long)]` repeatable `--filter Vec<String>`, `Option<usize>` limits, and `--path` prefix conventions; reuse `parse_filter` (`:404-423`).
- **`FromStr` for a CLI enum** — `SearchMode::from_str` (`src/search.rs:30-44`) is the template for `SortOrder`'s `--order` parsing and its `#[serde(rename_all="lowercase")]` Serialize.
- **Sync-state classification** — `tree::build_file_tree` (`src/tree.rs:47-104`) is the authoritative Indexed/Modified/New/Deleted logic (discover + `get_file_hashes` + hash compare). Reuse its approach for `gather_collection_rows`; reuse `FileState` (`src/tree.rs:13-20`) directly in `CollectionRow`.
- **Index accessors** — `get_file` (`src/index/state.rs:345`), `get_file_hashes` (`:351`), `get_indexed_file_paths` (`:675`), `get_file_mtime` (`:432`). `get_document` (`src/lib.rs:2094-2118`) shows the exact frontmatter-parse + `get_file_mtime` pattern to copy per row (but default `frontmatter` to `{}`, not `null`).
- **Scoped schema → columns** — `schema_scoped` (`src/lib.rs:1551-1572`) returns `ScopedSchema { scope, schema }` with persist-then-infer fallback; map each `SchemaField` (`src/schema.rs:69-84`) into `CollectionColumn`. `infer_field_type` (`src/schema.rs:121`) types unscoped columns.
- **Filter + comparison reuse** — `evaluate_filters`/`evaluate_single_filter` (`src/search.rs:1196-1257`) and the helpers `as_f64`/`value_as_string`/`compare_values` (`:1261-1285`) are the single source of truth for filtering and ordering; expose/extend, don't duplicate.
- **Human formatter** — `format::print_schema` (`src/format.rs:528-`) shows the scope header line (`:538-544`), the `●`/bold/dimmed section style, and the `[required]` tag — model `print_collection` on it.
- **Response struct style** — `DocumentInfo` (`src/lib.rs:199-216`) and `SearchResultFile` (`src/search.rs:232-244`) show the `#[derive(Debug, Clone, Serialize)]` convention and the `modified_at: Option<u64>`/`file_size: u64` shapes already exposed — keep field names aligned.
- **Testing conventions** — `tests/schema_test.rs` (scoped-schema integration) and `tests/cli_test.rs` (`env!("CARGO_BIN_EXE_mdvdb")`) are the templates; use `mock_config()` + `EmbeddingProviderType::Mock` (8 dims) + `tempfile::TempDir`, per CLAUDE.md testing requirements.
