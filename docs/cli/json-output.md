---
title: "JSON Output Reference"
description: "Comprehensive reference for JSON output schemas across all mdvdb commands that support --json"
category: "guides"
---

# JSON Output Reference

Use the global **--json** flag to consume mdvdb from scripts, agents, and
desktop applications:

~~~bash
mdvdb search "authentication" --json
mdvdb --json status
~~~

The flag is accepted before or after a public subcommand. Most commands write
one pretty-printed JSON document followed by a newline. The command-specific
types are intentionally different; there is no universal success envelope.

## Global behavior

- JSON data is written to stdout.
- Without **-v**, JSON mode suppresses tracing output. With **-v**, diagnostic
  logs and timing messages can still be written to stderr.
- A fatal error is plain text on stderr with a non-zero exit status. There is
  no JSON error envelope.
- Optional fields are controlled by each response type. Some are omitted,
  while other optional values serialize as null.
- Consumers should tolerate additive fields and use the command plus its mode
  to select a decoder.

Three output paths are exceptions to the usual pretty-printed document:

- **mdvdb ingest --json-lines** streams newline-delimited JSON.
- **mdvdb watch --json** streams newline-delimited JSON.
- **mdvdb graph --compact --json** writes one minified compact graph document.

The top-level version flag has a small independent contract:

~~~bash
mdvdb --version --json
~~~

~~~json
{"version":"0.2.0"}
~~~

The value is the installed package version.

## Command shape directory

The table names the top-level type or keys. Follow the command link for its
complete nested fields and examples.

| Command | JSON stdout |
| --- | --- |
| [search](./commands/search.md) | A SearchOutput object with **results**, **query**, **total_results**, and **mode**. **timings** is present with **-v**; non-empty graph and edge results are additive fields. |
| [ingest](./commands/ingest.md) | Normal ingest returns an IngestOutput object. **--preview --json** returns an IngestPreview object. **--json-lines** uses the streaming contract below. |
| [status](./commands/status.md) | One IndexStatus object. |
| [info](./commands/info.md) | One VaultInfo object for the Collection, path, or Shard scope. |
| [schema](./commands/schema.md) | An unscoped Schema object, or a ScopedSchema object when **--path** or **--shard** is used. |
| [clusters](./commands/clusters.md) | Automatic and computed Topic reports are arrays; **list** is a Topic-definition array; **unassigned** is an object with **count** and **paths**. Shard-local mutations return a ShardTopicMutation object. |
| [shards](./commands/shards.md) | **list** returns **shards** and **total_shards**; **get** returns one ShardInfo; mutations return **action** and the complete affected **shards** list. |
| [tree](./commands/tree.md) | One FileTree object, optionally filtered to a path or Shard. |
| [get](./commands/get.md) | One DocumentInfo object. **--populate** adds **relations** and **referenced_by**. |
| [collection](./commands/collection.md) | One CollectionResponse with **scope**, **recursive**, **columns**, **rows**, **total_rows**, optional **limit**, and **offset**. |
| [watch](./commands/watch.md) | A startup line followed by one compact WatchEventReport JSON value per line. |
| [modules](./commands/modules.md) | **list** returns ModuleDescriptor objects; **validate** returns **valid** and **diagnostics**; **run** returns a ModuleRunReport; **status** returns a diagnostic array. |
| [init](./commands/init.md) | No JSON contract. Initialization prints its normal success text even when the global flag is present. |
| [config](./commands/config.md) | With no action, one resolved Config object. **set**, **unset**, and **secret** actions emit no JSON body in JSON mode. |
| [embedding](./commands/embedding.md) | **models** returns **provider**, **discovery_available**, and **models**; **probe** returns **provider**, **model**, **dimensions**, and **latency_ms**. |
| [doctor](./commands/doctor.md) | One DoctorResult object with **checks**, **passed**, and **total**. |
| [links](./commands/links.md) | Depth 1 returns a LinksOutput object; depth 2 or 3 returns a NeighborhoodResult object. |
| [backlinks](./commands/backlinks.md) | An object with **file**, **backlinks**, and **total_backlinks**. |
| [orphans](./commands/orphans.md) | An object with **orphans** and **total_orphans**. |
| [edges](./commands/edges.md) | An object with **edges**, **total_edges**, and optional applied filters. |
| [graph](./commands/graph.md) | A GraphData object for document or chunk level. **--compact** uses the versioned compact graph type; Shard graph data can include **analysis**. |

Collection-level **clusters add**, **clusters update**, and **clusters remove**
currently emit no stdout body in JSON mode. Their Shard-local forms do return
the mutation object described above.

## Search envelope

Without verbose timing or non-empty graph fields, a search envelope can be as
small as:

~~~json
{
  "results": [],
  "query": "authentication",
  "total_results": 0,
  "mode": "hybrid"
}
~~~

Each result contains its relevance score, matched chunk, and source file.
Population, graph expansion, edge search, and verbose mode add data according
to the selected flags. See [search JSON output](./commands/search.md#json-output)
for those nested shapes.

## Ingest documents

Normal **mdvdb ingest --json** writes one object containing:

- File and chunk counts: **files_indexed**, **files_skipped**,
  **files_removed**, and **chunks_created**
- Embedding accounting: **api_calls** and **estimated_input_tokens**
- Recoverable file failures: **files_failed** and **errors**
- Computed-field work: **module_reports**
- Completion state: **duration_secs** and **cancelled**
- Optional phase **timings** when **-v** is supplied

Preview is a different response type because it performs no ingest:

~~~bash
mdvdb ingest --preview --json
~~~

It returns per-file preview records and aggregate file, chunk, token, and
estimated-call counts. See [ingest](./commands/ingest.md) for both complete
objects.

## Ingest JSON-lines framing

For a live progress stream, use:

~~~bash
mdvdb ingest --json-lines
~~~

Each line is one complete JSON object with exactly three top-level fields:

| Field | Meaning |
| --- | --- |
| **type** | **progress** or **result** |
| **data** | The phase payload or final IngestOutput |
| **operation** | **ingest** |

The stream begins with a preparing phase:

~~~json
{"type":"progress","data":{"phase":"preparing","reindex":false,"elapsed_ms":0,"accumulated_errors":0},"operation":"ingest"}
~~~

Every progress **data** object contains a tagged ingestion phase plus
**elapsed_ms** and **accumulated_errors**. Depending on work performed, phase
values can be:

- **preparing**, **probing**, or **discovering**
- **parsing**, **skipped**, or **file_error**
- **embedding**
- **saving**, **clustering**, or **cleaning**
- **cancelled** or **done**

Fields specific to a phase live beside **phase** in **data**. For example:

~~~json
{"type":"progress","data":{"phase":"parsing","current":4,"total":20,"path":"docs/api.md","elapsed_ms":31,"accumulated_errors":0},"operation":"ingest"}
~~~

The final successful frame uses the same result fields as ordinary ingest:

~~~json
{"type":"result","data":{"files_indexed":1,"files_skipped":4,"files_removed":0,"chunks_created":3,"api_calls":1,"estimated_input_tokens":420,"files_failed":0,"errors":[],"module_reports":[],"duration_secs":0.82,"cancelled":false},"operation":"ingest"}
~~~

There is no surrounding array and no comma between lines. Parse and handle
each line as it arrives. A fatal command error can end the process with stderr
and a non-zero status before a result frame is written.

For a real ingest, **--json-lines** takes precedence over **--json** when both
are present. Preview returns through its separate preview output path instead
of producing progress frames.

## Watch streaming

Watch also uses NDJSON, but it does not use the ingest
**type/data/operation** envelope:

~~~bash
mdvdb watch --json
~~~

The first line is:

~~~json
{"status":"watching","message":"File watching started"}
~~~

Every later line is a WatchEventReport with:

- **event_type** and **path**
- Optional **previous_path** for a rename
- **chunks_processed**, **estimated_input_tokens**, and **api_calls**
- **duration_ms**, **success**, and nullable **error**
- **module_reports**

The process keeps writing lines until it is cancelled. See
[watch](./commands/watch.md) for lifecycle details.

## Graph JSON

Normal graph JSON is pretty-printed:

~~~bash
mdvdb graph --level document --json
mdvdb graph --level chunk --json
~~~

The compact wire format is one minified document and requires JSON mode:

~~~bash
mdvdb graph --compact --json
~~~

Do not decode compact graph output as ordinary GraphData. Use the version and
interned contexts defined by the [graph command](./commands/graph.md).

## Automation examples

Extract result paths:

~~~bash
mdvdb search "authentication" --json |
  jq -r '.results[].file.path'
~~~

Read one page of structured records:

~~~bash
mdvdb collection projects --filter status=active --json |
  jq '.rows[] | {path, frontmatter}'
~~~

Select the final ingest frame:

~~~bash
mdvdb ingest --json-lines |
  jq -c 'select(.type == "result") | .data'
~~~

Treat stderr and the process exit status separately from stdout in all three
cases.

## Related pages

- [Command reference](./commands/index.md)
- [Search](./commands/search.md)
- [Ingest](./commands/ingest.md)
- [Collection](./commands/collection.md)
- [Modules](./commands/modules.md)
- [Shards](./commands/shards.md)
- [Graph](./commands/graph.md)
