---
title: "Shell Completions"
description: "Generate mdvdb completions for Bash, Zsh, Fish, and PowerShell"
category: "guides"
---

# Shell Completions

mdvdb generates version-matched completion scripts for Bash, Zsh, Fish, and PowerShell. The
generator is the hidden `completions` utility, so it does not appear in `mdvdb --help`.

## Generate a script

```bash
mdvdb completions <SHELL>
```

| Shell | Value |
|-------|-------|
| Bash | `bash` |
| Zsh | `zsh` |
| Fish | `fish` |
| PowerShell | `power-shell` |

The script is written to stdout. Generate it with the same mdvdb binary users will run, and
regenerate it after an upgrade.

## Current command coverage

Generated scripts expose all 21 visible top-level commands from the current CLI:

| Workflow | Commands |
|----------|----------|
| Retrieval and indexing | `search`, `ingest`, `status`, `info`, `collection`, `watch` |
| Metadata and analysis | `schema`, `clusters`, `tree`, `get`, `modules` |
| Setup | `shards`, `init`, `config`, `embedding`, `doctor` |
| Graph | `links`, `backlinks`, `orphans`, `edges`, `graph` |

Beyond top-level names, coverage is shell-specific. Current scripts include a curated selection of
embedding actions, cluster Topic actions, Shard management actions, module actions, collection
query flags, and Shard selectors; PowerShell currently focuses on top-level commands.
`mdvdb <COMMAND> --help` remains authoritative for every option.

## Bash

Load completions for the current session:

```bash
source <(mdvdb completions bash)
```

Install for the current user:

```bash
mkdir -p ~/.local/share/bash-completion/completions
mdvdb completions bash > ~/.local/share/bash-completion/completions/mdvdb
```

Open a new shell after installation. The `bash-completion` package must be installed and loaded.

## Zsh

Create a completion directory and generate `_mdvdb`:

```bash
mkdir -p ~/.zsh/completions
mdvdb completions zsh > ~/.zsh/completions/_mdvdb
```

Add the directory to `fpath` before `compinit` in `~/.zshrc`:

```zsh
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit
compinit
```

Then open a new shell. If an older definition remains cached, remove `~/.zcompdump` once and run
`compinit` again.

## Fish

Install for the current user:

```fish
mkdir -p ~/.config/fish/completions
mdvdb completions fish > ~/.config/fish/completions/mdvdb.fish
```

Fish loads files from that directory automatically. For the current session only:

```fish
mdvdb completions fish | source
```

## PowerShell

Load completions for the current session:

```powershell
mdvdb completions power-shell | Out-String | Invoke-Expression
```

For persistent use, generate a separate script and dot-source it from `$PROFILE`:

```powershell
$profileDir = Split-Path $PROFILE
New-Item -ItemType Directory -Force $profileDir | Out-Null
$completionPath = Join-Path $profileDir "mdvdb-completions.ps1"
mdvdb completions power-shell | Set-Content $completionPath
Add-Content $PROFILE ". `"$completionPath`""
```

Regenerate `mdvdb-completions.ps1` after upgrading instead of appending duplicate completer blocks
to the profile.

## Verify

After loading the script, type `mdvdb ` and request completion. The list should include newer
surfaces such as:

```text
embedding  info  shards  collection  modules  graph
```

You can also inspect the generated script directly:

```bash
mdvdb completions zsh | less
```

If completion does not appear, first confirm the generated output is non-empty, then check the
shell-specific loader (`bash-completion`, Zsh `compinit`, Fish's completion directory, or the
PowerShell profile execution policy).

## Related pages

- [Command Reference](./commands/index.md) -- All commands and options
- [Installation](./installation.md) -- Install or update mdvdb
- [Configuration](./configuration.md) -- Project, user, and shell configuration
