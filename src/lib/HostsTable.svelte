<script lang="ts">
  import Icon from './kit/Icon.svelte';
  // The Hosts page table (Orbit Fleet redesign step 4.6, Accounts board →
  // "Hosts"). One row per host with its connection, sessions, disk, agent
  // version and the accounts signed in, and the probe facts under its name.
  // Open shows the host's detail (the existing master–detail). Keyboard
  // lives in HostsView, as for the list: this table is ONE focusable element
  // whose active row is announced through aria-activedescendant.
  import type { AccountRow } from './accounts';
  import type { HostRow } from './hosts';
  import type { HostRowInfo } from './hosts_view';
  import type { SessionRow } from './sessions';
  import {
    accountsSignedIn,
    agentCell,
    connectionText,
    diskCell,
    machineLine,
    sessionsCell,
  } from './hosts_table';

  let {
    hosts,
    rowInfo,
    sessions,
    accountByUuid,
    newestClaude,
    selectedAlias,
    now,
    idleSecs,
    tableEl = $bindable(),
    emptyText = null,
    onselect,
    onopen,
  }: {
    /** Already in table order. */
    hosts: HostRow[];
    rowInfo: Map<string, HostRowInfo>;
    sessions: SessionRow[];
    accountByUuid: ReadonlyMap<string, AccountRow>;
    newestClaude: string | null;
    selectedAlias: string | null;
    now: number;
    idleSecs: number;
    tableEl?: HTMLElement;
    /** What the empty table says instead of "No hosts yet" (a hub contract
     *  skew: the list never arrived). */
    emptyText?: string | null;
    onselect: (alias: string) => void;
    onopen: (alias: string) => void;
  } = $props();

  const rowId = (alias: string) => `hosts-table-row-${alias.replace(/[^A-Za-z0-9_-]/g, '_')}`;

  $effect(() => {
    const alias = selectedAlias;
    if (!tableEl || alias === null) return;
    const el = tableEl.querySelector<HTMLElement>(`#${CSS.escape(rowId(alias))}`);
    if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'nearest' });
  });
</script>

<div
  bind:this={tableEl}
  class="hosts-table"
  role="grid"
  tabindex="0"
  aria-label="Hosts"
  aria-activedescendant={selectedAlias !== null ? rowId(selectedAlias) : undefined}
  data-testid="hosts-table"
>
  <div class="row head" role="row">
    <span role="columnheader">Host</span>
    <span role="columnheader">Connection</span>
    <span role="columnheader">Sessions</span>
    <span role="columnheader">Disk</span>
    <span role="columnheader">Agent</span>
    <span role="columnheader">Accounts signed in</span>
    <span role="columnheader"><span class="sr-only">Open</span></span>
  </div>
  {#each hosts as h (h.alias)}
    {@const info = rowInfo.get(h.alias)}
    {@const sess = sessionsCell(h.alias, sessions, { idleSecs, now })}
    {@const disk = diskCell(h)}
    {@const agent = agentCell(h, newestClaude)}
    {@const signedIn = accountsSignedIn(h, accountByUuid)}
    {@const machine = machineLine(h, now)}
    <!-- The grid owns the keyboard (HostsView); a row is only clicked. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      id={rowId(h.alias)}
      class="row"
      class:selected={h.alias === selectedAlias}
      class:off={!h.reachable && h.alias !== 'local'}
      role="row"
      tabindex="-1"
      aria-selected={h.alias === selectedAlias}
      data-testid="hosts-table-row"
      data-alias={h.alias}
      onclick={() => onselect(h.alias)}
      ondblclick={() => onopen(h.alias)}
    >
      <span class="host" role="gridcell">
        <span class="name">
          <span class="glyph" aria-hidden="true">{h.reachable || h.alias === 'local' ? '●' : '○'}</span>
          <span class="alias">{h.alias}</span>
          {#if h.alias === 'local'}<span class="muted">this machine</span>{/if}
          {#if h.hidden}<span class="muted">hidden</span>{/if}
          {#if info?.attention}
            <span class="attention" title={info.attention.title} role="img" aria-label={info.attention.title} data-testid="hosts-table-attention"
              ><Icon name={info.attention.icon} size={12} /></span
            >
          {/if}
        </span>
        {#if machine}<span class="machine" data-testid="hosts-table-machine">{machine}</span>{/if}
      </span>
      <span role="gridcell" class="mono" class:warn={!h.reachable && h.alias !== 'local'} data-testid="hosts-table-connection"
        >{connectionText(h)}</span
      >
      <span role="gridcell" class="mono" data-testid="hosts-table-sessions">
        {sess.total}{#if sess.needsYou}<span class="needs"> · {sess.needsYou} need{sess.needsYou === 1 ? 's' : ''} you</span>{/if}{#if sess.failed}<span
            class="failed"> · {sess.failed} failed</span
          >{/if}
      </span>
      <span role="gridcell" class="mono" class:warn={disk.level === 'warn'} class:crit={disk.level === 'crit'} data-testid="hosts-table-disk"
        >{disk.text}</span
      >
      <span role="gridcell" class="mono" data-testid="hosts-table-agent">
        {agent.text}{#if agent.update}<span class="update" title="Older than the newest Claude Code in the fleet"> · update</span>{/if}
      </span>
      <span role="gridcell" class="accounts" data-testid="hosts-table-accounts">{signedIn.length ? signedIn.join(', ') : 'none'}</span>
      <span role="gridcell" class="open-cell">
        <button
          type="button"
          class="open"
          tabindex="-1"
          data-testid="hosts-table-open"
          onclick={(e) => {
            e.stopPropagation();
            onopen(h.alias);
          }}>Open</button
        >
      </span>
    </div>
  {:else}
    <p class="empty" data-testid="hosts-table-empty">{emptyText ?? 'No hosts yet — add one.'}</p>
  {/each}
</div>

<style>
  .hosts-table {
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
    padding: 0.25rem 0 0.75rem;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(12rem, 2.2fr) minmax(7rem, 1fr) minmax(6rem, 1.1fr) minmax(7rem, 1fr) minmax(5rem, 0.8fr) minmax(8rem, 1.3fr) 4rem;
    align-items: center;
    gap: 0.75rem;
    padding: 0.4rem 1rem;
    font-size: var(--text-2xs);
    border-bottom: 1px solid var(--border);
    cursor: pointer;
  }
  .row.head {
    cursor: default;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .row:not(.head):hover { background: color-mix(in srgb, var(--fg) 5%, transparent); }
  .row.selected { background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .hosts-table:focus-visible .row.selected { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .row.off .alias { color: var(--fg-muted); }
  .host { display: flex; flex-direction: column; gap: 0.1rem; min-width: 0; }
  .name { display: flex; align-items: baseline; gap: 0.4rem; min-width: 0; }
  .alias { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .glyph { font-size: var(--text-2xs); }
  .row.off .glyph { color: var(--fg-muted); }
  .muted, .machine { color: var(--fg-muted); font-size: var(--text-2xs); }
  .machine { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .mono { font-variant-numeric: tabular-nums; white-space: nowrap; }
  .warn, .needs, .update { color: var(--usage-warn); }
  .crit, .failed { color: var(--usage-crit); }
  .accounts { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .attention { font-size: var(--text-2xs); cursor: help; }
  .open-cell { text-align: right; }
  .open {
    font-size: var(--text-2xs);
    padding: 0.15rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .open:hover { border-color: var(--accent); }
  .empty { margin: 1rem; color: var(--fg-muted); font-size: var(--text-xs); }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
