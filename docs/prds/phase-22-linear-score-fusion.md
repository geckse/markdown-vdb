# PRD: Phase 22 — Linear Score Fusion for Hybrid Search

## Overview

Replace Reciprocal Rank Fusion (RRF) with weighted linear score interpolation for hybrid search: `final(d) = α × semantic(d) + (1-α) × bm25_norm(d)`. Default α=0.7 (semantic-heavy), configurable via `MDVDB_SEARCH_SEMANTIC_WEIGHT`. This fixes a regression where hybrid search underperforms pure semantic search because RRF discards score magnitudes.

## Problem Statement

Hybrid search (semantic + BM25 fused via RRF) currently **underperforms** pure semantic search in benchmarks:

| Metric | Semantic | Hybrid (RRF) | Delta |
|--------|----------|-------------|-------|
| Recall | 85% | 82% | -3% |
| MRR | 79% | 73% | -6% |
| Precision@10 | 24% | 23% | -1% |
| Perfect recall | 17/25 | 16/25 | -1 |

RRF (`1/(k + rank)`) is purely rank-based — it discards how *good* a score is. A document ranked #10 in both lists outranks the #1 semantic-only result:

```
RRF #10 in both: 2 × 1/(60+10) = 0.0286
RRF #1 semantic only: 1/(60+1) = 0.0164
```

This rewards overlap across methods rather than quality. Documents that are merely "ok" in both lists outrank excellent semantic-only hits. This has been observed in hybrid retrieval surveys: RRF is robust and simple, but underperforms well-tuned score fusion when one signal (dense/semantic) is strictly stronger.

## Goals

- Replace RRF with linear score interpolation for hybrid mode
- Default α=0.7 (semantic-heavy) — configurable per-config and per-query
- Hybrid search recall ≥ pure semantic search recall
- Scores remain in [0, 1] — both inputs normalized before fusion
- Backward-compatible: no index format changes
- Configurable via `MDVDB_SEARCH_SEMANTIC_WEIGHT` env var
- Per-query override via `SearchQuery::with_semantic_weight(alpha)`

## Non-Goals

- Learned/dynamic per-query weighting (too complex for current use case)
- Semantic floor approach (misses good pure-lexical hits for IDs, rare tokens)
- Weighted RRF (still blind to absolute score gaps)
- Auto-tuning α from benchmark data
- Changes to the BM25 saturation normalization formula or constant

## Evidence & Literature

| Fusion Scheme | Uses Scores? | Verdict |
|--------------|-------------|---------|
| Plain RRF (current) | No | Over-rewards overlap, ignores score gaps — **our problem** |
| Weighted RRF | No | Still blind to absolute score gaps |
| **Linear interpolation** | **Yes** | **Strong empirical gains, interpretable α, standard in literature** |
| Semantic floor (BM25 booster) | Mixed | Misses good pure-lexical hits (IDs, rare tokens) |
| Learned weighting | Yes | Best potential but too complex |

Key references:
- **Shuai et al., "BERT-based Dense Retrievers Require Interpolation with BM25"** — Linear interpolation of normalized BM25 and dense scores yields "significant gains in effectiveness" on MS MARCO. Their formula is exactly `s(p) = α·ŝ_BM25(p) + (1-α)·s_BERT(p)`.
- **Hybrid retrieval for regulatory texts (2025)** — `Score = α·SemanticScore + (1-α)·LexicalScore` consistently outperforms BM25-only and semantic-only in Recall@k and MAP@k.
- **"Semantic–Lexical Fusion" (2025)** — Optimal balance around α ≈ 0.6 (more weight on semantic), reaching peak F1 ≈ 0.847 and 15–30% accuracy gains over single-paradigm methods.

## Technical Design

### Fusion Formula

```
final(d) = α × semantic_score(d) + (1 - α) × bm25_norm_score(d)
```

Where:
- `semantic_score(d)` = cosine similarity ∈ [0, 1] (already normalized from `1.0 - usearch_distance`)
- `bm25_norm_score(d)` = BM25 score normalized via saturation `raw/(raw + k)` ∈ [0, 1) (existing `normalize_bm25_scores()`)
- `α` ∈ [0, 1], default 0.7

Documents missing from one list get 0 for that signal:
- Semantic-only hit: `α × sem_score + 0`
- Lexical-only hit: `0 + (1-α) × bm25_score`

| α | Semantic #1 (0.9) only | Both #10 (sem 0.5, bm25 0.4) | Winner |
|---|----------------------|------------------------------|--------|
| 0.7 | 0.63 | 0.47 | Semantic ✓ |
| 0.5 | 0.45 | 0.45 | Tie |
| 0.3 | 0.27 | 0.43 | Both ✓ |

### Config Changes

**Replace** `search_rrf_k` with `search_semantic_weight` in `Config` (`src/config.rs`):

| Field | Env Var | Default | Type | Validation |
|---|---|---|---|---|
| `search_semantic_weight` | `MDVDB_SEARCH_SEMANTIC_WEIGHT` | `0.7` | `f64` | Must be in [0.0, 1.0] |

**Remove**: `search_rrf_k` field and `MDVDB_SEARCH_RRF_K` env var.

**Keep**: `bm25_norm_k` (still needed for BM25 saturation normalization).

### SearchQuery Changes

Add per-query override field + builder:

```rust
pub semantic_weight: Option<f64>,  // None = use config default
```

Builder: `with_semantic_weight(alpha: f64)`.

### Search Pipeline Changes

Current hybrid pipeline:
1. Semantic + lexical search in parallel
2. `reciprocal_rank_fusion()` — discards scores, uses only ranks
3. `normalize_rrf_scores()` — post-fusion normalization

New hybrid pipeline:
1. Semantic + lexical search in parallel
2. **Normalize BM25 scores** via saturation (moved from post-fusion to pre-fusion)
3. `linear_score_fusion()` — weighted interpolation of normalized scores
4. No post-fusion normalization needed (output already in [0, 1])

```rust
pub fn linear_score_fusion(
    semantic: &[(String, f64)],
    lexical: &[(String, f64)],
    alpha: f64,
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for (id, sem_score) in semantic {
        *scores.entry(id.clone()).or_default() += alpha * sem_score;
    }
    for (id, bm25_score) in lexical {
        *scores.entry(id.clone()).or_default() += (1.0 - alpha) * bm25_score;
    }
    let mut results: Vec<(String, f64)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
}
```

### Over-fetch Reduction

RRF needed 5x over-fetch because rank-based fusion loses many candidates. Linear interpolation preserves score magnitudes, so reduce to 3x (same as semantic/lexical modes).

### `search()` Signature Change

```rust
// Before:
pub async fn search(query, index, provider, fts_index, rrf_k: f64, bm25_norm_k, ...)
// After:
pub async fn search(query, index, provider, fts_index, semantic_weight: f64, bm25_norm_k, ...)
```

### Score Documentation Update

`SearchResult.score` doc comment changes from:
- **Hybrid**: RRF score normalized by theoretical maximum.

To:
- **Hybrid**: Weighted linear interpolation of semantic and BM25 scores (α × semantic + (1-α) × BM25).

### `SearchTimings.fusion_secs` doc comment

Change from "RRF fusion" to "score fusion".

## Implementation Steps

0. **Save PRD** — Write to `docs/prds/phase-22-linear-score-fusion.md`.

1. **Config** (`src/config.rs`) — Replace `search_rrf_k: f64` (default 60.0) with `search_semantic_weight: f64` (default 0.7). Replace env var `MDVDB_SEARCH_RRF_K` with `MDVDB_SEARCH_SEMANTIC_WEIGHT`. Update validation: `!(0.0..=1.0).contains(&self.search_semantic_weight)` → error. Update inline config tests.

2. **SearchQuery** (`src/search.rs`) — Add `semantic_weight: Option<f64>` field, initialize to `None`, add `with_semantic_weight()` builder.

3. **Fusion function** (`src/search.rs`) — Add `linear_score_fusion()`. Remove `reciprocal_rank_fusion()` and `normalize_rrf_scores()`.

4. **Search pipeline** (`src/search.rs`) — Update `search()` signature (`rrf_k` → `semantic_weight`). In Hybrid branch: normalize BM25 pre-fusion, resolve effective alpha, call `linear_score_fusion()`. Remove post-fusion RRF normalization. Reduce hybrid over-fetch to 3x. Update doc comments.

5. **Library API** (`src/lib.rs:~996`) — Pass `self.config.search_semantic_weight` instead of `self.config.search_rrf_k`.

6. **Display** (`src/format.rs:~955`) — Show `semantic_weight=` instead of `rrf_k=`.

7. **Mock configs** — Mechanical rename `search_rrf_k: 60.0` → `search_semantic_weight: 0.7` in:
   - `src/embedding/provider.rs:86`
   - `src/tree.rs:329`
   - `src/watcher.rs:462`
   - `tests/api_test.rs:36`
   - `tests/embedding_test.rs:37`
   - `tests/graph_test.rs:34`
   - `tests/ingest_test.rs:44`
   - `tests/links_test.rs:35`
   - `tests/tree_test.rs:35`
   - `tests/watcher_test.rs:39`

8. **Search tests** (`tests/search_test.rs`) — All 31 `search()` call sites: change 5th arg from `60.0` to `0.7`. Update hybrid test assertions.

9. **Config tests** (`tests/config_test.rs`) — Update default assertion (`search_semantic_weight == 0.7`), dotenv test (use `MDVDB_SEARCH_SEMANTIC_WEIGHT`), rename validation tests.

10. **Unit tests** (`src/search.rs`) — Remove 9 RRF tests + 4 RRF normalization tests. Add:
    - `test_linear_fusion_overlapping_lists` — verify weighted scores for items in both lists
    - `test_linear_fusion_disjoint_lists` — items in only one list
    - `test_linear_fusion_empty_inputs` — both empty
    - `test_linear_fusion_alpha_zero_pure_lexical` — α=0 → pure BM25
    - `test_linear_fusion_alpha_one_pure_semantic` — α=1 → pure semantic
    - `test_linear_fusion_scores_in_unit_range` — output bounded [0, 1]
    - `test_search_query_with_semantic_weight` — builder test
    - `test_search_query_semantic_weight_default_none` — default is None

11. **Electron app** (`app/src/renderer/types/cli.ts:351`) — Rename `search_rrf_k: number` → `search_semantic_weight: number`.

## Files Modified

| File | Change |
|---|---|
| `src/config.rs` | Replace `search_rrf_k` → `search_semantic_weight`, new env var, new validation |
| `src/search.rs` | New `linear_score_fusion()`, remove RRF functions, update pipeline, `SearchQuery` field, update tests |
| `src/lib.rs` | Wire `search_semantic_weight` to `search::search()` |
| `src/format.rs` | Update display string |
| `src/embedding/provider.rs` | Mock config rename |
| `src/tree.rs` | Mock config rename |
| `src/watcher.rs` | Mock config rename |
| `tests/api_test.rs` | Mock config rename |
| `tests/embedding_test.rs` | Mock config rename |
| `tests/graph_test.rs` | Mock config rename |
| `tests/ingest_test.rs` | Mock config rename |
| `tests/links_test.rs` | Mock config rename |
| `tests/tree_test.rs` | Mock config rename |
| `tests/watcher_test.rs` | Mock config rename |
| `tests/search_test.rs` | Update 31 call sites, hybrid assertions |
| `tests/config_test.rs` | Update assertions, test names |
| `app/src/renderer/types/cli.ts` | Rename TypeScript type |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] Semantic search results unchanged (no regression)
- [ ] Hybrid search recall ≥ semantic search recall on benchmark
- [ ] `α=1.0` produces identical results to pure semantic mode
- [ ] `α=0.0` produces identical results to pure lexical mode
- [ ] Scores remain in [0, 1] for all modes
- [ ] Documents in both lists score higher than those in only one (for similar quality)
- [ ] Strong semantic-only hits are not demoted below mediocre dual-list hits
- [ ] `MDVDB_SEARCH_SEMANTIC_WEIGHT=0.5` overrides default
- [ ] Per-query `with_semantic_weight(0.5)` overrides config
- [ ] Validation rejects `MDVDB_SEARCH_SEMANTIC_WEIGHT=1.5` and `-0.1`
- [ ] Old `MDVDB_SEARCH_RRF_K` in config is silently ignored (no error)
- [ ] Time decay + linear fusion compose correctly
- [ ] Link boosting + linear fusion compose correctly

## Anti-Patterns to Avoid

- **Do not normalize BM25 after fusion** — BM25 saturation must happen before interpolation so both signals are on the same [0, 1] scale.

- **Do not keep RRF as a fallback** — Clean removal. One fusion method, no mode selection complexity.

- **Do not use min-max normalization per query** — Unstable with small result sets. Saturation normalization (existing `score/(score+k)`) is robust.

- **Do not change `bm25_norm_k`** — The BM25 saturation constant (default 1.5) is orthogonal to the fusion weight. Keep it unchanged.

- **Do not over-fetch 5x for hybrid** — Linear fusion preserves score quality better than RRF. 3x is sufficient.

## Patterns to Follow

- **Config field naming:** `MDVDB_SEARCH_*` pattern in `src/config.rs`
- **SearchQuery builder:** `with_decay()`, `with_boost_links()` pattern in `src/search.rs`
- **Per-query override resolution:** `query.field.unwrap_or(config_default)` in `search()`
- **Score normalization:** `normalize_bm25_scores()` saturation pattern in `src/search.rs`
- **Test helpers:** `mock_config()` with field rename in all test files
