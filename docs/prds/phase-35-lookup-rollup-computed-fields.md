# Phase 35: Relation-Backed Lookup and Rollup Fields

## Summary

Lookup and Rollup are schema-declared computed field types backed by frontmatter Relations.
Lookup copies a selected top-level field from the document or documents reached through a
Relation. Rollup collects selected values from related documents and evaluates a Formula-engine
expression over that collection.

Like Formula fields, both are virtual definitions whose successful results are materialized into
the owning Markdown document's frontmatter. Markdown remains the source of truth for reads,
filters, sorting, export, and later ingestion. Formula keeps its existing module; Lookup and Rollup
share one built-in, always-on module named `lookup_rollup` and reuse the same host-controlled safe
write path.

This phase is additive. It does not introduce a second relation syntax, recursive populate payload,
new index, or database-side-only value.

## Overlay contract

Definitions live in `.markdownvdb.schema.yml` and use the existing global or path-scoped overlay
rules. Computed definitions cannot own the reserved `title` or `path` fields.

### Outgoing Lookup

```yaml
scopes:
  contacts:
    fields:
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
```

For each Contact, `client_domain` follows its `client` Relation and reads the related Client's
top-level `domain` frontmatter field.

A Lookup is always outgoing. Authors must not specify `relation_direction`, `relation_scope`,
`formula`, `result_type`, or the Relation-only `target` property on a Lookup definition.

### Outgoing Rollup

```yaml
scopes:
  clients:
    fields:
      selected_invoice_total:
        field_type: rollup
        relation_field: invoices
        target_field: total
        formula: values.reduce((sum, value) => sum + Number(value), 0)
        result_type: number
```

Outgoing is the default Rollup direction. It follows the current document's selected Relation,
collects every related document's `target_field` into `values`, and evaluates the expression using
the existing sandboxed Formula runtime. Outgoing definitions must not specify `relation_scope`.

### Incoming Rollup

```yaml
scopes:
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_direction: incoming
        relation_scope: invoices
        relation_field: client
        target_field: total
        formula: values.reduce((sum, value) => sum + Number(value), 0)
        result_type: number
```

For each Client, this scans only documents below the segment-safe `invoices` scope, selects those
whose `client` Relation resolves back to that Client, and aggregates their `total` fields. Incoming
Lookup is intentionally unsupported: reverse traversal is an aggregation and therefore belongs to
Rollup.

## Definition validation and normalization

- `field_type` accepts `lookup` and `rollup` case-insensitively and exposes them as PascalCase
  `Lookup` and `Rollup` in JSON.
- `relation_field` and `target_field` are required, non-empty exact top-level field names for both
  types. Surrounding whitespace is removed. A definition cannot use its own output as
  `relation_field`.
- Lookup rejects authored direction and scope. Its effective public direction is `Outgoing`.
- Rollup requires a non-empty `formula` and a supported `result_type`, exactly as Formula does.
- Rollup accepts `relation_direction: outgoing|incoming`; omission means `outgoing`.
- Incoming Rollup requires a non-empty normalized `relation_scope`. Backslashes are normalized,
  leading `./` and surrounding slashes are removed, and empty, root, `.` or `..` path components
  are rejected.
- Outgoing Rollup rejects `relation_scope`.
- `formula` and `result_type` remain illegal on non-Formula/non-Rollup fields. Relation traversal
  metadata remains illegal on other field types.
- `target` continues to describe a Relation field's target folder. Formula, Lookup, and Rollup
  definitions cannot repurpose it.

Invalid overlay metadata fails configuration validation. If a previously valid computed definition
becomes invalid or the overlay disappears, the module fails closed: it reports a diagnostic and
removes its stale materialized value instead of presenting old data as current.

## Public schema contract

`FieldType` adds `Lookup` and `Rollup`. A merged `SchemaField` exposes these always-present nullable
properties in addition to the existing Formula properties:

```text
relation_field: string | null
target_field: string | null
relation_direction: Outgoing | Incoming | null
relation_scope: string | null
```

For Lookup, `formula` and `result_type` are null, direction is `Outgoing`, and scope is null. For
Rollup, the formula, result type, and effective direction are present; scope is present only for an
incoming definition. Non-relation computed fields keep `relation_target: null`.

Collection column metadata carries the same additive properties so clients can render and edit the
definition without reparsing overlay YAML. Existing fields retain null values and existing JSON
shapes remain compatible.

Computed `occurrence_count` and `sample_values` describe successful module-owned materializations,
not an unrelated or stale raw field that happened to use the same name. Explicit null results are
valid values but do not count as non-null occurrences.

## Relation selection semantics

Lookup and Rollup reuse the phase-31 whole-value Relation parser and its deterministic three-step
target resolution. They do not add basename matching or fuzzy resolution:

1. A target containing `/` is tried root-relative, then source-directory-relative.
2. A bare target uses the selected Relation field's overlay `target` folder when declared.
3. It then falls back to source-directory-relative resolution.

Only Markdown Relations are accepted. A File link, malformed link, non-string relation element, or
unresolved target produces a diagnostic and no output for that definition.

Outgoing cardinality is preserved:

- Missing or null Relation -> Lookup `null`; Rollup receives `values = []`.
- Scalar Relation -> scalar Lookup result; Rollup receives a one-element `values` list.
- Relation list -> Lookup list in authored order, preserving duplicates; Rollup receives values in
  the same order, also preserving duplicates.

Incoming Rollup scans only the declared segment-safe scope. Matching source documents are deduped
and sorted by root-relative path before their values enter `values`, making results deterministic
across filesystems and index iteration orders.

`target_field` is an exact top-level lookup. An explicitly present `null` is a valid retrieved value.
A related document that exists but lacks `target_field` is an error, as is a broken relation. One
bad target fails that computed output closed; the module does not silently omit it from an
aggregate, because doing so would produce a plausible but incorrect total.

## Rollup expression semantics

Rollup expressions run in the same deterministic Formula engine and use the same supported result
types, exact decimal arithmetic, syntax restrictions, evaluation limits, type checking, and
diagnostic spans. The sole input added by Rollup is:

```text
values: list of retrieved JSON-compatible target values
```

Common formulas include:

```javascript
values.reduce((sum, value) => sum + Number(value), 0)
values.length
values.filter(value => value === "paid").length
values.length === 0 ? null : values.reduce((max, value) => Math.max(max, Number(value)), Number(values[0]))
```

Only expressions supported by the existing Formula subset are valid; the examples do not imply
ambient JavaScript, filesystem, network, clock, random, or host-function access.

## Dependencies and evaluation order

The always-on Formula module runs before `lookup_rollup`. A Lookup or Rollup may therefore select a
successfully materialized Formula field from a related document.

Lookup and Rollup definitions may select another Lookup or Rollup field. Their document/field
dependencies form one graph, are evaluated in deterministic dependency order, and may cross scope
boundaries. Strongly connected components are rejected with `dependency_cycle` diagnostics for
every participant; dependent nodes receive a dependency failure and stale outputs are removed.

A Formula definition depending on a Lookup or Rollup output is unsupported in this phase because
Formula executes first. Authors must express the downstream calculation as a Rollup or restructure
the dependency. The system must never satisfy such a dependency from yesterday's materialized
value.

## Materialization and ownership safety

Computed definitions are virtual, but successful results are written into the corresponding
Markdown frontmatter. The module never rewrites Markdown directly. It emits declarative field
patches to the shared module runner, which:

- Starts from an immutable indexed snapshot.
- Carries the expected source content hash with every patch and rejects stale compare-and-swap
  writes.
- Re-reads and patches only the intended top-level frontmatter keys.
- Preserves unrelated YAML/frontmatter and the Markdown body.
- Writes a temporary file, flushes it, and atomically replaces the source.
- Updates the stored frontmatter and source hash after a successful write without re-embedding when
  the body hash is unchanged.
- Suppresses or harmlessly absorbs the watcher echo from its own write.
- Unsets fields whose definitions were removed or whose evaluation failed rather than retaining a
  stale value.

A definition rename is represented as one overlay generation that removes the old output key and
adds the new output key. The next dependency-aware run therefore treats the old key as removed and
cleans only values proven to be owned by `lookup_rollup`, while evaluating and materializing the new
key normally. A rename must never be implemented as an unguarded frontmatter-wide key rewrite.

The index records module ownership, definition fingerprints, last valid serialized values, and
diagnostics. Each internal computed entry also persists its normalized input fingerprint and the
dependency snapshot that produced it: dependency path existence/content-hash state plus, for an
incoming Rollup, deterministic relation-scope membership and candidate hashes. The owner path is
recorded as existence-only because its full source hash is already protected by the patch CAS and
the module's own materialization necessarily changes it. Input fingerprints and dependency
snapshots are omitted from public `computed_fields`, schema, status, and diagnostic JSON; content
hashes never become part of the public computed-field contract. That metadata is coordination/cache
state, not an alternate source of truth.

Computed fields may contain strings that look like Markdown links. Formula, Lookup, and Rollup
outputs are excluded from relation extraction, link graph edges, backlinks, and populate traversal.
Exclusion uses both the live overlay and persisted per-document ownership so removing or breaking a
definition cannot briefly turn a stale computed value into a user-authored foreign key.

### Amendment (2026-08): adopt-by-declaration for computed sets

Ownership proof lives only in the index, and the index is disposable by design (unreadable or
incompatible archives are deleted and rebuilt). The original fail-closed rule — refusing a
computed `set` to an existing frontmatter key without materialized proof — therefore left every
previously materialized field permanently stuck after any index rebuild: the stale value kept
being served while every recompute was refused with `writeback_failed`. Fresh clones of a vault
whose files carry materialized values (e.g. the checked-in test vault) hit this immediately.

Computed `set` keys are by construction limited to fields the current overlay declares computed
for the owning path's scope. The overlay is the user's own declaration that the field is computed,
so an existing same-named value is either a stale materialization whose provenance was lost or a
manual value the user has since declared computed — in both cases the computed value now wins:
the write proceeds and records fresh ownership (adopt-by-declaration). Writes into a frontmatter
block that does not parse as a YAML mapping are still refused (never adopt through ambiguity), a
`set` equal to the existing value still converges without a write or an ownership claim, and
`unset` authority is unchanged — only values proven to be owned are ever removed.

## Execution hooks and consistency

The `lookup_rollup` module is built in and always on. It participates in:

- Full ingest.
- Incremental file changes.
- Schema-overlay changes.
- Watch mode.
- Explicit module runs.

Any changed relation, reverse-relation candidate, target value, definition, or dependent computed
field can invalidate owners elsewhere in the Collection. Implementations may optimize dependency
tracking later, but this phase recomputes from the coherent Collection snapshot where necessary;
correctness takes precedence over an unsafe local-only shortcut.

All successful frontmatter materializations flow through ordinary parse/index behavior, so get,
collection, search, metadata filters, sorting, schema samples, export, and API consumers see current
values without a special read-side payload. `--populate` remains depth one and does not recursively
expand computed fields.

## Module and CLI contract

The built-in module descriptor is:

```text
id: lookup_rollup
name: Lookup & Rollup
version: 1
always_on: true
hooks: full_ingest, files_changed, schema_changed, manual_run
```

It appears alongside Formula in module list/status output and can be explicitly run through the
existing module command. Manual path scoping limits which owning documents are written; automatic
events remain free to recompute all potentially affected owners. Diagnostics identify module,
document, output field, stable error code, message, and Formula source span when applicable.

No CLI version, crate version, compact graph version, index header version, or explicit index
version is bumped. During development, incompatible archived layouts continue to use the existing
self-healing index rebuild behavior.

## Diagnostics

Stable diagnostic categories include:

- Invalid or missing schema properties.
- Invalid relation value or non-Markdown target.
- Unresolved relation target.
- Missing `target_field` on an existing related document.
- Rollup syntax, runtime, limit, or result-type errors from the Formula engine.
- Lookup/Rollup dependency failure.
- Lookup/Rollup dependency cycle.
- Source changed before materialization and the compare-and-swap write was refused.

Diagnostics never justify writing a partial aggregate or preserving an outdated value.

## Non-goals

- Incoming Lookup.
- Recursive or arbitrary-depth populate.
- Basename-only or fuzzy relation resolution.
- Nested-path selectors such as `company.domain`; `target_field` is top-level.
- Cross-Collection relations, remote data sources, SQL-style joins, or query-defined source sets.
- Incremental reverse-dependency indexing beyond what correctness requires.
- Formula expressions that depend on later Lookup/Rollup outputs.
- Editing computed results as if they were user-owned frontmatter.

## Acceptance criteria

- Outgoing scalar and list Lookups preserve cardinality, order, duplicates, JSON types, and explicit
  null values.
- Outgoing and incoming Rollups receive deterministic `values` and reuse Formula behavior for every
  supported result type.
- Incoming traversal is segment-safe, deduped, and path-sorted.
- Formula target fields and acyclic Lookup/Rollup chains evaluate in dependency order.
- Cycles, missing target fields, malformed Relations, broken targets, invalid formulas, and type
  errors remove stale outputs and emit actionable diagnostics.
- Definition removal and invalid/missing overlay cleanup only module-owned fields and preserve
  unrelated frontmatter and body bytes.
- Definition rename converges by cleaning the old owned key and materializing the new key without
  overwriting an existing field or rewriting unrelated frontmatter.
- Materialization is atomic and hash-guarded; body-stable updates do not re-embed documents.
- Computed link-shaped values never create graph edges or populate Relations, including while a
  definition is being removed.
- Full ingest, incremental ingest, watch, schema change, and manual module execution converge to
  the same Markdown and index state.
- Public schema and Collection-column JSON expose the additive Lookup/Rollup metadata with stable
  PascalCase enum values and nulls where inapplicable.
- Existing Formula, Relation, populate, link graph, search, schema, module, and watcher tests remain
  green, with no version bumps.
