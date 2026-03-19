---
title: "Shell Completions"
description: "Setup instructions for tab completions in bash, zsh, fish, and PowerShell"
category: "guides"
---

# Shell Completions

mdvdb includes a built-in shell completions generator that provides tab completion for all commands, subcommands, and flags. Completions are available for **bash**, **zsh**, **fish**, and **PowerShell**.

## Generating Completions

The `completions` command is a hidden utility command (it does not appear in `mdvdb --help`). To generate completions, run:

```bash
mdvdb completions <shell>
```

Where `<shell>` is one of:

| Shell | Value |
|-------|-------|
| Bash | `bash` |
| Zsh | `zsh` |
| Fish | `fish` |
| PowerShell | `power-shell` |

The command writes the completion script to stdout, so you can redirect it to a file or pipe it to your shell's eval.

## Bash

### Install Completions

Generate the completion script and save it to your bash completions directory:

```bash
# System-wide (requires root)
mdvdb completions bash > /etc/bash_completion.d/mdvdb

# User-level (recommended)
mkdir -p ~/.local/share/bash-completion/completions
mdvdb completions bash > ~/.local/share/bash-completion/completions/mdvdb
```

### Load Immediately (Current Session)

To use completions in the current session without restarting your shell:

```bash
source <(mdvdb completions bash)
```

Or add this line to your `~/.bashrc` to load completions on every new session:

```bash
# Add to ~/.bashrc
eval "$(mdvdb completions bash)"
```

### What You Get

After installation, pressing `Tab` will complete:

- Subcommands: `mdvdb se<Tab>` completes to `mdvdb search`
- Global flags: `mdvdb --<Tab>` shows `--help`, `--version`, `--verbose`, `--root`, `--json`, `--no-color`
- Command-specific flags: `mdvdb search --<Tab>` shows `--limit`, `--filter`, `--mode`, `--path`, etc.
- File paths: `mdvdb get <Tab>` completes file paths
- Shell types: `mdvdb completions <Tab>` shows `bash`, `zsh`, `fish`, `power-shell`
- Init flags: `mdvdb init --<Tab>` shows `--global`, `--help`

## Zsh

### Install Completions

Generate the completion script and save it to a directory in your `$fpath`:

```bash
# Create completions directory if it doesn't exist
mkdir -p ~/.zsh/completions

# Generate the completion script
mdvdb completions zsh > ~/.zsh/completions/_mdvdb
```

Make sure `~/.zsh/completions` is in your `fpath`. Add this to your `~/.zshrc` **before** `compinit` is called:

```zsh
# Add to ~/.zshrc (before compinit)
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit && compinit
```

If you use Oh My Zsh, you can place the file in `~/.oh-my-zsh/completions/`:

```bash
mkdir -p ~/.oh-my-zsh/completions
mdvdb completions zsh > ~/.oh-my-zsh/completions/_mdvdb
```

### Rebuild Completion Cache

After installing, rebuild the zsh completion cache:

```bash
rm -f ~/.zcompdump && compinit
```

Or simply open a new terminal session.

### What You Get

Zsh completions include descriptions for each subcommand and flag:

```
$ mdvdb <Tab>
search    -- Semantic search across indexed markdown files
ingest    -- Ingest markdown files into the index
status    -- Show index status and configuration
schema    -- Show inferred metadata schema
clusters  -- Show document clusters
tree      -- Show file tree with sync status indicators
...
```

Command-specific flags are also completed with descriptions. For example, `mdvdb search --<Tab>` shows all search-specific flags with their descriptions.

## Fish

### Install Completions

Generate the completion script and save it to fish's completions directory:

```bash
# User-level (recommended)
mdvdb completions fish > ~/.config/fish/completions/mdvdb.fish

# System-wide (requires root)
mdvdb completions fish > /usr/share/fish/vendor_completions.d/mdvdb.fish
```

Fish automatically loads completions from these directories -- no additional configuration is needed.

### Load Immediately (Current Session)

To use completions in the current session:

```fish
mdvdb completions fish | source
```

### What You Get

Fish completions provide rich descriptions for all subcommands and flags:

- Subcommands with descriptions: `mdvdb <Tab>` shows all commands with tooltips
- Global flags: `--verbose`, `--root`, `--json`, `--no-color` available on every subcommand
- Search flags: `--limit`, `--filter`, `--mode`, `--semantic`, `--lexical`, `--path`, `--decay`, `--boost-links`, `--hops`, `--expand`, and more
- Init flags: `--global`
- Completions shell types: `bash`, `zsh`, `fish`, `power-shell`

## PowerShell

### Install Completions

Add the completion script to your PowerShell profile:

```powershell
# Generate and append to your PowerShell profile
mdvdb completions power-shell >> $PROFILE
```

If your profile file doesn't exist yet, create it first:

```powershell
# Create profile if needed
if (!(Test-Path -Path $PROFILE)) {
    New-Item -ItemType File -Path $PROFILE -Force
}

# Append completions
mdvdb completions power-shell >> $PROFILE
```

### Load Immediately (Current Session)

To use completions in the current session without restarting PowerShell:

```powershell
mdvdb completions power-shell | Invoke-Expression
```

### What You Get

PowerShell completions use `Register-ArgumentCompleter` to provide tab completion for all subcommands with tooltips. Pressing `Tab` or `Ctrl+Space` shows available subcommands with their descriptions.

## Verifying Completions

After installing completions for your shell, verify they work:

1. Open a new terminal session (or source the completions in your current session)
2. Type `mdvdb ` and press `Tab`
3. You should see a list of available subcommands

If completions don't appear:

- **Bash**: Ensure `bash-completion` is installed (`apt install bash-completion` or `brew install bash-completion@2`)
- **Zsh**: Ensure `compinit` is called in your `.zshrc` and the `_mdvdb` file is in `$fpath`
- **Fish**: Ensure the file is in `~/.config/fish/completions/` with a `.fish` extension
- **PowerShell**: Ensure your `$PROFILE` script is not blocked by execution policy (`Set-ExecutionPolicy RemoteSigned -Scope CurrentUser`)

## Updating Completions

When you update mdvdb to a new version, regenerate the completions to pick up any new commands or flags:

```bash
# Bash
mdvdb completions bash > ~/.local/share/bash-completion/completions/mdvdb

# Zsh
mdvdb completions zsh > ~/.zsh/completions/_mdvdb && rm -f ~/.zcompdump

# Fish
mdvdb completions fish > ~/.config/fish/completions/mdvdb.fish

# PowerShell (replace the block in your $PROFILE)
```

## Related

- [Installation](./installation.md) -- Install mdvdb on your system
- [Quick Start](./quickstart.md) -- Get started with mdvdb in 5 minutes
- [Command Reference](./commands/index.md) -- Browse all available commands
- [Configuration](./configuration.md) -- Configure mdvdb behavior and environment variables
