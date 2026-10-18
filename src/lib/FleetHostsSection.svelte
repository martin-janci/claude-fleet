<script lang="ts">
  // The Hosts page's extra fleets (multi-hub Phase 2): each fleet the filter
  // shows, under its badge, with its hosts as its own hub lists them — reach,
  // versions, sessions — and a Refresh. Read-only: adding, probing and
  // removing a host is its hub's operator's business, done from a window
  // whose primary is that hub (the actions route by owner only for the
  // commands in `FLEET_SCOPED`, and none of the host-management ones are).
  import { visibleExtraFleets, connectionWord, loadFleetRows, retryFleet } from './fleets';
  import { timeAgo } from './session_status';
  import Button from './kit/Button.svelte';

  const groups = $derived(
    $visibleExtraFleets.map(({ fleet, rows }) => {
      const counts = new Map<string, number>();
      for (const s of rows.sessions.values()) {
        if (s.status === 'lost') continue;
        counts.set(s.host_alias, (counts.get(s.host_alias) ?? 0) + 1);
      }
      const list = [...rows.hosts.values()].filter((h) => !h.hidden).sort((a, b) => a.alias.localeCompare(b.alias));
      return { fleet, list, counts, error: rows.error };
    }),
  );
</script>

{#each groups as g (g.fleet.id)}
  {@const cw = connectionWord(g.fleet)}
  <section class="fleet-hosts" style="--fleet: {g.fleet.color}" data-testid="fleet-hosts-{g.fleet.id}">
    <header>
      <span class="badge">{g.fleet.label}</span>
      <span class="muted">{g.list.length} hosts · {g.list.filter((h) => h.reachable).length} online · {cw.word}</span>
      <span class="grow"></span>
      {#if cw.tone === 'bad' || cw.tone === 'warn'}
        <Button size="sm" variant="quiet" onclick={() => void retryFleet(g.fleet.id)}>Retry</Button>
      {/if}
      <Button size="sm" variant="quiet" onclick={() => void loadFleetRows(g.fleet.id)}>Refresh</Button>
    </header>
    {#if g.fleet.duplicate_of}
      <p class="note">Same hub as {g.fleet.duplicate_of}; its hosts are listed there.</p>
    {:else if g.fleet.unavailable}
      <p class="note err">{g.fleet.unavailable}</p>
    {:else if g.error}
      <p class="note err">{g.error}</p>
    {:else if g.list.length === 0}
      <p class="note">{g.fleet.label} lists no hosts.</p>
    {:else}
      <table>
        <thead>
          <tr><th>Host</th><th>Reach</th><th>Sessions</th><th>Claude</th><th>Checked</th></tr>
        </thead>
        <tbody>
          {#each g.list as h (h.alias)}
            <tr data-testid="fleet-host-{g.fleet.id}-{h.alias}">
              <td><code>{h.alias}</code>{#if h.transport === 'agent'} <span class="muted">agent</span>{/if}</td>
              <td class:bad={!h.reachable}>{h.reachable ? 'online' : 'unreachable'}</td>
              <td>{g.counts.get(h.alias) ?? 0}</td>
              <td>{h.claude_version ?? '—'}</td>
              <td class="muted">{h.last_pinged_at ? timeAgo(h.last_pinged_at) : 'never'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
{/each}

<style>
  .fleet-hosts {
    border-left: 3px solid var(--fleet);
    padding: var(--space-2) var(--space-3);
    margin: var(--space-2) 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .badge {
    padding: 0 var(--space-2);
    border-radius: var(--radius-xs);
    border: 1px solid var(--fleet);
    background: color-mix(in srgb, var(--fleet) 16%, transparent);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .grow {
    flex: 1;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
    margin-top: var(--space-1);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-xs);
    padding: 2px var(--space-2) 2px 0;
  }
  td {
    padding: 2px var(--space-2) 2px 0;
    border-top: 1px solid var(--border);
  }
  td.bad {
    color: var(--danger);
  }
  .note {
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
  }
  .err {
    color: var(--danger);
  }
</style>
