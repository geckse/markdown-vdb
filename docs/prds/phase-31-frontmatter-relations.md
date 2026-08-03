# PRD: Phase 31 — Frontmatter Relations (Wiki-Link References as Foreign Keys)

## Overview

Folders already act as tables (phase 29 `collection`; app phase 39 table view). This phase adds the missing relationship part: frontmatter values like `client: clients/acme.md` become **foreign keys**. Plain `.md` paths are the preferred authoring form; wiki-link values such as `client: "[[clients/acme]]"` and Markdown links remain supported. The schema auto-infers a new `Relation` field type (refinable via the `.markdownvdb.schema.yml` overlay with an optional `target:` folder — the FK's "table"), all read paths (`get`, `collection`, `search`) gain a `--populate` flag that resolves relation values inline to `{raw, path, exists, title, frontmatter}` (depth 1, JOIN-like), `get --populate` additionally exposes `referenced_by` (reverse lookup — which documents reference this one, via which field), and frontmatter links join the existing link graph as first-class `LinkEntry` edges tagged with their originating `field` — so backlinks, orphan detection, multi-hop `--expand`, and link-boost see them with zero changes to those consumers. mdvdb stays strictly read-only: creating, editing, or repairing relations in markdown files is the app's job.

This PRD and the app PRD (`app/docs/prds/phase-42-frontmatter-relations.md`) share **one canonical JSON contract** (below). This PRD is authoritative; the app mirrors the exact field names and types in `types/cli.ts`. Any divergence breaks the integration.

## Problem Statement

Frontmatter references are opaque strings today:

- `yaml_to_json` (`src/parser.rs:176-210`) keeps `client: "[[clients/acme]]"` as the literal JSON string `"[[clients/acme]]"`, and `parse_markdown_file` extracts links from the **body only** (`extract_links(body)`, `src/parser.rs:72`). A frontmatter reference therefore never enters the link graph: `backlinks`, `orphans`, `links_neighborhood`, `mdvdb graph`, link-boost, and `--expand` are all blind to it, and a document referenced only via frontmatter shows up as an orphan.
- There is no JOIN. To render a relation column ("show the client's name on each invoice row"), the app must issue one `get` per referenced document — the exact N+1 process-spawn pattern phase 29 eliminated for rows.
- Filtering fails: `--filter client=clients/acme` compares raw strings (`evaluate_single_filter`, `src/search.rs:1242-1291`) and does not match `"[[clients/acme]]"` or `"[[clients/acme|Acme]]"`.
- The schema types a link field as `String` (`infer_field_type`, `src/schema.rs:121-136`), so consumers cannot distinguish a relation column from prose, cannot scope a picker to the target folder, and nothing validates that targets exist (dangling references are silent).

## Canonical JSON Contract

> The section between the `CONTRACT-BEGIN` and `CONTRACT-END` markers is shared **verbatim** with the app PRD (`app/docs/prds/phase-42-frontmatter-relations.md`). Edit both together or not at all.

<!-- CONTRACT-BEGIN -->

### RelationValue

Emitted wherever a relation resolves — always inside arrays:

```jsonc
{
  "raw": "[[clients/acme|Acme]]",  // the literal frontmatter value (or list element)
  "path": "clients/acme.md",       // resolved root-relative path; null only if unresolvable (e.g. empty after fragment strip)
  "exists": true,                  // resolved path is present in the index (during full ingest: the discovered file set)
  "title": "Acme Corp",            // derived server-side via the phase-29 title rule on the target; null when !exists
  "frontmatter": { "...": "..." }  // ALWAYS-present key: object | null. null when !exists OR the target has no frontmatter.
}                                  // NEVER nested: a populated target's frontmatter never contains "relations".
```

### Populate surfaces

```jsonc
// mdvdb get <path> --populate — both keys ALWAYS present under --populate, absent (not null) without it:
"relations": { "client": [ /* RelationValue */ ] },   // map keyed by frontmatter field name, alphabetical; values always arrays
"referenced_by": [ { "source": "invoices/i1.md", "field": "client", "title": "Invoice i1" } ]  // sorted by (source, field); unbounded

// mdvdb collection <path> --populate:
//   rows[].relations = the same map, computed for the RETURNED PAGE rows only.
//   rows[].frontmatter stays the RAW object — populate never mutates it (phase-29 guarantee unchanged).

// mdvdb search --populate:
//   results[].file.relations = the same map. graph_context items are NOT populated.
```

### Schema and columns

```jsonc
// mdvdb schema --json fields[] and mdvdb collection --json columns[] gain:
"field_type": "Relation",         // FieldType set is now {"String","Number","Boolean","List","Date","Mixed","Relation"} — PascalCase
"relation_target": "clients"      // string | null. Overlay-declared FK target folder. NO trailing slash emitted.
                                  // (Overlay input accepts "clients" or "clients/"; output is always normalized slash-less.)
```

### Link graph

```jsonc
// Every LinkEntry (links/backlinks/neighborhood JSON) gains an ALWAYS-present key:
"field": "client"                 // string | null. null = body link; "client" = frontmatter relation from that field.
// Frontmatter-origin entries carry "line_number": 0 (sentinel — YAML parsing loses per-key line info).
// Invariant: field != null  ⇔  line_number == 0.

// GraphEdge (mdvdb graph --json edges[]) gains the same ALWAYS-present "field": string | null.
```

### Hard guarantees

- `relations` / `referenced_by` are present **iff** populate was requested — `{}` / `[]` when empty, never `null`; both keys absent entirely without populate.
- **Relation detection is value-driven**: a key appears in `relations` iff at least one value (or list element) is link-shaped — regardless of the schema's `field_type`. (Persisted schemas go stale after single-file ingest — phase-29 RESOLVED precedent — so schema-driven detection would silently miss fresh edits.) The schema contributes only the `"Relation"` label and `relation_target`. A schema-declared Relation field whose document has no link-shaped value produces **no key**; consumers distinguish "declared but empty" via column/field metadata.
- `relations` arrays preserve the source order of list elements and preserve duplicates; non-link elements in a mixed list are skipped (they produce no RelationValue).
- `RelationValue.frontmatter` is an always-present key, `object | null` — `null` when the target is missing **or** exists without frontmatter. (Deliberately different from the phase-29 row-level "frontmatter is always an object" rule, which continues to apply to `rows[].frontmatter`.)
- **Link-shaped is a whole-value predicate.** The entire trimmed string must be exactly one of: a wiki link `[[target]]` / `[[target|alias]]`; a markdown link `[text](target)` whose target is not external (`http(s)://`, `mailto:`) or a bare `#anchor`; or a bare vault path ending in `.md` with no whitespace. `"See [[x]] for details"` is NOT a relation (that is a body-link concern).
- **Resolution order for frontmatter relation targets** (body-link resolution is UNCHANGED — source-dir-relative):
  1. Target contains `/` → resolve **root-relative** (normalize `.`/`..`, append `.md` if missing); if that path is not in the index, fall back to source-dir-relative; if neither exists, the root-relative candidate is the reported `path` with `exists: false`.
  2. Else, if the field's schema/overlay declares a `target` folder → `target + "/" + name + ".md"`.
  3. Else → source-dir-relative (same as body links).
  No vault-wide basename search (nondeterministic with duplicate basenames — future work). `#fragment` is stripped; `\` normalizes to `/`; alias text is display-only.
- **Self-references are skipped** in both the link graph and `relations` (a self-FK is meaningless).
- A body link and a frontmatter relation to the same target are **two distinct edges** (graph dedup key is `(target, field)`); duplicate values within one field dedupe in the graph but are preserved in `relations`.
- Path matching against the index is **exact-case** (`[[Clients/Acme]]` does not match `clients/acme.md`) — documented limitation.
- `field_type` serialization stays PascalCase (no `rename_all`); all new JSON keys are additive.

<!-- CONTRACT-END -->

## Goals

- Extract frontmatter references (wiki links, markdown links, bare `.md` paths — whole-value only) during parsing and feed them into the link graph as `LinkEntry` edges tagged with their originating `field`, maintained by full ingest, single-file ingest, and the watcher.
- Auto-infer `FieldType::Relation` for fields whose values are link-shaped (scalars and lists), refinable via the overlay: `field_type: relation` + optional `target: <folder>`.
- `--populate` on `get`, `collection`, and `search`: depth-1 resolution of every relation value to a `RelationValue`; `get --populate` also returns `referenced_by`.
- Deterministic 3-step resolution order for frontmatter targets (root-relative → overlay target folder → source-dir-relative), leaving body-link semantics untouched.
- Relation-aware `Equals`/`In` filter matching so `--filter client=clients/acme`, `client=clients/acme.md`, and `client=[[clients/acme]]` all match `"[[clients/acme|Acme]]"` — in both `search` and `collection`.
- `doctor` gains a "Relations" check: dangling relation targets (Warn with examples), overlay `target:` folders that match no indexed folder, and the unquoted-`[[x]]` YAML footgun.
- Fix the overlay silent-ignore footgun minimally: accept `type:` as an alias for `field_type:`.
- No index VERSION bump — the rkyv layout change self-heals (delete + rebuild on open). Full test coverage (unit + integration + CLI golden JSON) and a clean `cargo clippy --all-targets`.

## Non-Goals

- **No markdown writes.** Creating, editing, or repairing relations (including rename propagation — see Migration notes) is the app's job. mdvdb never writes frontmatter.
- **No nested populate.** Depth 1 only; a populated target's frontmatter never carries `relations`. (A shallow populate variant — RelationValue without target `frontmatter` — is flagged as coordinated future work with the app PRD for collection-payload pressure; not in v1.)
- **No vault-wide basename resolution** for `[[acme]]` without an overlay target — nondeterministic with duplicate basenames; future work.
- **No populate of `graph_context` items** (`--expand` output) or of `links`/`backlinks` command output.
- **No semantic edges from frontmatter links.** Frontmatter references have no paragraph context; they are excluded from the semantic-edge extraction/embedding/clustering pipeline entirely.
- **No plain-bare-string relations.** `client: acme` (no `[[..]]`, no `.md` suffix) is never a relation, even if the overlay declares the field `relation` with a `target:`. Only the three locked syntaxes count. (Doctor may warn on suspicious values; value rewriting is the app's domain.)
- **No `relation_target` inference.** The target folder is overlay-declared only; inference from observed paths is future work.
- **No `item_type:` / `default:` overlay semantics.** Only the `type:` alias is added; the other historically-ignored keys remain documented-ignored (a doctor "overlay hygiene" warning covers folder targets only).
- **No new `MetadataFilter` variants and no resolved-path filter matching.** Relation matching is syntactic normalization on `Equals`/`In` (see Technical Design); full resolution-aware matching is future work.
- **No CLI flag to filter `mdvdb links` output by field.** JSON consumers filter on the new `field` key themselves.

## Technical Design

### Data Model Changes

**`src/schema.rs`:**

- `FieldType` (`src/schema.rs:10-19`) gains a `Relation` variant (appended last; serde derive stays default → PascalCase `"Relation"`).
- `SchemaField` (`src/schema.rs:67-84`) gains a trailing `pub relation_target: Option<String>` — serialized as `null` when absent (like `description`), stored slash-less.
- `OverlayField` (`src/schema.rs:36-46`) gains `pub target: Option<String>`, and `field_type` gains `#[serde(alias = "type")]`.
- `parse_field_type_str` (`src/schema.rs:147-157`) accepts `"relation"`.
- `infer_field_type` (`src/schema.rs:121-136`) gains relation rules ahead of the existing arms: a `Value::String` that is link-shaped → `Relation`; a non-empty `Value::Array` whose every element is a link-shaped string → `Relation`. **Field-level heuristic:** a field is typed `Relation` iff every non-null observed value classifies as `Relation` under these per-value rules; any other observed value makes the field `Mixed` via the existing multi-discriminant rule (or its plain type if no link-shaped value was ever seen). Empty arrays classify as `List` (no evidence) and therefore break Relation typing. This slots into the existing inference iteration with no new plumbing.
- `Schema::merge` (`src/schema.rs:339-400`) copies `target` through **both** branches (existing-field `:353-368`, overlay-only `:377-388`) into `relation_target`, normalizing away a trailing `/`. An overlay field with `target:` but no explicit `field_type` implies `FieldType::Relation`.

> **RESOLVED — overlay `type:` alias.** The live `test-vault/.markdownvdb.schema.yml` already writes `type:`, `item_type:`, and `default:` keys that are silently dropped (`OverlayField` has no `deny_unknown_fields`; only `field_type:` is read). This phase asks users to declare `field_type: relation` — the same users would write `type: relation` and conclude relations are broken. Fix minimally: `#[serde(alias = "type")]` on `field_type` (one line, fully backward compatible). Do **not** add `deny_unknown_fields` (would hard-break existing vaults) and do **not** implement `item_type:`/`default:` semantics (scope creep) — they remain documented-ignored.

**`src/parser.rs`:**

- New struct `FrontmatterLink { pub field: String, pub raw: String, pub target: String, pub text: String, pub is_wikilink: bool }` (derive style mirrors `RawLink`, `src/parser.rs:31-41`).
- `MarkdownFile` (`src/parser.rs:10-28`) gains `pub frontmatter_links: Vec<FrontmatterLink>` — deliberately **separate** from `links` so the semantic-edge pipeline (`extract_links_with_context`, `src/parser.rs:384`, driven off `file.links`) and chunking never see frontmatter references.
- New `pub fn extract_frontmatter_links(frontmatter: Option<&serde_json::Value>) -> Vec<FrontmatterLink>`: iterate top-level object entries; for `String` values and `String` elements of `Array` values, apply the whole-value link-shape predicate. `target` = the inner link target (wiki inner before `|` / markdown-link target / the bare string); `text` = alias or link text, falling back to the target; `raw` = the original string. Nested objects and non-string values are skipped. Called from `parse_markdown_file` right after `extract_frontmatter` (`src/parser.rs:70`).
- `is_external_or_anchor` (`src/parser.rs:398-404`) becomes `pub(crate)` (reused by the shape predicate).
- The wiki-link regex tolerance intentionally matches the existing body extractor (`src/parser.rs:271`) — aliases containing `]]` and markdown targets containing `)` share the same known limitations; parity over perfection.

**New module `src/relations.rs`** (declared in `lib.rs`; keeps the shape predicate, resolution, and contract types in one place):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RelationValue {
    pub raw: String,
    pub path: Option<String>,
    pub exists: bool,
    pub title: Option<String>,
    /// No skip attribute — serializes as null per the contract.
    pub frontmatter: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferencedBy {
    pub source: String,
    pub field: String,
    pub title: String,
}

/// Context for resolving frontmatter relation targets (graph build + populate).
pub struct RelationContext {
    pub known_files: std::collections::HashSet<String>,
    pub overlay: Option<schema::OverlaySchema>,
}
impl RelationContext {
    /// Overlay-declared target folder for (source file, field), resolved via
    /// resolve_overlay_for_path on the source's directory (src/schema.rs:306-336).
    /// Returned slash-less; cache per-directory resolution for large vaults.
    pub fn target_for(&self, source: &str, field: &str) -> Option<String>;
}

/// Whole-value link-shape predicate (single source of truth — schema inference,
/// parser extraction, and filter normalization all call this).
pub fn is_link_shaped(s: &str) -> bool;

/// Filter normalization: inner link target, #fragment stripped, '\'→'/',
/// trailing ".md" stripped. None if not link-shaped.
pub(crate) fn relation_key(s: &str) -> Option<String>;

/// The 3-step resolution order (contract section). Returns (resolved_path, exists).
/// None if the target is empty after fragment strip / trim.
pub fn resolve_relation_target(
    source: &str,
    target: &str,
    target_folder: Option<&str>,
    known_files: &std::collections::HashSet<String>,
) -> Option<(String, bool)>;
```

`links::normalize_path` (`src/links.rs:203-215`) becomes `pub(crate)` for the root-relative branch.

**`src/links.rs`:**

- `LinkEntry` (`src/links.rs:11-24`) gains a trailing `pub field: Option<String>` (rkyv trailing field; serde: always-present key, **no** `skip_serializing_if`). Doc comment: `None` = body link; `Some(field)` = frontmatter relation, whose `line_number` is `0`.
- `build_link_graph` (`src/links.rs:220-274`) and `update_file_links` (`src/links.rs:412-450`) gain a `ctx: &relations::RelationContext` parameter. After the body-link loop, iterate `file.frontmatter_links`: resolve via `resolve_relation_target(source, target, ctx.target_for(source, &fl.field).as_deref(), &ctx.known_files)`; skip empty and self-links. **The dedup key changes from `target` to `(target, field)`** so a body link and a frontmatter relation to the same target coexist, while duplicate values within one field dedupe. Push `LinkEntry { source, target, text: fl.text, line_number: 0, is_wikilink: fl.is_wikilink, field: Some(fl.field) }`. All existing `LinkEntry` constructions add `field: None`.
- Where an edge-id string is generated (`src/links.rs:165`), frontmatter edges use a field-qualified form (e.g. `edge:src->target@fm.client` instead of `@0`) so two different fields linking the same target cannot collide. Existing body-edge ids are unchanged.

**`src/lib.rs` (graph output):**

- `GraphEdge` (`src/lib.rs:422`) gains `pub field: Option<String>`, populated from `LinkEntry.field` at **both** construction sites that build edges from `link_graph.forward` entries (`src/lib.rs:2042` and `:2203`). **Serialization: always-present key — no `skip_serializing_if`.** This deliberately breaks the struct's local convention (every other `Option` on `GraphEdge` is skip-serialized): the contract requires `"field": null` on body/semantic edges, and the app mirrors the key as required, so omitting it would break the integration. Semantic edges and any other non-`forward`-derived edges set `field: None`.
- **Consumers need zero changes**: `compute_backlinks` (`:279`), `query_links` (`:355`), `find_orphans` (`:387`), `bfs_neighbors` (`:299`), `neighborhood` (`:468`), search link-boost (`src/search.rs:818-898`), and `expand_graph_context` (`src/search.rs:956-1115`) all consume `forward` edges and therefore see relations automatically. This is the payoff of making relations first-class edges — document the intended behavior changes explicitly: relation edges now contribute to link-boost and `--expand`, and documents referenced only via frontmatter stop being orphans.

### Graph-Build Wiring (ordering problem, resolved)

`build_link_graph` runs in the live ingest pipeline at `src/lib.rs:1320-1349` (full) and `update_file_links` at `:1308-1319` (single-file) — **before** the schema recompute at `:1534-1567`, so the persisted schema cannot supply `target` folders at build time.

> **RESOLVED:** call `schema::Schema::load_overlay(&self.root)` directly at each graph-build site. It is a cheap single-file read (`src/schema.rs:281-301`) that the ingest path already performs independently (`src/lib.rs:1536`) and the watcher performs too (`src/watcher.rs:322`). Build the `RelationContext` as:
>
> - **Full ingest** (`src/lib.rs:1338`): `known_files` = the discovered vault path set (all `.md`, forward slashes) — matching what body-link Valid/Broken classification uses downstream.
> - **Single-file ingest** (`src/lib.rs:1308-1319`): `known_files` = `self.index.get_file_hashes()` keys.
> - **Watcher change** (`src/watcher.rs:300-305`): same as single-file. **Drop the `if !file.links.is_empty()` gate and always call `update_file_links`** — this also fixes the pre-existing quirk where removing a file's last link left stale graph entries (update is remove-then-readd).
> - Watcher delete/rename (`src/watcher.rs:223-241`): no change — `remove_file_links` is path-keyed.
>
> `src/ingest.rs` never touches the link graph; all wiring is `lib.rs` + `watcher.rs`.

**Honest asymmetry note:** full ingest resolves `exists` against the discovered on-disk set; single-file/watcher paths resolve against indexed files. A relation to an on-disk-but-not-yet-indexed file can report a different `exists` between the two paths until the next full ingest — the same asymmetry body-link Valid/Broken already has. Documented, not fixed here.

### Populate (Interface Changes)

**Internal helpers on `MarkdownVdb`** (`src/lib.rs`, near `derive_title` `:2867`):

```rust
/// Value-driven: iterates frontmatter entries, resolves every link-shaped value/element.
/// Skips self-references. BTreeMap => alphabetical keys per the contract.
fn compute_relations(&self, source: &str, frontmatter: &serde_json::Value,
                     ctx: &relations::RelationContext)
                     -> std::collections::BTreeMap<String, Vec<relations::RelationValue>>;

/// Relation backlinks: compute_backlinks(graph) entries with field.is_some()
/// targeting `path`. Absent graph => Ok(vec![]) — populate must not fail on fresh vaults.
fn compute_referenced_by(&self, path: &str) -> Result<Vec<relations::ReferencedBy>>;

/// get_file_hashes() keys + Schema::load_overlay. Build ONE per populate call.
fn relation_context(&self) -> relations::RelationContext;
```

`compute_relations` builds each `RelationValue` as: `raw` = the original string; `path`/`exists` from `resolve_relation_target`; `title` = `derive_title` on the target's stored frontmatter when `exists` (never empty, phase-29 rule); `frontmatter` = the target's parsed stored frontmatter (`serde_json::from_str` of `StoredFile.frontmatter`, degrade to `None` → `null` on parse failure — no panics). Unresolvable target → `path: None, exists: false, title/frontmatter: null`. `compute_referenced_by` derives `title` via `derive_title` on the **source** document and sorts `(source, field)`.

**`get`:** `DocumentInfo` (`src/lib.rs:201-216`) gains two trailing fields with `#[serde(skip_serializing_if = "Option::is_none")]`: `relations: Option<BTreeMap<String, Vec<RelationValue>>>` and `referenced_by: Option<Vec<ReferencedBy>>`. New method `pub fn get_document_populated(&self, relative_path: &str) -> Result<DocumentInfo>` = `get_document` (`:2346-2370`, output otherwise byte-identical) + both fields set to `Some(..)` (empty map/vec allowed; a document with no frontmatter gets `relations: Some(BTreeMap::new())`).

**`collection`:** `CollectionQuery` (`src/lib.rs:229-245`) gains `pub populate: bool`. `CollectionRow` (`:291-311`) gains `relations: Option<..>` (skip-if-none). In `collection()` (`:2386-2436`), **after pagination**: if populate, build one `relation_context()` and fill `relations` for the returned page rows only — never the full filtered set. Deleted/new rows compute from whatever frontmatter the row carries (`{}` → empty map). `CollectionColumn` (`:269-288`) gains `pub relation_target: Option<String>`, copied from `SchemaField` in `build_collection_columns` (`:2566-2619`); present-but-unscoped columns get `None`.

**`search`:** `SearchQuery` (`src/search.rs:93-120`) gains `pub populate: bool` (constructor default false + `with_populate` builder). `SearchResultFile` (`:267-278`) gains `relations: Option<..>` (skip-if-none). The engine (`search::search`) ignores the flag; the `MarkdownVdb::search` wrapper (`src/lib.rs:1621-1643`) post-processes: if `query.populate`, build one `relation_context()` and fill each `results[i].file.relations` from the result file's frontmatter (already carried on `SearchResultFile`). This avoids threading overlay/index context through the engine signature. `graph_context` untouched.

**Re-exports** (`src/lib.rs`, alongside `CollectionQuery` etc.): `RelationValue`, `ReferencedBy`.

**Performance notes (honest):** collection populate is page-scoped by design; search populate is limit-bounded. `compute_referenced_by` runs `compute_backlinks` — O(edges) per `get --populate`, the same cost class as the existing `links`/`backlinks` commands. Graph build adds an overlay-scope resolution per frontmatter link; `RelationContext::target_for` should cache per-directory resolution for vaults with thousands of relation links. The `(target, field)` dedup key grows the graph slightly (body + relation duplicates) — intended; existing test fixtures asserting edge counts may shift.

### New Commands / CLI Surface

No new subcommands — one new flag on three existing ones (`src/main.rs`):

- `GetArgs` (`:337-340`): `#[arg(long)] populate: bool`; handler (`:1066-1076`) branches to `get_document_populated`.
- `CollectionArgs` (`:342-371`): `#[arg(long)] populate: bool`; handler (`:1078-1103`) passes it into `CollectionQuery`.
- `SearchArgs` (`:172-243`): `#[arg(long)] populate: bool`; the search handler sets `query.populate`.

Help text: "Resolve frontmatter relations (.md paths, wiki links, or Markdown links) inline: path, existence, title, target frontmatter". Shell completions regenerate via clap automatically.

### Relation-Aware Filter Matching

In `evaluate_single_filter` (`src/search.rs:1242-1291`), extract a helper `fn filter_values_equal(field_value: &Value, filter_value: &Value) -> bool` used by `Equals` (scalar + array-contains) and `In`:

1. Exact `==` first — zero regression for every existing filter.
2. Else, if both are strings and `relation_key(field_str)` is `Some(k)`: match iff `k == coerce(filter_str)`, where `coerce(v)` = `relation_key(v)` if link-shaped, else `v` with a trailing `.md` stripped.

So `--filter client=clients/acme`, `client=clients/acme.md`, and `client=[[clients/acme]]` all match `client: "[[clients/acme|Acme]]"`. This is **purely syntactic** — `evaluate_filters` has no source path, so no source-dir or target-folder resolution happens. Documented limitation: `[[acme]]` under a `target: clients` overlay matches `--filter client=acme`, **not** `client=clients/acme`; resolved matching is future work. `Range`/`Exists` are unchanged. Both `search` and `collection` get this via the shared `evaluate_filters`.

### Doctor: "Relations" Check

In `doctor()` (`src/lib.rs:2654-2836`), after Source directories, before the tally (`:2828`) — one new `DoctorCheck` named `"Relations"`:

- Load the link graph; collect `forward` entries with `field.is_some()` whose `target` is not in `get_file_hashes()` keys. No graph / zero relation edges → Pass (`"no relation links"`); all resolve → Pass (`"{n} relation link(s), all targets resolve"`); else **Warn** — `"{k} dangling relation(s): invoices/i1.md#client → clients/ghost.md, …"` (cap 5 examples, `+N more`). Warn, not Fail: broken references are vault content, not index health.
- **Overlay hygiene:** Warn when an overlay `target:` folder prefixes no indexed file (e.g. `target: ghostfolder`).
- **Unquoted-`[[x]]` footgun:** Warn when a frontmatter value parses as a nested single-string array (`[["clients/acme"]]`) whose inner string would be link-shaped if the value had been quoted — the YAML signature of an unquoted `client: [[clients/acme]]`. Detail suggests quoting.

`print_doctor` badges (`src/format.rs:1239-1241`) need no change.

### Human-Readable Output (`src/format.rs`)

- Field-type match arms in `print_schema` (`:556-562`) and `print_collection` (`:639-645`): `FieldType::Relation => "relation"`; in `print_schema`, a declared target renders as a dimmed `relation → clients` suffix.
- `print_document`: when `relations` is present, a "Relations" section — `client → clients/acme.md  ✓ Acme Corp` (or `✗ missing`); when `referenced_by` is non-empty, a "Referenced by" section — `invoices/i1.md  (client)  Invoice i1`.
- `print_links` / `print_backlinks`: entries with `field: Some(f)` get a dimmed `(f)` tag after the target.
- Search human output: unchanged — populate is a JSON-consumer feature (state this explicitly).

### Migration Strategy

**`VERSION` stays `1`** (`src/index/storage.rs:16`) — explicitly locked; do NOT bump. The rkyv archived layouts of `FieldType` (new variant), `SchemaField`, and `LinkEntry` (new trailing fields) DO change, and all three live inside the persisted `IndexMetadata` (`link_graph` at `src/index/types.rs:79`, `schema`/`scoped_schemas` at `:75`/`:84`). An old index file fails rkyv validated deserialization → `Error::IndexCorrupted` → the established self-heal: `open_or_create_with_options` (`src/index/state.rs:178-190`) deletes the file and recreates an empty index. **Both open paths self-heal** — write-mode (`src/lib.rs:621`) and read-only (`src/lib.rs:678`) — so the **first command after upgrading silently resets the index and returns empty results** until the user runs `mdvdb ingest` (one-time full re-embed; identical cost to a version bump, same UX as prior layout changes — `semantic_edges`, `scoped_schemas`). `MarkdownFile`/`FrontmatterLink` are not persisted — no impact. All JSON contract changes are additive; the phase-29 golden test's pinned `field_type` set gains `"Relation"` and columns gain `relation_target`.

**Rename handling is stale-by-design (explicit note):** mdvdb never rewrites markdown, so renaming `clients/acme.md` dangles every `client: "[[clients/acme]]"` — relation edges go Broken, `exists: false`, doctor Warns. The watcher rename path only fixes the renamed file's own outgoing edges. `referenced_by` + the doctor check are exactly the primitives an app-side "repair references" flow needs; that flow is the app's future work, not this phase.

**Schema staleness note:** single-file ingest does not recompute persisted schemas (phase-29 RESOLVED), so a `field_type: "Relation"` column label can lag until the next full ingest — same accepted behavior as phase-29's `in_schema:false`. Populate is immune (value-driven detection), and overlay `target:` resolution reads the overlay file live.

## Implementation Steps

1. **`src/relations.rs` module** — `is_link_shaped`, `relation_key`, `resolve_relation_target`, `RelationValue`, `ReferencedBy`, `RelationContext::target_for` (with per-directory cache); make `links::normalize_path` and `parser::is_external_or_anchor` `pub(crate)`. Unit tests in-file: shape-predicate matrix (wiki/alias/md/bare/embedded-text/whitespace/external/anchor), resolution-order matrix (root-hit, root-miss-source-fallback, target-folder, bare-no-target, fragment strip, self, empty), `relation_key` normalization.
2. **Parser extraction** — `FrontmatterLink`, `MarkdownFile.frontmatter_links`, `extract_frontmatter_links`, wired into `parse_markdown_file` (`src/parser.rs:70-84`). Unit tests: wiki/alias/md/bare/array/mixed-array-skips-non-links/nested-object-skip/external-skip/whole-value strictness/unquoted-`[[x]]`-is-not-a-relation.
3. **Schema typing** — `FieldType::Relation`; `infer_field_type` relation rules; `parse_field_type_str` `"relation"`; `OverlayField.target` + `#[serde(alias = "type")]`; `SchemaField.relation_target`; `merge` copies + normalizes target, target-implies-Relation. Unit tests: all-link → Relation; mixed → Mixed; all-link lists → Relation; empty array breaks; overlay overrides both directions; `type:` alias honored; slash normalization.
4. **`LinkEntry.field` + graph build** — new field; `build_link_graph`/`update_file_links` take `&RelationContext`; dedup key `(target, field)`; relation entries `line_number: 0`; field-qualified edge ids. Wire `field` into `GraphEdge` (`src/lib.rs:422`) and both construction sites (`:2042`, `:2203`), serialized always-present (no skip attribute — see Data Model). Update call sites: `src/lib.rs:1317`/`1338` (ctx from `load_overlay` + discovered set / file-hash keys), `src/watcher.rs:300-305` (drop the empty-links gate), all test constructors (`field: None`). Unit tests: tagged relation edge; body+relation to same target both kept; target-folder resolution at build time; frontmatter links never produce semantic edges; self-link skipped; last-link-removed updates the graph; `mdvdb graph --json` edges carry `field: null` for body/semantic edges.
5. **Populate core** — `relation_context`, `compute_relations`, `compute_referenced_by`, `get_document_populated`; `DocumentInfo` fields. Covered by integration tests (step 9).
6. **Collection + search populate** — `CollectionQuery.populate`, `CollectionRow.relations`, `CollectionColumn.relation_target` (via `build_collection_columns`), page-only population; `SearchQuery.populate` + builder, `SearchResultFile.relations`, wrapper post-processing in `MarkdownVdb::search` (`src/lib.rs:1621`).
7. **Relation-aware filters** — `filter_values_equal` in `src/search.rs` using `relation_key`; unit tests including no-regression cases (plain strings, numbers, arrays, dates).
8. **Doctor + CLI flags + format** — "Relations" check (dangling + overlay hygiene + unquoted footgun) before `src/lib.rs:2828`; `--populate` on Get/Collection/Search args + handlers; `format.rs` arms and sections; re-exports.
9. **Integration tests** — new `tests/relations_test.rs` (per project conventions: `mock_config()` + `EmbeddingProviderType::Mock` (8 dims) + `tempfile::TempDir`). Fixture vault: `clients/acme.md` (frontmatter title), `clients/globex.md`, `invoices/i1.md` (`client: "[[clients/acme]]"`), `invoices/i2.md` (`clients: ["[[clients/acme|A]]", "[[clients/globex]]"]` plus md-link and bare-path variants), `notes/dangling.md` (`client: "[[clients/ghost]]"`), self-reference case, unquoted-`[[x]]` case, overlay with `scopes: invoices: fields: client: {field_type: relation, target: clients}`. Assert every Validation Criteria bullet below.
10. **CLI golden tests** — extend `tests/cli_test.rs`: golden JSON pinning `relations`/`referenced_by`/`RelationValue` key names, `"Relation"` PascalCase, `relation_target` (slash-less), `LinkEntry.field` + `line_number: 0`, `GraphEdge.field`; array order/duplicates preservation; absence-without-flag assertions; update the phase-29 collection golden for the new column key and FieldType set.
11. **Docs** — README/CLI help snippets; ROADMAP entry; cross-reference the app PRD. Phase-29's PRD is not edited (the contract is additive).

## Validation Criteria

- [ ] `cargo test` and `cargo clippy --all-targets` pass clean.
- [ ] `get --populate` returns `relations` (always a map) + `referenced_by` (always an array); both keys absent without the flag; `get` output without the flag is byte-identical to pre-phase-31.
- [ ] `RelationValue` matches the contract exactly: `raw`/`path`/`exists`/`title`/`frontmatter`; `frontmatter` key always present, `null` for missing targets and frontmatter-less targets; never nested relations.
- [ ] Resolution order verified: `[[clients/acme]]` root-relative; root-miss falls back source-dir-relative; `[[acme]]` + overlay `target: clients` → `clients/acme.md`; `[[acme]]` without target → source-dir; `[[x#frag|alias]]` fragment stripped, alias as display text only.
- [ ] Relations arrays preserve source order and duplicates; non-link elements in mixed lists produce no RelationValue (golden-tested).
- [ ] Self-references appear in neither the graph nor `relations`.
- [ ] Schema infers `Relation` for all-link fields and lists-of-links; mixed values → `Mixed`; overlay `field_type: relation` + `target:` refine; `type:` alias works; `relation_target` (slash-less) appears on `schema --json` and collection columns.
- [ ] `links`/`backlinks --json`: relation edges present with `field` set and `line_number: 0`; body links carry `field: null`; a frontmatter-only-referenced doc is not an orphan; body + relation to the same target = two entries; `mdvdb graph --json` edges carry `field`.
- [ ] `--filter client=clients/acme`, `=clients/acme.md`, and `=[[clients/acme]]` all match `"[[clients/acme|Acme]]"` in both `search` and `collection`; non-link filtering behavior is unchanged.
- [ ] `collection --populate`: `relations` on page rows only; `rows[].frontmatter` stays raw; `total_rows` unaffected by populate.
- [ ] `search --populate`: `results[].file.relations` present; `graph_context` unpopulated; relation edges participate in link-boost and `--expand`.
- [ ] `doctor` Relations check: Pass on a clean vault; Warn listing `notes/dangling.md#client → clients/ghost.md`; Warn on a `target:` folder matching no indexed file; Warn on unquoted `[[x]]` nested-array values.
- [ ] `VERSION == 1`; opening a pre-phase-31 index self-heals (deleted + recreated, no panic) on both write and read-only paths; after `mdvdb ingest` all features work.
- [ ] Golden JSON pins all new field names and casing, including `"Relation"` PascalCase and always-present `field`/`frontmatter` keys.
- [ ] Watcher: editing a file's frontmatter link updates the graph incrementally, including removing the last link.
- [ ] No markdown file is ever written by any code path in this phase.

## Anti-Patterns to Avoid

- **Do NOT write markdown or add a frontmatter writer.** CLAUDE.md hard rule; relation creation/repair (including rename propagation) is the app's job. Even a "convenience" writer violates the architecture.
- **Do NOT bump the index VERSION.** Explicitly declined; the self-heal on `IndexCorrupted` (`src/index/state.rs:178-190`) is the established migration path.
- **Do NOT add `#[serde(rename_all)]` to `schema::FieldType`.** PascalCase is the shipped contract (`mdvdb schema --json`, phase-29 collection columns).
- **Do NOT merge frontmatter links into `MarkdownFile.links`.** The semantic-edge pipeline (`extract_links_with_context`) consumes `links` with body paragraph context; frontmatter links have none and would generate garbage semantic edges. Keep the separate `frontmatter_links` vec.
- **Do NOT change body-link resolution** (`links::resolve_link` semantics). Body links stay source-dir-relative; only frontmatter relations get the 3-step order. Anything else silently retargets existing graphs.
- **Do NOT drive query-time relation detection off the persisted schema.** Single-file ingest skips schema recomputation (phase-29 RESOLVED note), so schema-driven detection goes stale immediately after an edit. Detection is value-driven; the schema only labels and scopes.
- **Do NOT populate recursively or populate the full filtered collection set.** Depth 1 is locked; populate page rows only — otherwise a 10k-row vault does 10k target lookups per call.
- **Do NOT implement vault-wide basename resolution.** Nondeterministic with duplicate basenames; explicitly deferred.
- **Do NOT reimplement filter/title/schema logic.** Extend `evaluate_single_filter`, reuse `derive_title` (`src/lib.rs:2867`), reuse `infer_field_type`. Divergent semantics between `search` and `collection` is exactly what phase 29 eliminated.
- **Do NOT keep the single-`target` graph dedup key.** It would silently drop either the body link or the tagged relation edge to the same target.
- **Do NOT `unwrap()` in library code; thiserror in lib, tracing not println.** Frontmatter parse failures degrade to `null`; an absent graph degrades to empty `referenced_by`, never an error on populate paths.
- **Do NOT skip-serialize `LinkEntry.field`, `GraphEdge.field`, or `RelationValue.frontmatter`.** The contract requires always-present keys (`| null`); the app's type mirrors treat key absence differently from `null`. `GraphEdge` is the trap: every other `Option` on it carries `skip_serializing_if` — do not copy that local convention onto `field`.

## Patterns to Follow

- **Canonical-contract + hard-guarantees PRD structure** — `docs/prds/phase-29-collection-folder-table.md` (its RESOLVED callouts, honest perf notes, cross-repo contract discipline); overlay/scoping precedent — `docs/prds/phase-23-scoped-schema.md`.
- **rkyv-archived struct evolution** — `LinkGraph.semantic_edges`/`edge_cluster_state` (`src/links.rs:77-88`): trailing `Option` fields + self-heal on layout breaks.
- **Response-struct style + `skip_serializing_if`** — `SearchResponse` (`src/search.rs:348-359`), `CollectionResponse` (`src/lib.rs:249-266`).
- **CLI flag + handler shape** — `Commands::Get`/`Collection` handlers (`src/main.rs:1066-1103`); `parse_filter` (`:480-499`).
- **Overlay load/resolve/merge** — `src/schema.rs:281-400`; scoped ingest pipeline `src/lib.rs:1534-1567`.
- **Title derivation single source of truth** — `derive_title` (`src/lib.rs:2867`) and its unit tests.
- **Doctor check shape** — existing checks `src/lib.rs:2654-2826` (`DoctorCheck { name, status, detail }`, Warn semantics).
- **Testing conventions** — `tests/collection_test.rs`, `tests/links_test.rs`, `tests/cli_test.rs` golden style; `mock_config()` + `EmbeddingProviderType::Mock` (8 dims) + `tempfile::TempDir`.

## Cross-Repo Coordination

App counterpart: `app/docs/prds/phase-42-frontmatter-relations.md`. The app mirrors the contract types in `types/cli.ts`, gates all relation UI on the CLI version shipping this phase, and owns every write-side concern (picker, cell editing, target-folder annotation via its overlay writer, future rename-repair). Coordinated future work flagged in both PRDs: a **shallow populate** variant (RelationValue without target `frontmatter`) if collection payload pressure materializes in large tables.
