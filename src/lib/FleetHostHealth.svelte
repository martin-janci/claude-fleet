<!--
  Host health for the extra fleets (multi-hub): each fleet the sidebar's
  fleet filter shows, under its badge, with its hosts' CPU, memory and disk
  as its own hub reports them. Control's Hosts view and the status bar's
  host chip list these below this window's own hosts. A host's name opens
  nothing: the Hosts view is keyed by this window's aliases, and an extra
  fleet's alias may name another machine there.
-->
<script lang="ts">
  import HostHealthList from './HostHealthList.svelte';
  import { visibleExtraFleets, connectionWord } from './fleets';
  import { fleetLoads, type LoadLimits } from './host_health';

  let {
    now,
    limits,
    focus = null,
    focusNote = null,
  }: {
    now: number;
    limits?: LoadLimits;
    /** The host on top: an extra fleet's session is open on it. */
    focus?: { fleet: string; alias: string } | null;
    focusNote?: string | null;
  } = $props();

  const groups = $derived(
    $visibleExtraFleets.map(({ fleet, rows }) => {
      const mine = focus?.fleet === fleet.id ? focus.alias : null;
      return { fleet, rows, focus: mine, loads: fleetLoads([...rows.hosts.values()], mine, now, limits) };
    }),
  );
</script>

{#each groups as g (g.fleet.id)}
  {@const cw = connectionWord(g.fleet)}
  <section class="fleet" style="--fleet: {g.fleet.color}" data-testid="fleet-host-health-{g.fleet.id}">
    <header>
      <span class="badge">{g.fleet.label}</span>
      {#if cw.tone !== 'ok'}<span class="note">{cw.word}</span>{/if}
    </header>
    {#if g.fleet.duplicate_of}
      <p class="note">Same hub as {g.fleet.duplicate_of}; its hosts are listed there.</p>
    {:else if g.fleet.unavailable}
      <p class="note err">{g.fleet.unavailable}</p>
    {:else if g.rows.error}
      <p class="note err">{g.rows.error}</p>
    {:else if g.loads.length === 0}
      <p class="note">{g.fleet.label} lists no hosts.</p>
    {:else}
      <HostHealthList loads={g.loads} focus={g.focus} {now} focusNote={g.focus ? focusNote : null} />
    {/if}
  </section>
{/each}

<style>
  .fleet {
    border-left: 3px solid var(--fleet);
    padding-left: var(--space-2);
    margin-top: var(--space-3);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-bottom: var(--space-1);
  }
  .badge {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    border: 1px solid var(--fleet);
    background: color-mix(in srgb, var(--fleet) 16%, transparent);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .note {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .err {
    color: var(--danger);
  }
</style>
