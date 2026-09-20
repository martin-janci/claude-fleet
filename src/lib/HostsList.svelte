<script lang="ts">
  // The Hosts view's master list (spec: "The Hosts view" → List). Hosts
  // grouped by Claude account in a stable order; usage is shown once per
  // group. Keyboard handling lives in HostsView: this list is ONE focusable
  // listbox whose active row is announced through aria-activedescendant, like
  // the quick switcher. No destructive controls here — no ×, no 🚫.
  import type { AccountUsageSnapshot } from './account_usage_store';
  import AccountNickname from './AccountNickname.svelte';
  import UsageBar from './UsageBar.svelte';
  import { hubStatus, hubBlock } from './hub';
  import { hubConnection, connectionBanner } from './hub_connection';
  import {
    compactWindow,
    freshnessMark,
    type HostGroup,
    type HostRowInfo,
  } from './hosts_view';

  const nicknameBlocked = $derived(hubBlock('set_account_nickname', $hubStatus));

  // Same honest-empty-state fix as Sidebar.svelte: `E_HUB_CONTRACT` never
  // heals itself, so an empty list here would otherwise read as "no hosts"
  // rather than "this couldn't load". Reuses the connection banner's own
  // sentence.
  const hubSkewEmptyMessage = $derived(
    $hubConnection.state === 'hub_too_old' || $hubConnection.state === 'hub_too_new'
      ? connectionBanner($hubConnection, $hubStatus.url)
      : null,
  );

  let {
    groups,
    rowInfo,
    snapshots,
    selectedAlias,
    listId,
    now,
    locale,
    timeZone,
    editingUuid,
    filter = $bindable(''),
    listEl = $bindable(),
    filterEl = $bindable(),
    onselect,
    oneditstart,
    oneditdone,
  }: {
    groups: HostGroup[];
    rowInfo: Map<string, HostRowInfo>;
    snapshots: Record<string, AccountUsageSnapshot>;
    selectedAlias: string | null;
    listId: string;
    now: number;
    locale?: string;
    timeZone?: string;
    /** The account whose nickname is being edited in a group header. */
    editingUuid: string | null;
    filter?: string;
    listEl?: HTMLElement;
    filterEl?: HTMLInputElement;
    onselect: (alias: string) => void;
    oneditstart: (uuid: string) => void;
    oneditdone: () => void;
  } = $props();

  const MINI = [
    { kind: '5h' as const, short: '5h' },
    { kind: 'weekly' as const, short: 'wk' },
  ];

  function optionIdFor(alias: string): string {
    return `${listId}-opt-${alias.replace(/[^A-Za-z0-9_-]/g, '_')}`;
  }

  // Keep the active row on screen as the selection moves.
  $effect(() => {
    const alias = selectedAlias;
    if (!listEl || alias === null) return;
    const el = listEl.querySelector<HTMLElement>(`#${CSS.escape(optionIdFor(alias))}`);
    if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'nearest' });
  });
</script>

<div class="hosts-list">
  <input
    bind:this={filterEl}
    bind:value={filter}
    class="filter"
    type="search"
    placeholder="Filter hosts  /"
    aria-label="Filter hosts"
    aria-controls={listId}
    data-testid="hosts-filter"
    autocomplete="off"
    spellcheck="false"
  />
  <div
    bind:this={listEl}
    id={listId}
    class="list"
    role="listbox"
    tabindex="0"
    aria-label="Hosts"
    aria-activedescendant={selectedAlias !== null ? optionIdFor(selectedAlias) : undefined}
    data-testid="hosts-list"
  >
    {#each groups as g (g.key)}
      {@const snap = g.accountUuid ? (snapshots[g.accountUuid] ?? null) : null}
      <div class="group" role="group" aria-label={g.label} data-testid="hosts-group" data-key={g.key}>
        <div class="group-header" data-testid="hosts-group-header">
          <div class="group-title">
            {#if g.account}
              <AccountNickname
                account={g.account}
                editing={editingUuid === g.account.uuid}
                onedit={() => oneditstart(g.account!.uuid)}
                ondone={oneditdone}
                testid="group-label"
                blocked={nicknameBlocked}
              />
            {:else}
              <span class="group-label" data-testid="group-label">{g.label}</span>
            {/if}
            {#if g.accountUuid}
              {@const tier = snap?.subscription ?? g.account?.seat_tier ?? null}
              {@const fm = freshnessMark(snap, now)}
              {#if tier}<span class="tier" data-testid="group-tier">{tier}</span>{/if}
              <span class="fresh fresh-{fm.state}" title={fm.title} data-testid="group-freshness">{fm.mark}</span>
            {/if}
          </div>
          {#if g.accountUuid}
            {#each MINI as { kind, short } (kind)}
              {@const cw = compactWindow(kind, snap, now, locale, timeZone)}
              {@const win = snap?.usage ? (kind === '5h' ? snap.usage.five_hour : snap.usage.seven_day) : null}
              <div class="mini" data-testid="group-usage-{kind}">
                <span class="mini-name">{short}</span>
                <UsageBar
                  window={kind}
                  {win}
                  fetchedAt={snap?.fetched_at ?? null}
                  {now}
                  hasExtraUsage={g.account?.has_extra_usage ?? false}
                  compact
                />
                <span class="mini-left" class:muted={cw.freshness !== 'fresh'}>{cw.left}</span>
                {#if cw.reset}<span class="mini-reset">· {cw.reset}</span>{/if}
              </div>
            {/each}
          {/if}
        </div>
        {#each g.hosts as h (h.alias)}
          {@const info = rowInfo.get(h.alias)}
          <!-- The listbox owns the keyboard (HostsView); a row is only clicked. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            id={optionIdFor(h.alias)}
            class="host-row"
            class:selected={h.alias === selectedAlias}
            class:hidden-host={h.hidden}
            role="option"
            tabindex="-1"
            aria-selected={h.alias === selectedAlias}
            data-testid="host-row"
            data-alias={h.alias}
            onclick={() => onselect(h.alias)}
          >
            <span class="glyph" class:off={!h.reachable} aria-hidden="true">{h.reachable ? '●' : '○'}</span>
            <span class="alias">{h.alias}</span>
            {#if !h.reachable}<span class="word-offline" data-testid="host-offline">offline</span>{/if}
            {#if h.hidden}<span class="word-hidden">hidden</span>{/if}
            {#if h.transport === 'agent'}<span class="word-agent" data-testid="host-transport-agent">agent</span>{/if}
            <span class="spacer"></span>
            {#if info?.attention}
              <span class="attention" title={info.attention.title} aria-label={info.attention.title} data-testid="host-attention" data-kind={info.attention.kind}
                >{info.attention.glyph}</span
              >
            {/if}
            {#if info}
              <span class="counts" title={info.counts.title} data-testid="host-counts">{info.counts.text}</span>
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <p class="empty" data-testid="hosts-empty">{filter ? `No host matches “${filter}”.` : (hubSkewEmptyMessage ?? 'No hosts yet.')}</p>
    {/each}
  </div>
</div>

<style>
  .hosts-list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }
  .filter {
    font: inherit;
    font-size: 0.8rem;
    margin: 0.5rem;
    padding: 0.25rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
    color: var(--fg);
  }
  .filter:focus { outline: none; border-color: var(--accent); }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    outline: none;
    padding-bottom: 0.5rem;
  }
  .list:focus-visible .host-row.selected { outline: 1px solid var(--accent); outline-offset: -1px; }
  .group + .group { margin-top: 0.4rem; }
  .group-header {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: 0.4rem 0.6rem 0.3rem;
    border-top: 1px solid var(--border);
    font-size: 0.75rem;
  }
  .group-title {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
  }
  .group-label { font-weight: 600; color: var(--fg-muted); }
  .tier { color: var(--fg-muted); font-size: 0.7rem; }
  .fresh {
    margin-left: auto;
    color: var(--fg-muted);
    font-size: 0.7rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .fresh-expired { color: var(--usage-warn); }
  .mini {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.7rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    min-width: 0;
  }
  .mini-name { width: 1.3rem; color: var(--fg-muted); }
  .mini-left { font-weight: 600; }
  .mini-left.muted { color: var(--fg-muted); font-weight: 400; }
  .mini-reset { color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; }
  .host-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.25rem 0.6rem 0.25rem 1rem;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .host-row:hover { background: color-mix(in srgb, var(--fg) 5%, transparent); }
  .host-row.selected { background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .host-row.hidden-host .alias { color: var(--fg-muted); }
  .glyph { font-size: 0.65rem; color: var(--fg); }
  .glyph.off { color: var(--fg-muted); }
  .alias { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .word-offline { color: var(--usage-warn); font-size: 0.7rem; }
  .word-hidden { color: var(--fg-muted); font-size: 0.7rem; }
  .word-agent { color: var(--accent); font-size: 0.7rem; }
  .spacer { flex: 1; }
  .attention { font-size: 0.75rem; cursor: help; }
  .counts { color: var(--fg-muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
  .empty { margin: 0.6rem; font-size: 0.8rem; color: var(--fg-muted); }
</style>
