# PRD: Phase 3 — Chunking Engine

## Overview

Implement the hybrid document chunking strategy that splits markdown files into semantically meaningful chunks for embedding. The primary split is by headings (preserving document structure), with a secondary size guard that sub-splits oversized sections into overlapping fixed-size chunks. Each chunk carries metadata linking it back to its source file, heading hierarchy, and line range.

## Problem Statement

Embedding entire documents produces poor search results — a 5000-word document about multiple topics will weakly match many queries instead of strongly matching specific ones. Chunking at the heading level preserves the author's intended structure and produces focused, topically coherent chunks. However, some sections are too long for embedding models (which have token limits), so a size guard is needed as a fallback.

## Goals

- Split markdown documents by headings (h1–h6), where each heading starts a new chunk
- Sub-split sections exceeding `MDVDB_CHUNK_MAX_TOKENS` into overlapping fixed-size chunks
- Overlap between sub-splits is `MDVDB_CHUNK_OVERLAP_TOKENS` tokens
- Short files under the token limit remain a single chunk
- Each chunk retains: parent file path, heading hierarchy (breadcrumb), line range, chunk index
- Token counting uses `tiktoken-rs` for accuracy matching embedding model tokenization
- Chunking is deterministic — same input always produces same chunks

## Non-Goals

- No embedding generation (Phase 4)
- No semantic chunking (splitting by topic detection) — only structural (headings) and mechanical (token limit)
- No sentence-level splitting — sub-splits use token boundaries, not sentence boundaries
- No chunk deduplication across files

## Technical Design

### Data Model Changes

**`Chunk` struct** — a single embeddable unit:

```rust
pub struct Chunk {
    /// Unique ID: "{relative_path}#{chunk_index}"
    pub id: String,
    /// Path to source file (relative to project root)
    pub source_path: PathBuf,
    /// Heading hierarchy breadcrumb, e.g. ["Getting Started", "Installation", "macOS"]
    pub heading_hierarchy: Vec<String>,
    /// The text content of this chunk (what gets embedded)
    pub content: String,
    /// 1-based start line in the source file
    pub start_line: usize,
    /// 1-based end line in the source file (inclusive)
    pub end_line: usize,
    /// 0-based index of this chunk within its source file
    pub chunk_index: usize,
    /// Whether this chunk is a sub-split of a larger section
    pub is_sub_split: bool,
}
```

### Interface Changes

**Chunking function:**

```rust
/// Split a parsed MarkdownFile into chunks
pub fn chunk_document(
    file: &MarkdownFile,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Result<Vec<Chunk>>;
```

**Token counting utility:**

```rust
/// Count tokens in a text string using the cl100k_base tokenizer
pub fn count_tokens(text: &str) -> usize;
```

### Chunking Algorithm

```
1. Parse the body into sections by heading boundaries
   - Each section = heading text + all content until the next heading of equal or higher level
   - Content before the first heading (preamble) is its own section with empty heading hierarchy

2. For each section:
   a. Count tokens using tiktoken-rs (cl100k_base encoding)
   b. If tokens <= max_tokens:
      → Emit as a single chunk with heading hierarchy and line range
   c. If tokens > max_tokens:
      → Sub-split into fixed-size chunks of max_tokens with overlap_tokens overlap
      → Each sub-chunk inherits the parent section's heading hierarchy
      → Mark is_sub_split = true on each sub-chunk

3. Heading hierarchy is built as a stack:
   - h1 resets the stack to [h1_text]
   - h2 under h1 becomes [h1_text, h2_text]
   - h3 under h2 becomes [h1_text, h2_text, h3_text]
   - A new h2 pops back to [h1_text, new_h2_text]
   (Each heading pops all headings of equal or lower level from the stack)

4. Line ranges tracked by counting newlines in accumulated content
```

### Migration Strategy

Not applicable — no prior data exists.

## Implementation Steps

1. **Create `src/chunker.rs`** — Implement the chunking module:
   - Define `Chunk` struct with `Serialize`/`Deserialize` derives (needed later for index storage)
   - Implement `count_tokens(text)` using `tiktoken_rs::cl100k_base()` encoder. Cache the encoder instance (it's expensive to create) using a module-level `std::sync::OnceLock`.
   - Implement `chunk_document(file, max_tokens, overlap_tokens)`:
     - First, split the body into sections. Iterate through `file.headings` to find section boundaries. Each section spans from one heading to the next. Content before the first heading is the "preamble" section.
     - Build heading hierarchy as a stack: when encountering heading level N, pop all entries with level >= N, then push the new heading.
     - For each section, call `count_tokens` on its content. If under `max_tokens`, emit as a single `Chunk`. If over, call `sub_split_section()`.
     - Track line numbers by splitting the body into lines and mapping heading positions.

2. **Implement `sub_split_section()`** — Private function for oversized sections:
   - Tokenize the section content with `tiktoken_rs`
   - Split tokens into windows of `max_tokens` size with `overlap_tokens` overlap
   - Detokenize each window back to text
   - Each sub-chunk gets the parent section's heading hierarchy
   - Set `is_sub_split = true`
   - Calculate approximate line ranges based on character offsets

3. **Implement chunk ID generation** — The `id` field format is `"{relative_path}#{chunk_index}"` where `chunk_index` is the 0-based position of this chunk within the file's chunk list. This provides a stable, unique identifier per chunk.

4. **Update `src/lib.rs`** — Add `pub mod chunker;`

5. **Write chunker unit tests** — In `src/chunker.rs` add `#[cfg(test)] mod tests`:
   - Test: file with 3 headings produces 3 chunks (one per section)
   - Test: file with no headings produces 1 chunk (preamble only)
   - Test: preamble content before first heading becomes chunk 0
   - Test: oversized section (> max_tokens) is sub-split into multiple chunks
   - Test: sub-split chunks have `is_sub_split = true`
   - Test: sub-split chunks have overlap (last N tokens of chunk K = first N tokens of chunk K+1)
   - Test: heading hierarchy breadcrumb is correct for nested headings (h1 > h2 > h3)
   - Test: heading hierarchy resets correctly when a new h1 appears
   - Test: short file under token limit is a single chunk
   - Test: empty body produces a single empty chunk
   - Test: `count_tokens` returns accurate count (verify against known values)
   - Test: chunk IDs follow the format `"path/to/file.md#0"`, `"path/to/file.md#1"`, etc.
   - Test: line ranges are correct (start_line and end_line map to actual source lines)

6. **Write integration test** — Create `tests/chunker_test.rs`:
   - Use the `tests/fixtures/` markdown files from Phase 2
   - Parse each file with `parse_markdown_file()`, then chunk with `chunk_document()`
   - Verify end-to-end: fixture file → parsed file → chunks with correct structure
   - Test with various `max_tokens` values (64, 128, 512, 1024) to verify sub-splitting behavior

## Validation Criteria

- [ ] A markdown file with 5 h2 sections produces 5 chunks (assuming each is under token limit)
- [ ] A 2000-token section with `max_tokens=512` and `overlap_tokens=50` produces ~4 chunks
- [ ] Sub-split chunks overlap by exactly `overlap_tokens` tokens
- [ ] Heading hierarchy for `h1 > h2 > h3` content is `["H1 Text", "H2 Text", "H3 Text"]`
- [ ] Heading hierarchy resets when a new same-level heading appears
- [ ] Preamble (content before first heading) has empty heading hierarchy `[]`
- [ ] A 100-token file with `max_tokens=512` produces exactly 1 chunk
- [ ] An empty file produces exactly 1 chunk with empty content
- [ ] Chunk IDs are unique within a file and follow `"path#index"` format
- [ ] `count_tokens("hello world")` returns the correct tiktoken count (2 tokens for cl100k_base)
- [ ] Chunking is deterministic — running twice on the same input produces identical chunks
- [ ] Line ranges in chunks correctly map to source file line numbers
- [ ] `cargo test` passes all chunker tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT split on arbitrary character counts** — Use `tiktoken-rs` token counting. Character count is a poor proxy for tokens ("don't" is 6 chars but 2 tokens).
- **Do NOT create a new tiktoken encoder per call** — The `cl100k_base()` encoder is expensive to initialize. Use `OnceLock` to create it once and reuse.
- **Do NOT discard heading hierarchy for sub-splits** — Sub-split chunks must inherit their parent section's heading hierarchy so search results can show context.
- **Do NOT use sentence splitting for the size guard** — Sentence detection is unreliable in markdown (code blocks, lists, etc.). Token-window splitting is simpler and more predictable.
- **Do NOT generate UUIDs for chunk IDs** — Use deterministic `"path#index"` format so the same file always produces chunks with the same IDs. This is required for incremental re-indexing (Phase 8).

## Patterns to Follow

- **Data flow:** `MarkdownFile` (from `src/parser.rs`) → `chunk_document()` → `Vec<Chunk>` — each module transforms data without side effects
- **Struct serialization:** Derive `serde::Serialize` and `serde::Deserialize` on `Chunk` for later use in index storage (Phase 5)
- **Error handling:** Return `Result<Vec<Chunk>>` using the `Error` type from `src/error.rs`; only the top-level function is fallible (token counting itself doesn't fail)
- **Config usage:** Accept `max_tokens` and `overlap_tokens` as parameters, not the full `Config` — keeps the function testable with different values
