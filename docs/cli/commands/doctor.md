---
title: "mdvdb doctor"
description: "Diagnose configuration, provider connectivity, index health, relations, and Shards"
category: "commands"
---

# mdvdb doctor

Run a read-only health check for the current project. `doctor` reports configuration discovery,
embedding connectivity, index integrity, source discovery, frontmatter Relations, and Shard/Topic
configuration in one result.

## Usage

```bash
mdvdb doctor [OPTIONS]
```

## Options

This command has no command-specific options. It accepts the [global options](./index.md#global-options),
including `--root`, `--json`, `--verbose`, and `--no-color`.

## Diagnostic checks

A successful diagnostic run returns nine checks in this order:

| # | Check | What it validates |
|---|-------|-------------------|
| 1 | **Config loaded** | Shows the resolved embedding provider, model, and dimensions. The command has already loaded configuration when this check is emitted. |
| 2 | **User config** | Looks for `~/.mdvdb/config.yaml`, or `config.yaml` below `MDVDB_CONFIG_HOME`. A missing user config is a warning because it is optional. |
| 3 | **Project config** | Checks that the project has a `.markdownvdb/` directory. |
| 4 | **API key** | Explicitly validates `OPENAI_API_KEY` for the OpenAI provider. Mock needs no key; other providers are reported as configured and are actually exercised by the connectivity check. |
| 5 | **Provider reachable** | Sends one test embedding request with a five-second timeout. Authentication, endpoint, model, and network failures surface here. |
| 6 | **Index** | Reports documents, chunks, edge vectors, and vectors. A healthy index has `vectors = chunks + edges`. An empty or mismatched index warns. |
| 7 | **Source directories** | Discovers Markdown in the configured source directories and reports the file count. |
| 8 | **Relations** | Warns about dangling frontmatter Relation targets, schema target folders with no indexed files, and unquoted `[[wikilink]]` values that YAML parsed as nested lists. |
| 9 | **Shards** | Validates Shard definitions, target folders, and local Topic definitions. Missing or malformed configuration warns. |

Each check is `Pass`, `Warn`, or `Fail`. Warnings describe repairable or optional conditions; failures
normally prevent an operation such as source discovery or provider access from working.

## Human-readable output

```text
  ● mdvdb doctor

  ✓ Config loaded             OpenAI / text-embedding-3-small / 1536
  ✓ User config               /home/user/.mdvdb/config.yaml
  ✓ Project config            .markdownvdb/
  ✓ API key                   OPENAI_API_KEY is set
  ✓ Provider reachable        OK (243ms)
  ✓ Index                     57 docs, 342 chunks, 360 vectors (342 chunk + 18 edge)
  ✓ Source directories        ./ (57 .md files)
  ✓ Relations                 18 relation link(s), all targets resolve
  ✓ Shards                    3 Shard(s) configured, all folders exist

  9/9 checks passed
```

Warnings are included in the denominator but not the passed count:

```text
  ! Index                     empty — run `mdvdb ingest` to index your markdown files
  ! Relations                 2 dangling relation(s): projects/a.md#owner → people/missing.md, +1 more
  ! Shards                    2 Shard(s) configured; 1 missing folder(s): archive (archive)
```

## Examples

```bash
# Run all checks in the current project
mdvdb doctor

# Return the structured result
mdvdb doctor --json

# Diagnose another project
mdvdb doctor --root /path/to/project

# Include diagnostic logging
mdvdb doctor -v
```

## JSON output

`--json` serializes a `DoctorResult`. The `checks` array uses the same order and detail strings as
the human-readable result.

```json
{
  "checks": [
    {
      "name": "Config loaded",
      "status": "Pass",
      "detail": "OpenAI / text-embedding-3-small / 1536"
    },
    {
      "name": "User config",
      "status": "Pass",
      "detail": "/home/user/.mdvdb/config.yaml"
    },
    {
      "name": "Project config",
      "status": "Pass",
      "detail": ".markdownvdb/"
    },
    {
      "name": "API key",
      "status": "Pass",
      "detail": "OPENAI_API_KEY is set"
    },
    {
      "name": "Provider reachable",
      "status": "Pass",
      "detail": "OK (243ms)"
    },
    {
      "name": "Index",
      "status": "Pass",
      "detail": "57 docs, 342 chunks, 360 vectors (342 chunk + 18 edge)"
    },
    {
      "name": "Source directories",
      "status": "Pass",
      "detail": "./ (57 .md files)"
    },
    {
      "name": "Relations",
      "status": "Pass",
      "detail": "18 relation link(s), all targets resolve"
    },
    {
      "name": "Shards",
      "status": "Pass",
      "detail": "3 Shard(s) configured, all folders exist"
    }
  ],
  "passed": 9,
  "total": 9
}
```

| Field | Type | Description |
|-------|------|-------------|
| `checks` | `DoctorCheck[]` | Ordered diagnostic results. |
| `passed` | `number` | Number of checks with status `Pass`. |
| `total` | `number` | Number of checks returned. |
| `checks[].name` | `string` | Human-readable check name. |
| `checks[].status` | `"Pass" \| "Fail" \| "Warn"` | Check status. |
| `checks[].detail` | `string` | Context or repair information. |

## Troubleshooting

| Check | Symptom | What to do |
|-------|---------|------------|
| **Project config** | `.markdownvdb not found` | Run [`mdvdb init`](./init.md). |
| **API key** | `OPENAI_API_KEY not set` | Configure the credential via the shell, `.markdownvdb/.env`, or `mdvdb config secret set OPENAI_API_KEY --stdin`. |
| **Provider reachable** | Timeout or authentication/model error | Check the endpoint, model, credentials, and network. For Ollama, verify the service is running. |
| **Index** | Empty | Run [`mdvdb ingest`](./ingest.md). |
| **Index** | Vector mismatch | Rebuild with `mdvdb ingest --reindex`. The expected count is chunks plus edge vectors, not chunks alone. |
| **Source directories** | Discovery failure | Check configured paths and read permissions. |
| **Relations** | Dangling target | Correct the link target or add/index the target file. Quote wikilink values in YAML, for example `owner: "[[people/ada]]"`. |
| **Relations** | Schema target matches no file | Correct the Relation field's `target` folder or index that folder. |
| **Shards** | Missing folder or malformed Topic | Correct the Shard path or its local Topic definition, then rerun `doctor`. |

## Notes

- The index is opened read-only; `doctor` does not rebuild or mutate it.
- The provider check makes one real embedding request and may consume a small amount of provider quota.
- The OpenAI credential has a dedicated preflight check. For other providers, rely on the provider
  connectivity result for credential validation.

## Related documentation

- [`mdvdb status`](./status.md) — index counts and embedding compatibility
- [`mdvdb config`](./config.md) — resolved configuration
- [`mdvdb links`](./links.md) — body links and frontmatter Relations
- [`mdvdb shards`](./shards.md) — named collection scopes
- [Configuration](../configuration.md) — files, environment variables, and secrets
- [Embedding providers](../concepts/embedding-providers.md) — provider-specific setup
- [Relations](../concepts/relations.md) — typed frontmatter links and population
- [Shards and Topics](../concepts/shards-and-topics.md) — scoped knowledge organization
- [JSON output](../json-output.md) — machine-readable command conventions
