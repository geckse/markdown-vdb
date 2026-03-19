---
title: "mdvdb doctor"
description: "Run diagnostic checks on configuration, embedding provider connectivity, and index health"
category: "commands"
---

# mdvdb doctor

Run diagnostic checks on the project configuration and index. Validates configuration loading, checks for user-level and project-level config files, verifies API key availability, tests embedding provider connectivity, inspects index integrity, and confirms source directories are accessible.

## Usage

```bash
mdvdb doctor [OPTIONS]
```

## Options

This command has no command-specific options. Only [global options](#global-options) apply.

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Diagnostic Checks

The doctor command runs 7 diagnostic checks in sequence:

| # | Check | What It Validates |
|---|-------|-------------------|
| 1 | **Config loaded** | Configuration was loaded successfully. Displays the active provider, model, and dimensions. Always passes if the command can start. |
| 2 | **User config** | Checks whether a user-level config file exists at `~/.mdvdb/config` (or the `MDVDB_CONFIG_HOME` location). **Pass** if the file exists, **Warn** if not found or home directory cannot be resolved. |
| 3 | **Project config** | Checks whether the `.markdownvdb/` directory exists in the project root. **Pass** if found, **Fail** if missing. |
| 4 | **API key** | Checks whether the required API key is available for the configured provider. For OpenAI: checks `OPENAI_API_KEY`. For Ollama/Custom: always passes. **Fail** if OpenAI is configured but the key is missing. |
| 5 | **Provider reachable** | Sends a test embedding request to the configured provider with a 5-second timeout. **Pass** with response time in milliseconds. **Fail** on error or timeout. |
| 6 | **Index** | Inspects the index for document, chunk, and vector counts. **Pass** if counts are consistent (vectors match chunks). **Warn** if the index is empty or counts are mismatched. |
| 7 | **Source directories** | Discovers markdown files in the configured source directories. **Pass** with directory list and file count. **Fail** if discovery encounters an error. |

### Check Statuses

Each check reports one of three statuses:

| Status | Icon | Meaning |
|--------|------|---------|
| **Pass** | `✓` (green) | Check passed successfully |
| **Warn** | `!` (yellow) | Non-critical issue detected -- may need attention |
| **Fail** | `✗` (red) | Critical issue -- must be fixed for normal operation |

## Human-Readable Output

```
  ● mdvdb doctor

  ✓ Config loaded              OpenAI / text-embedding-3-small / 1536
  ✓ User config                /home/user/.mdvdb/config
  ✓ Project config             .markdownvdb/
  ✓ API key                    OPENAI_API_KEY is set
  ✓ Provider reachable         OK (243ms)
  ✓ Index                      57 docs, 342 chunks, 342 vectors
  ✓ Source directories         ./ (57 .md files)

  7/7 checks passed
```

### Example with Issues

```
  ● mdvdb doctor

  ✓ Config loaded              OpenAI / text-embedding-3-small / 1536
  ! User config                /home/user/.mdvdb/config (not found)
  ✗ Project config             .markdownvdb not found
  ✗ API key                    OPENAI_API_KEY not set
  ✗ Provider reachable         401 Unauthorized
  ! Index                      empty — run `mdvdb ingest` to index your markdown files
  ✓ Source directories         ./ (12 .md files)

  2/7 checks passed
```

## Examples

```bash
# Run all diagnostic checks
mdvdb doctor

# Run diagnostics with JSON output
mdvdb doctor --json

# Run diagnostics for a specific project
mdvdb doctor --root /path/to/project

# Run diagnostics with verbose logging
mdvdb doctor -v
```

## JSON Output

### DoctorResult (`--json`)

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
      "detail": "/home/user/.mdvdb/config"
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
      "detail": "57 docs, 342 chunks, 342 vectors"
    },
    {
      "name": "Source directories",
      "status": "Pass",
      "detail": "./ (57 .md files)"
    }
  ],
  "passed": 7,
  "total": 7
}
```

### DoctorResult Fields

| Field | Type | Description |
|-------|------|-------------|
| `checks` | `DoctorCheck[]` | Array of individual diagnostic check results |
| `passed` | `number` | Number of checks that passed |
| `total` | `number` | Total number of checks run |

### DoctorCheck Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Human-readable name of the check |
| `status` | `string` | Check result: `"Pass"`, `"Fail"`, or `"Warn"` |
| `detail` | `string` | Detail message describing the result |

### CheckStatus Values

| Value | Meaning |
|-------|---------|
| `"Pass"` | Check passed successfully |
| `"Fail"` | Critical failure -- must be addressed |
| `"Warn"` | Non-critical warning -- may need attention |

## Troubleshooting

### Common Failures and Fixes

| Check | Failure | Fix |
|-------|---------|-----|
| **Project config** | `.markdownvdb not found` | Run [`mdvdb init`](./init.md) to create the project config |
| **API key** | `OPENAI_API_KEY not set` | Set `OPENAI_API_KEY` in your shell, `.markdownvdb/.config`, `.env`, or `~/.mdvdb/config`. See [Configuration](../configuration.md). |
| **Provider reachable** | `timeout (5s)` | Check your network connection. For Ollama, verify it is running (`ollama serve`). For OpenAI, check firewall/proxy settings. |
| **Provider reachable** | `401 Unauthorized` | Your API key is invalid or expired. Regenerate it from the provider's dashboard. |
| **Index** | `empty` | Run [`mdvdb ingest`](./ingest.md) to index your markdown files. |
| **Index** | `mismatch` | Vector and chunk counts don't match. Run `mdvdb ingest --reindex` to rebuild the index. |
| **Source directories** | Error message | Verify that `MDVDB_SOURCE_DIRS` paths exist and are readable. |

### Warnings vs Failures

- **Warnings** (`!`) are informational. The system can still function. For example, a missing user config (`~/.mdvdb/config`) simply means no user-level defaults are applied.
- **Failures** (`✗`) indicate issues that will prevent normal operation. For example, a missing API key for OpenAI means embedding calls will fail during ingestion.

## Notes

- The doctor command opens the index in **read-only** mode. It never modifies the index.
- The provider connectivity check sends a single test embedding request with a **5-second timeout**. This makes a real API call and may count against API usage quotas.
- All 7 checks are always run, even if earlier checks fail. This gives you the full picture in a single run.

## Related Commands

- [`mdvdb init`](./init.md) -- Create a configuration file if project config is missing
- [`mdvdb config`](./config.md) -- View the fully resolved configuration values
- [`mdvdb status`](./status.md) -- View index document and vector counts
- [`mdvdb ingest`](./ingest.md) -- Index files to populate an empty index

## See Also

- [Configuration](../configuration.md) -- Config resolution order and all environment variables
- [Embedding Providers](../concepts/embedding-providers.md) -- Provider setup and troubleshooting
- [Index Storage](../concepts/index-storage.md) -- Index file format and `.markdownvdb/` directory
- [Quick Start](../quickstart.md) -- Getting started guide including the doctor step
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference
