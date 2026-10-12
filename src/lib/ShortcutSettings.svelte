<script lang="ts">
  // Settings → Shortcuts (redesign step 7.1, board SettingsMore): every
  // chord in the shortcut registry (`shortcuts.ts`, step 0.3), Mac beside
  // Windows and Linux, grouped by where it works. Read-only: the registry is
  // the one source, so this list cannot drift from what the keys do.
  import { SCOPE_TITLES, SHORTCUTS, bindingsFor, formatBinding, type Scope, type Shortcut } from './shortcuts';

  let query = $state('');

  const chords = (s: Shortcut, isMac: boolean) =>
    bindingsFor(s, isMac)
      .map((b) => formatBinding(b, isMac))
      .join(' · ');

  const groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const out: { scope: Scope; rows: Shortcut[] }[] = [];
    for (const s of SHORTCUTS) {
      const hay = `${s.action} ${chords(s, true)} ${chords(s, false)}`.toLowerCase();
      if (q && !hay.includes(q)) continue;
      let g = out.find((x) => x.scope === s.scope);
      if (!g) out.push((g = { scope: s.scope, rows: [] }));
      g.rows.push(s);
    }
    return out;
  });
</script>

<section class="block" data-testid="shortcuts-section">
  <div class="section-header"><h4>Shortcuts</h4></div>
  <input
    class="filter"
    type="search"
    placeholder="Filter shortcuts"
    aria-label="Filter shortcuts"
    data-testid="shortcuts-filter"
    bind:value={query} />
  {#each groups as g (g.scope)}
    <table class="keys" data-testid={`shortcuts-scope-${g.scope}`}>
      <caption>{SCOPE_TITLES[g.scope]}</caption>
      <thead>
        <tr><th scope="col">Action</th><th scope="col">Mac</th><th scope="col">Windows · Linux</th></tr>
      </thead>
      <tbody>
        {#each g.rows as s (s.id)}
          <tr data-testid={`shortcut-${s.id}`}>
            <td>
              {s.action}
              {#if s.status === 'planned'}<span class="soon" title={`Wired in step ${s.step}`}>coming</span>{/if}
            </td>
            <td><kbd>{chords(s, true) || '—'}</kbd></td>
            <td><kbd>{chords(s, false) || '—'}</kbd></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="none">No shortcut matches.</p>
  {/each}
  <p class="note">
    Single keys act only when no text box has focus. Every key from 0.5.3 does what it did then.
  </p>
</section>

<style>
  .filter {
    width: 100%;
    max-width: 18rem;
    font-size: var(--control-font);
    height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    margin-bottom: var(--space-2);
  }
  .keys {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--control-font);
    margin-bottom: var(--space-3);
  }
  caption {
    text-align: left;
    font-size: var(--control-font-sm);
    font-weight: 500;
    color: var(--fg-muted);
    padding: var(--space-1) 0;
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    padding: 2px var(--space-2) 2px 0;
  }
  td {
    padding: 3px var(--space-2) 3px 0;
    border-top: 1px solid var(--border);
    vertical-align: top;
  }
  td:first-child {
    width: 50%;
  }
  kbd {
    font-family: var(--font-mono);
    font-size: var(--control-font-sm);
  }
  .soon {
    margin-left: var(--space-1);
    font-size: var(--control-font-sm);
    color: var(--fg-muted);
  }
  .none,
  .note {
    font-size: var(--control-font);
    color: var(--fg-muted);
    margin: 0;
  }
</style>
