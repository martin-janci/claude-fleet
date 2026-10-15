<!--
  Every host's CPU, memory and disk (Thread room, boards Main and A4Events):
  the focused host on top with a meter for each, then the others, those in
  trouble first. The status bar's host chip shows it in a popover and
  Control's right panel as its Hosts view. A host's name opens it in the
  Hosts view.
-->
<script lang="ts">
  import Meter from './kit/Meter.svelte';
  import { checkedAgo, loadParts, type HostLoad } from './host_health';

  let {
    loads,
    focus,
    now,
    focusNote = null,
    onopen,
  }: {
    loads: readonly HostLoad[];
    focus: string | null;
    now: number;
    /** Why this host is on top ("follows the selected session"). */
    focusNote?: string | null;
    onopen?: (alias: string) => void;
  } = $props();

  const top = $derived(loads.find((l) => l.alias === focus) ?? null);
  const others = $derived(loads.filter((l) => l !== top));
</script>

{#snippet name(l: HostLoad, cls: string)}
  {#if onopen}
    <button type="button" class="name {cls}" data-testid="host-health-open" title="Open {l.alias} in Hosts" onclick={() => onopen?.(l.alias)}
      >{l.alias}</button
    >
  {:else}
    <span class="name {cls}">{l.alias}</span>
  {/if}
{/snippet}

<div class="list" data-testid="host-health-list">
  {#if top}
    <div class="top" data-testid="host-health-focus">
      <div class="top-head">
        {@render name(top, 'strong')}
        <span class="note">{[focusNote, checkedAgo(top.at, now)].filter(Boolean).join(' · ')}</span>
      </div>
      {#if !top.reachable}
        <p class="trouble">Offline: the last probe did not answer.</p>
      {:else}
        <div class="meters">
          {#each [{ k: 'CPU', v: top.cpu, lv: top.cpuLevel }, { k: 'Memory', v: top.mem, lv: top.memLevel }, { k: 'Disk', v: top.disk, lv: top.diskLevel }] as m (m.k)}
            <span class="k">{m.k}</span>
            {#if m.v != null}
              <Meter value={m.v / 100} level={m.lv} label="{m.k} {m.v}% on {top.alias}" />
              <span class="v">{m.v}%</span>
            {:else}
              <span class="unknown">unknown</span>
              <span class="v">—</span>
            {/if}
          {/each}
        </div>
        {#if top.pressure != null && top.pressure >= 1}<p class="note">Memory pressure {top.pressure}%</p>{/if}
        {#if top.trouble}<p class="trouble" data-testid="host-health-focus-trouble">{top.alias}: {top.trouble}</p>{/if}
      {/if}
    </div>
  {/if}
  {#if others.length > 0}
    <div class="others">
      {#if top}<span class="note">Other hosts</span>{/if}
      {#each others as l (l.alias)}
        <div class="other" data-testid="host-health-row" data-trouble={l.trouble ? 'true' : undefined}>
          {@render name(l, '')}
          <span class="parts">{#each loadParts(l) as p, i (i)}{#if i > 0}{' · '}{/if}<span
                class:warn={!!l.trouble && l.trouble.split(' ')[0] === p.split(' ')[0].toLowerCase()}>{p}</span
              >{/each}</span
          >
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: var(--text-xs);
  }
  .top {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .top-head {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }
  .name {
    border: 0;
    padding: 0;
    background: none;
    color: var(--fg);
    font: inherit;
    text-align: left;
  }
  button.name {
    cursor: pointer;
  }
  button.name:hover {
    text-decoration: underline;
  }
  button.name:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .strong {
    flex: 1 1 auto;
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .note {
    color: var(--fg-muted);
    margin: 0;
  }
  .meters {
    display: grid;
    grid-template-columns: 56px minmax(0, 1fr) 40px;
    gap: 8px 10px;
    align-items: center;
  }
  .k {
    color: var(--fg-2);
  }
  .v {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .unknown {
    color: var(--fg-muted);
  }
  .trouble,
  .warn {
    color: var(--status-waiting);
    margin: 0;
  }
  .others {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: var(--space-2);
    border-top: 1px solid var(--border);
  }
  .top + .others {
    border-top: 1px solid var(--border);
  }
  .other {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .parts {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
</style>
