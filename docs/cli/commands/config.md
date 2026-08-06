---
title: "mdvdb config"
description: "Inspect resolved settings and safely modify YAML configuration or dotenv secrets"
category: "commands"
---

# mdvdb config

Inspect the resolved runtime configuration, update a YAML setting by dotted path, or manage a
supported provider secret without putting its value in process arguments.

## Usage

```bash
mdvdb config [OPTIONS]
mdvdb config [OPTIONS] set <KEY> <VALUE>
mdvdb config [OPTIONS] unset <KEY>
mdvdb config [OPTIONS] secret set <NAME> --stdin
mdvdb config [OPTIONS] secret unset <NAME>
```

## Commands and options

| Command | Description |
|---|---|
| `set <KEY> <VALUE>` | Set a project YAML value using a dotted key path |
| `unset <KEY>` | Remove a project override so an inherited value or default applies |
| `secret set <NAME> --stdin` | Read a supported secret from stdin and store it |
| `secret unset <NAME>` | Remove a stored secret |

| Option | Description |
|---|---|
| `--global` | Target user configuration instead of this Collection for mutations |
| `--root <PATH>` | Use another Collection root |
| `--json` | Serialize the resolved configuration when no mutation command is supplied |
| `-v`, `-vv`, `-vvv` | Increase logging detail |
| `--no-color` | Disable colored human output |

`--global` changes the mutation target. With no subcommand, `mdvdb config --global` still shows the
fully merged runtime configuration for the selected Collection.

## Inspect resolved settings

```bash
mdvdb config
mdvdb --json config
mdvdb --root /path/to/collection config
```

Human output is a compact operational summary: provider, model, configured dimensions, batch size,
source directories, chunking, search, watch, clustering, and the user config path. It does not
attribute each setting to its source.

JSON output serializes the complete resolved runtime `Config`, using runtime field names such as
`embedding_provider`, `clustering_algorithm`, and `topics_min_similarity`:

```bash
mdvdb --json config \
  | jq '{provider: .embedding_provider,
         model: .embedding_model,
         dimensions: .embedding_dimensions,
         clustering: .clustering_algorithm}'
```

For `embedding.dimensions: auto`, config JSON remains `"auto"`. The concrete dimension resolved
from an existing index or a live inference appears in `mdvdb status` or `mdvdb embedding probe`;
it is not written back into YAML.

Supported secret values are never serialized in config JSON. The human `API key` presence line
reflects `OPENAI_API_KEY` only and never prints its value; it is not a credential check for the
other providers. Use `mdvdb embedding probe` or `mdvdb doctor` to verify the active provider's
authentication and connectivity.

`--json` suppresses the confirmation text for mutation commands; `set`, `unset`, and `secret`
mutations do not emit a JSON result object.

## Setting YAML values

`set` writes `.markdownvdb/config.yaml` by default:

```bash
mdvdb config set search.limit 20
mdvdb config set search.decay.enabled true
mdvdb config set embedding.dimensions auto
mdvdb config set clustering.algorithm leiden
```

Values are parsed as booleans, integers, floats, structured inline YAML sequences/mappings, or
strings. Quote structured values so the shell passes them as one argument:

```bash
mdvdb config set sources.dirs '[docs, notes]'
mdvdb config set search.decay.exclude '[reference, pinned]'
```

The file is created when needed. Updates hold the configuration lock, preserve unrelated and
unknown YAML keys, and replace the file atomically.

The resulting merged configuration is validated after the write. A validation failure is reported
as a warning, but the value remains written. Correct it with another `set`, use `unset`, or edit
the YAML directly. Syntax-invalid YAML must be repaired directly because configuration is loaded
before the command action runs.

Use the dedicated cluster command for Topic definition arrays:

```bash
mdvdb clusters add Reliability \
  --description "Incidents, recovery, and resilience" \
  --seeds incident,rollback,failover
```

## Removing YAML overrides

`unset` removes only the selected key from the target YAML:

```bash
mdvdb config unset search.limit
mdvdb config unset embedding.endpoint
```

The effective value then comes from the next source in the precedence chain. Removing a missing
key is safe and reports `No override found`.

## User defaults

Pass `--global` to mutate the user-level config:

```bash
mdvdb config --global set search.limit 20
mdvdb config --global unset search.limit
```

The default target is `~/.mdvdb/config.yaml`. If `MDVDB_CONFIG_HOME` is set, mdvdb uses that
directory instead. Project YAML still overrides user YAML for that Collection.

## Managing secrets

Secret values must arrive on stdin and the explicit `--stdin` safety switch is required:

```bash
# Project .markdownvdb/.env
printf '%s' "$OPENROUTER_API_KEY" \
  | mdvdb config secret set OPENROUTER_API_KEY --stdin

# User ~/.mdvdb/.env
printf '%s' "$OPENROUTER_API_KEY" \
  | mdvdb config --global secret set OPENROUTER_API_KEY --stdin

# Remove the project copy
mdvdb config secret unset OPENROUTER_API_KEY
```

mdvdb refuses empty values and unsupported names. Secret files are replaced atomically and use
owner-only permissions on Unix. Values are safely quoted for dotenv parsing and are never echoed,
logged, or returned.

Supported names are:

```text
OPENAI_API_KEY
OPENROUTER_API_KEY
GEMINI_API_KEY
AZURE_OPENAI_API_KEY
AZURE_OPENAI_ACCESS_TOKEN
HF_TOKEN
AWS_BEARER_TOKEN_BEDROCK
AWS_ACCESS_KEY_ID
AWS_SECRET_ACCESS_KEY
AWS_SESSION_TOKEN
OLLAMA_HOST
```

Connection settings that are not secrets belong in YAML. For example, use `embedding.endpoint`
instead of storing `AZURE_OPENAI_ENDPOINT` through this command, and use
`embedding.bedrock.profile` for a named AWS profile.

## Provider map

The resolved `embedding.provider` accepts eight public backends:

| Provider | YAML value | Primary credential or connection |
|---|---|---|
| OpenAI | `openai` | `OPENAI_API_KEY` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |
| Azure OpenAI | `azure` | `embedding.endpoint` plus API key or bearer token |
| AWS Bedrock | `bedrock` | Bedrock bearer token, AWS credentials, or shared profile |
| Hugging Face | `huggingface` | `HF_TOKEN` for serverless; endpoint mode can be private |
| Ollama | `ollama` | `OLLAMA_HOST` |
| OpenAI-compatible | `custom` | Exact `embedding.endpoint`; optional `OPENAI_API_KEY` |

Use `embedding.dimensions: auto` unless you intentionally need to pin a positive dimension. See
[Embedding Providers](../concepts/embedding-providers.md) for nested Azure, Bedrock, Hugging Face,
and purpose configuration.

## Resolution order

Ordinary settings resolve from highest to lowest priority:

1. Shell `MDVDB_*` variables
2. Project `.markdownvdb/config.yaml`
3. User `~/.mdvdb/config.yaml`
4. Built-in defaults

User and project YAML are deep-merged. Mappings merge key by key; higher-priority scalars and
sequences replace lower-priority values. `MDVDB_*` entries found only in dotenv files are ignored:
put ordinary settings in YAML or export them in the shell.

Credentials and connection secrets resolve independently:

1. Shell environment
2. Project-root `.env`
3. Project `.markdownvdb/.env`
4. User `~/.mdvdb/.env`
5. Legacy user `~/.mdvdb/config`

`MDVDB_NO_USER_CONFIG=1` skips user YAML and user secret sources. Legacy dotenv-style mdvdb
configuration is migrated to YAML when loaded; non-`MDVDB_*` secrets are preserved in a sibling
`.env`.

## Common examples

```bash
# One-command CI override; no file mutation
MDVDB_SEARCH_MODE=lexical mdvdb search "exact identifier"

# Change the provider and discover its models
mdvdb config set embedding.provider openrouter
mdvdb embedding models --provider openrouter

# Resolve dimensions with one live inference
mdvdb config set embedding.dimensions auto
mdvdb embedding probe

# Select the K-means fallback
mdvdb config set clustering.algorithm kmeans
mdvdb config set clustering.granularity 1.5
mdvdb ingest
```

## Related pages

- [Configuration](../configuration.md) — canonical YAML and complete precedence reference
- [Embedding Providers](../concepts/embedding-providers.md) — all eight backends
- [Clustering](../concepts/clustering.md) — Leiden, K-means, and Topics
- [`mdvdb init`](./init.md) — create starter YAML
- [`mdvdb embedding`](./embedding.md) — discover models and probe dimensions
- [`mdvdb doctor`](./doctor.md) — validate configuration and provider connectivity
