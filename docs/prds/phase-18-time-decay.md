# PRD: Phase 18 — Time Decay for Search Results

## Overview

Add an optional time-based decay multiplier to search scores, favoring more recently edited files. Decay is computed from the file's filesystem modification time (mtime) using an exponential half-life formula: `score * 0.5^(elapsed_days / half_life_days)`. Disabled by default — can be enabled per-config or per-query, with an adjustable half-life parameter (default 90 days). Requires storing file modification timestamps in the index during ingestion.

## Problem Statement

All search results in mdvdb are currently ranked purely by relevance (cosine similarity, BM25, or RRF fusion). In active knowledge bases, recently edited files are often more pertinent than stale ones — a project's current architecture docs should rank above a three-year-old design doc with similar semantic content. There is no mechanism to incorporate recency into search ranking.

Users managing evolving documentation, research notes, or project specs need the ability to soft-boost recent content without discarding older results entirely. A configurable decay curve gives users control over how aggressively freshness is weighted.

## Goals

- Capture and store filesystem modification time (`mtime`) for each indexed file
- Implement exponential time decay with a configurable half-life (default 90 days)
- Opt-in by default — existing users see zero behavior change
- Per-config enable/disable via `MDVDB_SEARCH_DECAY` env var
- Per-query override via `--decay` / `--no-decay` CLI flags
- Per-query half-life override via `--decay-half-life` CLI flag and `SearchQuery` builder
- Works identically across all three search modes (semantic, lexical, hybrid)
- Scores remain in `[0, 1]` — decay only reduces, never increases
- Backward-compatible index format (no version bump, no forced re-ingest)
- Surface `modified_at` timestamp in search results JSON output

## Non-Goals

- Boosting scores above base relevance (decay is strictly a penalty, multiplier in `(0, 1]`)
- Using frontmatter `date` fields for decay — only filesystem mtime
- Per-file or per-directory decay configuration — one half-life applies uniformly
- Negative decay (boosting old content)
- Index version bump — uses `Option<HashMap>` on `IndexMetadata` for backward compat
- Automatic decay presets (e.g., "fast decay for news, slow for docs")

## Technical Design

### Decay Formula

Exponential half-life decay — score is halved every `half_life_days`:

```
decay_multiplier = 0.5^(elapsed_days / half_life_days)
```

Where:
- `elapsed_days = (now_unix_secs - modified_at_unix_secs) / 86400.0`
- `half_life_days` = configurable, default 90.0
- `decay_multiplier` is always in `(0, 1]` (1.0 when elapsed_days = 0)

| Age | Multiplier | Effect |
|-----|-----------|--------|
| 0 days | 1.000 | No penalty |
| 30 days | 0.794 | ~21% reduction |
| 90 days | 0.500 | Halved |
| 180 days | 0.250 | Quartered |
| 365 days | 0.060 | ~6% of original |

### Data Model Changes

**`MarkdownFile` in `src/parser.rs`** — add field:

```rust
pub modified_at: u64,  // NEW: filesystem mtime as Unix timestamp
```

Non-serialized struct (in-memory only during parsing), no backward compat concern.

**`IndexMetadata` in `src/index/types.rs`** — add field:

```rust
pub file_mtimes: Option<HashMap<String, u64>>,  // NEW: path -> mtime
```

Follows the same `Option<T>` pattern used for `schema`, `cluster_state`, and `link_graph`. Old indices deserialize with `None` — no version bump needed, no forced re-ingest.

**`SearchResultFile` in `src/search.rs`** — add field:

```rust
pub modified_at: Option<u64>,  // NEW: exposed in JSON output
```

**`DocumentInfo` in `src/lib.rs`** — add field:

```rust
pub modified_at: Option<u64>,  // NEW
```

### Config Changes

Two new fields in `Config` (`src/config.rs`):

| Field | Env Var | Default | Type | Validation |
|---|---|---|---|---|
| `search_decay_enabled` | `MDVDB_SEARCH_DECAY` | `false` | `bool` | — |
| `search_decay_half_life` | `MDVDB_SEARCH_DECAY_HALF_LIFE` | `90.0` | `f64` | Must be > 0 |

### SearchQuery Changes

Two new fields + builders on `SearchQuery`:

```rust
pub decay: Option<bool>,          // Per-query override (None = use config)
pub decay_half_life: Option<f64>, // Per-query half-life override (days)
```

Builder: `with_decay(bool)`, `with_decay_half_life(f64)`.

### Search Pipeline Integration

Update `search()` signature to accept `decay_enabled: bool` and `decay_half_life: f64` from config.

Decay applied inside `assemble_results()`, after file metadata lookup, before `min_score` check:

```rust
fn apply_time_decay(score: f64, modified_at: u64, half_life_days: f64, now: u64) -> f64 {
    let elapsed_secs = now.saturating_sub(modified_at) as f64;
    let elapsed_days = elapsed_secs / 86400.0;
    let multiplier = 0.5_f64.powf(elapsed_days / half_life_days);
    score * multiplier
}
```

**Order in assemble_results:**
1. Look up chunk metadata → path prefix filter
2. Look up file metadata (has `modified_at` via `file_mtimes`)
3. **Apply time decay** (if enabled)
4. Apply `min_score` filter (on decayed score)
5. Apply metadata filters → build `SearchResult`
6. Link boosting (post-assembly, as today)

### CLI Changes

Three new flags on `search` subcommand:

```
--decay                    Enable time decay for this search
--no-decay                 Disable time decay (conflicts with --decay)
--decay-half-life <DAYS>   Half-life in days (how many days until score halved)
```

### Migration Strategy

- **No index version bump.** `file_mtimes: Option<HashMap<String, u64>>` on `IndexMetadata` — old indices deserialize with `None`.
- Data populates incrementally: each file gets mtime on next ingest.
- Decay falls back to `indexed_at` when mtime unavailable.

## Implementation Steps

0. **Save PRD & update roadmap** — Write this PRD to `docs/prds/phase-18-time-decay.md`. Update `docs/prds/ROADMAP.md` to add Phase 18 under a new Sprint 3 section with dependency on Phase 14 (hybrid search).

1. **Parser: capture mtime** — `src/parser.rs`: Add `modified_at: u64` to `MarkdownFile`. In `parse_markdown_file()`, call `std::fs::metadata(&full_path)?.modified()` → Unix timestamp. Update all test helpers constructing `MarkdownFile` to include `modified_at: 0`.

2. **IndexMetadata: add mtime storage** — `src/index/types.rs`: Add `pub file_mtimes: Option<HashMap<String, u64>>` to `IndexMetadata`. Update `Index::create()` in `src/index/state.rs` to init as `Some(HashMap::new())`.

3. **Index state: mtime read/write** — `src/index/state.rs`: In `upsert()`, store mtime in `file_mtimes`. Add `get_file_mtime(&self, path: &str) -> Option<u64>`. In `remove_file()`, also remove from `file_mtimes`.

4. **Config: decay settings** — `src/config.rs`: Add `search_decay_enabled: bool` (env: `MDVDB_SEARCH_DECAY`, default: false) and `search_decay_half_life: f64` (env: `MDVDB_SEARCH_DECAY_HALF_LIFE`, default: 90.0). Validate half-life > 0.

5. **SearchQuery: decay fields** — `src/search.rs`: Add `decay: Option<bool>` and `decay_half_life: Option<f64>`. Add `with_decay()` and `with_decay_half_life()` builders.

6. **Search engine: implement decay** — `src/search.rs`: Add `apply_time_decay()` function. Update `search()` signature. Resolve per-query overrides. Apply decay in `assemble_results()` after file lookup, before `min_score`. Pass `now` as parameter for testability.

7. **SearchResultFile + DocumentInfo** — Add `modified_at: Option<u64>` to both. Populate from `file_mtimes` lookup.

8. **Library API** — `src/lib.rs`: Wire `config.search_decay_enabled` and `config.search_decay_half_life` to `search::search()`. Update `get_document()` to include `modified_at`.

9. **CLI flags** — `src/main.rs`: Add `--decay`, `--no-decay` (conflicts), `--decay-half-life <DAYS>`. Wire to `SearchQuery`. Update shell completions.

10. **Watcher** — `src/watcher.rs`: No code changes needed — already calls `parse_markdown_file()` + `upsert()`, which now capture and store mtime automatically. Verify in test.

11. **Tests** — Comprehensive coverage:
    - Unit: `apply_time_decay()` (zero age, half-life, very old, edge cases)
    - Unit: `SearchQuery` builder for decay fields
    - Integration (`tests/search_test.rs`): Two files at different mtimes, decay reorders them
    - Config (`tests/config_test.rs`): Defaults, validation rejects zero/negative half-life
    - API (`tests/api_test.rs`): Ingest + search with decay, `modified_at` in results
    - CLI (`tests/cli_test.rs`): Flag acceptance, conflicts, JSON output
    - Index (`tests/index_test.rs`): Mtime stored/retrieved via `get_file_mtime()`

12. **Documentation** — Update `CLAUDE.md` config table, CLI flags, re-exports.

## Files Modified

| File | Change |
|---|---|
| `src/parser.rs` | Add `modified_at: u64` to `MarkdownFile`, capture mtime |
| `src/index/types.rs` | Add `file_mtimes: Option<HashMap<String, u64>>` to `IndexMetadata` |
| `src/index/state.rs` | Store/retrieve mtimes in `upsert()`, `remove_file()`, add `get_file_mtime()` |
| `src/config.rs` | Add `search_decay_enabled`, `search_decay_half_life` + env parsing + validation |
| `src/search.rs` | Decay fields on `SearchQuery`, `apply_time_decay()`, update `search()` + `assemble_results()`, `modified_at` on `SearchResultFile` |
| `src/lib.rs` | Wire decay config, `modified_at` on `DocumentInfo`, update `get_document()` |
| `src/main.rs` | `--decay`, `--no-decay`, `--decay-half-life` flags, wire to SearchQuery, completions |
| `tests/*.rs` | Update helpers, add decay-specific tests across all test files |
| `CLAUDE.md` | Config table, CLI flags, re-exports |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] Decay disabled by default: `mdvdb search "query"` returns identical results to before
- [ ] `mdvdb search "query" --decay` applies decay, reducing old file scores
- [ ] `mdvdb search "query" --no-decay` disables even when config enables it
- [ ] `mdvdb search "query" --decay --decay-half-life 30` uses 30-day half-life
- [ ] `--decay` and `--no-decay` conflict (clap error)
- [ ] All three modes (hybrid, semantic, lexical) apply decay identically
- [ ] Scores remain in `[0, 1]` — never negative, never exceed base score
- [ ] File modified 0 days ago → multiplier 1.0 (no penalty)
- [ ] File modified 90 days ago (default) → multiplier ~0.5
- [ ] `modified_at` in search JSON output and `mdvdb get --json`
- [ ] Old indices load correctly (`file_mtimes` = None, fallback to `indexed_at`)
- [ ] After ingest, `modified_at` populated for all files
- [ ] `MDVDB_SEARCH_DECAY_HALF_LIFE=0` and `-1` rejected by validation
- [ ] Link boosting + decay compose correctly (decay before boost)

## Anti-Patterns to Avoid

- **Do not add `modified_at` directly to `StoredFile`** — Changes rkyv layout, breaks old indices. Use `file_mtimes: Option<HashMap>` on `IndexMetadata` instead (same pattern as schema/clusters/link_graph).

- **Do not bump index version** — The `Option<HashMap>` approach avoids forcing re-ingest on all users.

- **Do not apply decay before score normalization** — BM25 saturation and RRF normalization expect raw scores. Decay must be post-normalization.

- **Do not apply decay at candidate level** — Candidates are `(chunk_id, score)` without file metadata. Decay needs `modified_at` from file lookup in `assemble_results()`.

- **Do not use `SystemTime::now()` in decay calculation** — Pass `now` as a `u64` parameter for deterministic testing.

- **Do not allow half-life of zero** — Division by zero / zeroes all scores. Validate `> 0.0`.

- **Do not make decay opt-out (enabled by default)** — Would change behavior for all existing users on upgrade.

## Patterns to Follow

- **Optional IndexMetadata fields:** `schema: Option<Schema>`, `cluster_state: Option<ClusterState>`, `link_graph: Option<LinkGraph>` in `src/index/types.rs:75-79`
- **SearchQuery builder:** `with_boost_links()`, `with_mode()` in `src/search.rs:86-119`
- **CLI flag conflicts:** `--semantic` / `--lexical` pattern in `src/main.rs`
- **Score modification:** Link boosting in `src/search.rs:372-416`
- **Config field naming:** `MDVDB_SEARCH_*` pattern in `src/config.rs`
- **Test helpers:** `mock_config()` in `tests/api_test.rs`, `fake_markdown_file()` in `tests/search_test.rs`
