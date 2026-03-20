<script>
  let scrolled = $state(false);

  $effect(() => {
    function onScroll() {
      scrolled = window.scrollY > 40;
    }
    window.addEventListener('scroll', onScroll, { passive: true });
    onScroll();
    return () => window.removeEventListener('scroll', onScroll);
  });

  let mobileOpen = $state(false);
</script>

<header
  class="fixed top-0 left-0 w-full z-50"
  class:header-scrolled={scrolled}
  class:header-transparent={!scrolled}
>
  <nav aria-label="Main navigation" class="mx-auto flex max-w-7xl items-center justify-between px-6 py-3">
    <a href="/" class="brand font-bold text-xl tracking-tight" style="font-family: 'Space Grotesk', sans-serif;">
      Tesseract
    </a>

    <!-- Desktop nav -->
    <div class="hidden md:flex items-center gap-8">
      <a href="#features" class="nav-link">Features</a>
      <a href="#download" class="nav-link">Download</a>
      <a
        href="https://github.com/nicholasgriffintn/markdown-vdb"
        target="_blank"
        rel="noopener noreferrer"
        class="nav-link"
      >
        GitHub
      </a>
      <a
        href="https://github.com/nicholasgriffintn/markdown-vdb"
        target="_blank"
        rel="noopener noreferrer"
        class="cta-button"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="currentColor" class="inline-block mr-1.5 -mt-0.5">
          <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
        </svg>
        Star on GitHub
      </a>
    </div>

    <!-- Mobile CTA -->
    <div class="flex md:hidden items-center gap-3">
      <a
        href="https://github.com/nicholasgriffintn/markdown-vdb"
        target="_blank"
        rel="noopener noreferrer"
        class="cta-button text-sm"
      >
        GitHub
      </a>
      <button
        onclick={() => mobileOpen = !mobileOpen}
        class="p-2 rounded-md text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
        aria-label="Toggle menu"
        aria-expanded={mobileOpen}
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          {#if mobileOpen}
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          {:else}
            <line x1="3" y1="12" x2="21" y2="12" />
            <line x1="3" y1="6" x2="21" y2="6" />
            <line x1="3" y1="18" x2="21" y2="18" />
          {/if}
        </svg>
      </button>
    </div>
  </nav>

  <!-- Mobile menu -->
  {#if mobileOpen}
    <div class="md:hidden border-t border-border/40 bg-background/95 backdrop-blur-xl px-6 py-4 flex flex-col gap-3">
      <a href="#features" class="nav-link" onclick={() => mobileOpen = false}>Features</a>
      <a href="#download" class="nav-link" onclick={() => mobileOpen = false}>Download</a>
      <a
        href="https://github.com/nicholasgriffintn/markdown-vdb"
        target="_blank"
        rel="noopener noreferrer"
        class="nav-link"
      >
        GitHub
      </a>
    </div>
  {/if}
</header>

<style>
  .header-transparent {
    background: transparent;
  }

  .header-scrolled {
    background: hsl(var(--background) / 0.8);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-bottom: 1px solid hsl(var(--border) / 0.4);
    box-shadow: 0 1px 12px hsl(var(--primary) / 0.08), inset 0 -1px 0 hsl(var(--primary) / 0.15);
  }

  header {
    transition: all 0.4s cubic-bezier(0.16, 1, 0.3, 1);
  }

  .brand {
    color: hsl(var(--foreground));
    text-decoration: none;
  }

  .nav-link {
    color: hsl(var(--muted-foreground));
    text-decoration: none;
    font-size: 0.875rem;
    transition: color 0.2s ease;
  }

  .nav-link:hover {
    color: hsl(var(--foreground));
  }

  .nav-link:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px hsl(var(--primary) / 0.5);
    border-radius: 4px;
  }

  .cta-button {
    display: inline-flex;
    align-items: center;
    padding: 0.4rem 1rem;
    font-size: 0.875rem;
    font-weight: 500;
    color: hsl(var(--primary-foreground));
    background: hsl(var(--primary));
    border-radius: 6px;
    text-decoration: none;
    transition: opacity 0.2s ease, box-shadow 0.2s ease;
  }

  .cta-button:hover {
    opacity: 0.9;
    box-shadow: var(--neon-glow);
  }

  .cta-button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px hsl(var(--primary) / 0.5);
  }
</style>
