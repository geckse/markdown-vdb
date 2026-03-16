<script lang="ts">
  interface Flag {
    name: string;
    short?: string;
    type: string;
    description: string;
  }

  interface Command {
    name: string;
    args?: string;
    description: string;
    flags: Flag[];
    example: { input: string; output: string };
  }

  const commands: Command[] = [
    {
      name: 'search',
      args: '<query>',
      description: 'Semantic search across indexed markdown files',
      flags: [
        { name: '--limit', short: '-l', type: 'number', description: 'Maximum number of results to return' },
        { name: '--min-score', type: 'float', description: 'Minimum similarity score (0.0 to 1.0)' },
        { name: '--filter', short: '-f', type: 'KEY=VALUE', description: 'Metadata filter expression' },
        { name: '--mode', type: 'hybrid|semantic|lexical', description: 'Search mode' },
        { name: '--boost-links', type: 'bool', description: 'Favor results linked to/from top matches' },
        { name: '--path', type: 'string', description: 'Restrict search to files under this path prefix' },
        { name: '--decay', type: 'bool', description: 'Favor recently modified files' },
        { name: '--expand', type: '0-3', description: 'Graph expansion depth for context' },
      ],
      example: {
        input: 'mdvdb search "authentication flow" --limit 5',
        output: `  0.94  docs/auth/oauth.md#setup
         OAuth 2.0 authentication flow with PKCE...
  0.87  docs/auth/sessions.md#tokens
         Session token management and refresh...`,
      },
    },
    {
      name: 'ingest',
      description: 'Ingest markdown files into the index',
      flags: [
        { name: '--reindex', type: 'bool', description: 'Force re-embedding of all files' },
        { name: '--file', type: 'path', description: 'Ingest a specific file only' },
        { name: '--preview', type: 'bool', description: 'Preview what ingestion would do without actually ingesting' },
      ],
      example: {
        input: 'mdvdb ingest --preview',
        output: `  Would index: 12 files (3 new, 9 unchanged)
  Would skip: 45 files (unchanged)`,
      },
    },
    {
      name: 'status',
      description: 'Show index status and configuration',
      flags: [],
      example: {
        input: 'mdvdb status',
        output: `  Index: .markdownvdb/index (2.4 MB)
  Files: 57 indexed | Chunks: 324 | Vectors: 324
  Provider: openai (text-embedding-3-small)`,
      },
    },
    {
      name: 'schema',
      description: 'Show inferred metadata schema',
      flags: [
        { name: '--path', type: 'string', description: 'Restrict schema to files under this path prefix' },
      ],
      example: {
        input: 'mdvdb schema',
        output: `  title     string   100%  (57/57 files)
  tags      array     84%  (48/57 files)
  draft     boolean   21%  (12/57 files)`,
      },
    },
    {
      name: 'clusters',
      description: 'Show document clusters',
      flags: [],
      example: {
        input: 'mdvdb clusters',
        output: `  Cluster 0 (14 docs): authentication, oauth, security
  Cluster 1 (11 docs): api, endpoints, rest
  Cluster 2 (9 docs):  deployment, docker, ci`,
      },
    },
    {
      name: 'tree',
      description: 'Show file tree with sync status indicators',
      flags: [
        { name: '--path', type: 'string', description: 'Restrict tree to files under this path prefix' },
      ],
      example: {
        input: 'mdvdb tree',
        output: `  docs/
    ✓ auth/oauth.md
    ✓ auth/sessions.md
    ✗ guides/new-draft.md`,
      },
    },
    {
      name: 'get',
      args: '<file_path>',
      description: 'Get metadata for a specific file',
      flags: [],
      example: {
        input: 'mdvdb get docs/auth/oauth.md',
        output: `  File: docs/auth/oauth.md
  Title: OAuth 2.0 Setup Guide
  Tags: [authentication, oauth, security]
  Chunks: 6 | Modified: 2 hours ago`,
      },
    },
    {
      name: 'watch',
      description: 'Watch for file changes and re-index automatically',
      flags: [],
      example: {
        input: 'mdvdb watch',
        output: `  Watching for changes... (Ctrl+C to stop)
  [14:32:05] Changed: docs/api/rest.md → re-indexed (3 chunks)`,
      },
    },
    {
      name: 'init',
      description: 'Initialize a new .markdownvdb config file',
      flags: [
        { name: '--global', type: 'bool', description: 'Create user-level config at ~/.mdvdb/config' },
      ],
      example: {
        input: 'mdvdb init',
        output: '  Created .markdownvdb config file',
      },
    },
    {
      name: 'config',
      description: 'Show resolved configuration',
      flags: [],
      example: {
        input: 'mdvdb config',
        output: `  MDVDB_PROVIDER=openai
  MDVDB_MODEL=text-embedding-3-small
  MDVDB_INDEX_DIR=.markdownvdb`,
      },
    },
    {
      name: 'doctor',
      description: 'Run diagnostic checks on config, provider, and index',
      flags: [],
      example: {
        input: 'mdvdb doctor',
        output: `  ✓ Config loaded
  ✓ Provider reachable (openai)
  ✓ Index valid (324 vectors)`,
      },
    },
    {
      name: 'links',
      args: '<file_path>',
      description: 'Show links originating from a file',
      flags: [
        { name: '--depth', type: '1-3', description: 'Link traversal depth (1 = direct, 2-3 = multi-hop)' },
      ],
      example: {
        input: 'mdvdb links docs/auth/oauth.md',
        output: `  → docs/auth/sessions.md (resolved)
  → docs/api/tokens.md (resolved)
  → ../external.md (broken)`,
      },
    },
    {
      name: 'backlinks',
      args: '<file_path>',
      description: 'Show backlinks pointing to a file',
      flags: [],
      example: {
        input: 'mdvdb backlinks docs/auth/oauth.md',
        output: `  ← docs/getting-started.md
  ← docs/security/overview.md
  2 backlinks found`,
      },
    },
    {
      name: 'orphans',
      description: 'Find orphan files with no links',
      flags: [],
      example: {
        input: 'mdvdb orphans',
        output: `  docs/legacy/old-api.md (no incoming or outgoing links)
  docs/drafts/scratch.md (no incoming or outgoing links)
  2 orphan files found`,
      },
    },
    {
      name: 'edges',
      description: 'Show semantic edges between linked files',
      flags: [
        { name: '--relationship', type: 'string', description: 'Filter by relationship type (substring match)' },
      ],
      args: '[file]',
      example: {
        input: 'mdvdb edges docs/auth/oauth.md',
        output: `  oauth.md ←→ sessions.md  (similarity: 0.89)
  oauth.md ←→ tokens.md    (similarity: 0.76)`,
      },
    },
    {
      name: 'graph',
      description: 'Show graph data (nodes, edges, clusters) for visualization',
      flags: [
        { name: '--level', type: 'document|chunk', description: 'Graph granularity level' },
        { name: '--path', type: 'string', description: 'Restrict graph to files under this path prefix' },
      ],
      example: {
        input: 'mdvdb graph --json',
        output: `  {"nodes": [...], "edges": [...], "clusters": [...]}`,
      },
    },
  ];

  let selectedCommand: string = $state('search');
  let isMobile: boolean = $state(false);

  const selected = $derived(commands.find((c) => c.name === selectedCommand)!);

  function setupMediaQuery() {
    if (typeof window === 'undefined') return;
    const mql = window.matchMedia('(max-width: 768px)');
    isMobile = mql.matches;
    const handler = (e: MediaQueryListEvent) => { isMobile = e.matches; };
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }

  $effect(() => {
    return setupMediaQuery();
  });

  function handleKeydown(e: KeyboardEvent) {
    const idx = commands.findIndex((c) => c.name === selectedCommand);
    if (e.key === 'ArrowDown' || e.key === 'ArrowRight') {
      e.preventDefault();
      selectedCommand = commands[Math.min(idx + 1, commands.length - 1)].name;
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') {
      e.preventDefault();
      selectedCommand = commands[Math.max(idx - 1, 0)].name;
    } else if (e.key === 'Enter') {
      e.preventDefault();
    }
  }
</script>

<div class="command-explorer">
  {#if isMobile}
    <div class="mobile-select">
      <label for="command-select" class="sr-only">Select a command</label>
      <select
        id="command-select"
        bind:value={selectedCommand}
      >
        {#each commands as cmd}
          <option value={cmd.name}>{cmd.name} — {cmd.description}</option>
        {/each}
      </select>
    </div>
  {:else}
    <div class="panels">
      <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
      <ul
        class="command-list"
        role="listbox"
        aria-label="CLI commands"
        onkeydown={handleKeydown}
      >
        {#each commands as cmd}
          <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
          <li
            role="option"
            aria-selected={selectedCommand === cmd.name}
            class="command-item"
            class:active={selectedCommand === cmd.name}
            onclick={() => (selectedCommand = cmd.name)}
            tabindex={selectedCommand === cmd.name ? 0 : -1}
          >
            <span class="command-name">{cmd.name}</span>
            <span class="command-desc">{cmd.description}</span>
          </li>
        {/each}
      </ul>

      <div class="command-detail" aria-live="polite">
        {#if selected}
          <h3 class="signature">
            <span class="bin">mdvdb</span> <span class="cmd">{selected.name}</span>{#if selected.args} <span class="args">{selected.args}</span>{/if}
          </h3>
          <p class="description">{selected.description}</p>

          {#if selected.flags.length > 0}
            <div class="flags-section">
              <h4>Flags</h4>
              <table class="flags-table">
                <thead>
                  <tr>
                    <th>Flag</th>
                    <th>Type</th>
                    <th>Description</th>
                  </tr>
                </thead>
                <tbody>
                  {#each selected.flags as flag}
                    <tr>
                      <td class="flag-name">
                        {flag.name}{#if flag.short}, {flag.short}{/if}
                      </td>
                      <td class="flag-type">{flag.type}</td>
                      <td>{flag.description}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}

          <div class="example-section">
            <h4>Example</h4>
            <div class="terminal">
              <div class="terminal-header">
                <span class="terminal-dot"></span>
                <span class="terminal-dot"></span>
                <span class="terminal-dot"></span>
              </div>
              <pre class="terminal-body"><code><span class="prompt">$</span> {selected.example.input}
{selected.example.output}</code></pre>
            </div>
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .command-explorer {
    width: 100%;
    max-width: 960px;
    margin: 0 auto;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .panels {
    display: grid;
    grid-template-columns: 260px 1fr;
    gap: 0;
    border: 1px solid var(--color-border, rgba(139, 92, 246, 0.3));
    border-radius: var(--radius-lg, 12px);
    overflow: hidden;
    background: var(--color-surface, rgba(15, 10, 30, 0.8));
  }

  .command-list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 480px;
    overflow-y: auto;
    border-right: 1px solid var(--color-border, rgba(139, 92, 246, 0.3));
  }

  .command-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 16px;
    cursor: pointer;
    border-left: 3px solid transparent;
    transition: background 0.15s, border-color 0.15s;
  }

  .command-item:hover {
    background: var(--color-surface-hover, rgba(139, 92, 246, 0.08));
  }

  .command-item:hover .command-name {
    color: var(--color-accent, #67E8F9);
  }

  .command-item.active {
    background: var(--color-primary-dim, rgba(139, 92, 246, 0.15));
    border-left-color: var(--color-accent, #67E8F9);
  }

  .command-name {
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--color-text, #E2E8F0);
    transition: color 0.15s;
  }

  .active .command-name {
    color: var(--color-accent, #67E8F9);
  }

  .command-desc {
    font-size: 0.75rem;
    color: var(--color-text-dim, rgba(226, 232, 240, 0.5));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .command-detail {
    padding: 24px;
    min-height: 480px;
  }

  .signature {
    margin: 0 0 12px;
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    font-size: 1.125rem;
    font-weight: 400;
  }

  .bin {
    color: var(--color-text-dim, rgba(226, 232, 240, 0.5));
  }

  .cmd {
    color: var(--color-accent, #67E8F9);
    font-weight: 700;
  }

  .args {
    color: var(--color-secondary, #A78BFA);
  }

  .description {
    color: var(--color-text, #E2E8F0);
    margin: 0 0 24px;
    line-height: 1.5;
  }

  .flags-section h4,
  .example-section h4 {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--color-text-dim, rgba(226, 232, 240, 0.5));
    margin: 0 0 8px;
  }

  .flags-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 24px;
    font-size: 0.8125rem;
  }

  .flags-table th {
    text-align: left;
    padding: 6px 12px 6px 0;
    border-bottom: 1px solid var(--color-border, rgba(139, 92, 246, 0.3));
    color: var(--color-text-dim, rgba(226, 232, 240, 0.5));
    font-weight: 500;
    font-size: 0.75rem;
  }

  .flags-table td {
    padding: 6px 12px 6px 0;
    border-bottom: 1px solid var(--color-border, rgba(139, 92, 246, 0.1));
    color: var(--color-text, #E2E8F0);
  }

  .flag-name {
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    color: var(--color-accent, #67E8F9) !important;
    white-space: nowrap;
  }

  .flag-type {
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    color: var(--color-secondary, #A78BFA) !important;
    font-size: 0.75rem;
  }

  .terminal {
    border-radius: var(--radius-md, 8px);
    overflow: hidden;
    border: 1px solid var(--color-border, rgba(139, 92, 246, 0.3));
  }

  .terminal-header {
    display: flex;
    gap: 6px;
    padding: 8px 12px;
    background: rgba(0, 0, 0, 0.4);
  }

  .terminal-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--color-border, rgba(139, 92, 246, 0.3));
  }

  .terminal-body {
    margin: 0;
    padding: 16px;
    background: rgba(0, 0, 0, 0.6);
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    font-size: 0.8125rem;
    line-height: 1.6;
    color: var(--color-text, #E2E8F0);
    overflow-x: auto;
  }

  .prompt {
    color: var(--color-accent, #67E8F9);
    user-select: none;
  }

  .mobile-select select {
    width: 100%;
    padding: 12px 16px;
    background: var(--color-surface, rgba(15, 10, 30, 0.8));
    color: var(--color-text, #E2E8F0);
    border: 1px solid var(--color-border, rgba(139, 92, 246, 0.3));
    border-radius: var(--radius-md, 8px);
    font-family: var(--font-mono, 'JetBrains Mono', monospace);
    font-size: 0.875rem;
  }
</style>
