---
title: "mdvdb init"
description: "Create project or user-level YAML configuration"
category: "commands"
---

# mdvdb init

Create a YAML configuration for a collection or for user-wide defaults.

## Usage

```bash
mdvdb init [--global]
```

| Flag | Effect |
|---|---|
| `--global` | Create user defaults at `~/.mdvdb/config.yaml` instead of project config |

The standard global flags also apply. Use `mdvdb --root <PATH> init` to initialize a different
collection root.

## Project initialization

Run `init` at the root of a Markdown collection:

```bash
cd my-notes
mdvdb init
```

It creates `.markdownvdb/config.yaml` with a starter configuration equivalent to:

```yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
  batch_size: 100

search:
  limit: 10
  min_score: 0.0
  mode: hybrid
  rrf_k: 60.0

chunking:
  max_tokens: 512
  overlap_tokens: 50

clustering:
  enabled: true
  rebalance_threshold: 50

watch:
  enabled: true
  debounce_ms: 300

sources:
  dirs: [.]
```

Unwritten settings retain built-in defaults, including Leiden as the automatic clustering
algorithm. Edit the YAML directly or use dotted scalar updates such as:

```bash
mdvdb config set embedding.provider ollama
mdvdb config set embedding.model nomic-embed-text
mdvdb config set embedding.dimensions auto
```

After the first ingest, the directory also contains generated index data:

```text
.markdownvdb/
├── config.yaml
├── index
└── fts/
```

Features such as Shard-local analysis may later add a disposable `cache/` directory.

`init` creates configuration only. It does not scan, embed, or index files; run `mdvdb ingest`
after configuring a provider.

## User defaults

```bash
mdvdb init --global
```

This creates `~/.mdvdb/config.yaml`, or `$MDVDB_CONFIG_HOME/config.yaml` when
`MDVDB_CONFIG_HOME` is set. The generated file is intentionally minimal:

```yaml
# Values here apply unless project config.yaml overrides them.
# Credentials belong in .env, not YAML.

# embedding:
#   provider: openai
#   model: text-embedding-3-small
#   dimensions: auto
```

User YAML supplies defaults. Project `.markdownvdb/config.yaml` is deep-merged over it, and shell
`MDVDB_*` overrides have the highest settings priority.

Do not store API keys in either YAML file. Put shared credentials in `~/.mdvdb/.env`, or write them
through stdin:

```bash
printf '%s' "$OPENAI_API_KEY" \
  | mdvdb config --global secret set OPENAI_API_KEY --stdin
```

See [Configuration](../configuration.md) for the full settings and secret precedence rules.

## Existing and legacy configurations

`init` never overwrites an existing configuration. It returns `ConfigAlreadyExists` when it finds:

- `.markdownvdb/config.yaml`
- legacy `.markdownvdb/.config`
- a legacy flat `.markdownvdb` file
- an existing user `config.yaml` for `--global`

Loading a legacy dotenv configuration automatically migrates recognized settings to YAML,
preserves secrets in `.env`, and retains the old file as a backup. Review the migrated files
instead of deleting the old configuration before migration.

## Output

On success, `init` prints the created project directory or user config path. It has no distinct
JSON response body; use the following commands to verify the result:

```bash
mdvdb config --json
mdvdb doctor --json
```

## Examples

```bash
# Current directory
mdvdb init

# Another collection root
mdvdb --root /path/to/notes init

# User-wide YAML defaults
mdvdb init --global

# Typical first run
mdvdb init
mdvdb embedding probe
mdvdb ingest
mdvdb search "first query"
```

## Related pages

- [Configuration](../configuration.md) — YAML, secrets, providers, and precedence
- [Quick Start](../quickstart.md) — first collection workflow
- [`mdvdb config`](./config.md) — inspect and mutate settings
- [`mdvdb ingest`](./ingest.md) — build or refresh the index
- [`mdvdb doctor`](./doctor.md) — diagnose configuration and provider issues
