<script lang="ts">
  // The Hosts view: a master–detail screen for every host, grouped by Claude
  // account with each account's usage shown once (spec:
  // docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md,
  // "The Hosts view", "Keyboard", "Staleness and failure"). This component
  // owns selection, focus, the keyboard table, the clock and the single
  // endpoint-outage banner; `HostsList` and `HostDetail` render.
  //
  // Keyboard: no type-ahead, stray letters do nothing, letters are ignored
  // while an input has focus, and no bound key is destructive — Rotate and
  // Remove are reachable only by Tab or pointer.
  import { isEditable } from './terminal_keys';
  import { onMount } from 'svelte';
  import { hosts } from './hosts';
  import { accounts, accountByUuid } from './accounts';
  import { sessions } from './sessions';
  import {
    accountUsage,
    refreshAccountUsage,
    type AccountUsageSnapshot,
  } from './account_usage_store';
  import { clock as clockText, refreshCountdown } from './account_usage';
  import { hookHealth } from './hook_health';
  import { hostTokens, hostTokensLoaded, loadHostTokens, reprobeHost } from './host_actions';
  import { copyText } from './clipboard';
  import { pushError } from './toasts';
  import {
    endpointOutage,
    filterGroups,
    groupHostsByAccount,
    hostAttention,
    newestClaudeVersion,
    sessionCounts,
    sharedWith,
    type HostRowInfo,
  } from './hosts_view';
  import AddHostPicker from './AddHostPicker.svelte';
  import HostsList from './HostsList.svelte';
  import HostDetail from './HostDetail.svelte';
  import { hubStatus, hubBlock, hubActionBlocked, ownsTheFleet } from './hub';
  import { hubConnection, connectionBanner } from './hub_connection';

  let {
    preselect = null,
    onClose,
    onFilterSidebar,
    onNewSession,
    onSelectionChange,
    clock = () => Math.floor(Date.now() / 1000),
    locale,
    timeZone,
  }: {
    /** Host alias to select on open. */
    preselect?: string | null;
    /** Esc in the list. */
    onClose: () => void;
    /** `s`: narrow the sidebar to this host. */
    onFilterSidebar: (alias: string) => void;
    /** `n`: start a new session on this host. */
    onNewSession: (alias: string) => void;
    /** The selected host changed (App remembers the last-viewed host). */
    onSelectionChange?: (alias: string) => void;
    /** Unix seconds; injectable for tests. */
    clock?: () => number;
    locale?: string;
    timeZone?: string;
  } = $props();

  const LIST_ID = 'hosts-view-list';
  const TICK_MS = 30_000;

  // One clock for every relative time in the view.
  const readClock = () => clock();
  let now = $state(readClock());
  $effect(() => {
    const t = setInterval(() => (now = readClock()), TICK_MS);
    return () => clearInterval(t);
  });

  let filter = $state('');
  let selectedAlias = $state<string | null>(null);
  let editing = $state<{ uuid: string; where: 'list' | 'detail' } | null>(null);
  let legendOpen = $state(false);
  let showAddPicker = $state(false);
  let probing = $state<string | null>(null);
  /** A floor refusal: `refresh available in m:ss`, counted down from `now`. */
  let refusal = $state<{ alias: string; nextTryAt: number } | null>(null);
  let copied = $state(false);

  let listEl = $state<HTMLElement>();
  let filterEl = $state<HTMLInputElement>();
  let detailEl = $state<HTMLElement>();

  const allGroups = $derived(groupHostsByAccount($hosts, $accounts));
  const groups = $derived(filterGroups(allGroups, filter));
  const ordered = $derived(groups.flatMap((g) => g.hosts));
  const newestClaude = $derived(newestClaudeVersion($hosts));

  const rowInfo = $derived.by(() => {
    const m = new Map<string, HostRowInfo>();
    for (const h of $hosts) {
      const counts = sessionCounts(h.alias, $sessions);
      const attention = hostAttention({
        host: h,
        hasToken: $hostTokens.has(h.alias),
        tokensLoaded: $hostTokensLoaded,
        hook: hookHealth(h.alias, $hostTokens.has(h.alias), $sessions),
        sessionCount: counts.total,
        newestClaude,
      });
      m.set(h.alias, { counts, attention });
    }
    return m;
  });

  /** Accounts at least one host is logged in to: the ones the view shows. */
  const linkedUuids = $derived(allGroups.map((g) => g.accountUuid).filter((u): u is string => !!u));
  const outage = $derived(
    endpointOutage(
      linkedUuids.map((u) => $accountUsage[u]).filter((s): s is AccountUsageSnapshot => !!s),
      now,
      locale,
      timeZone,
    ),
  );

  const selectedHost = $derived($hosts.find((h) => h.alias === selectedAlias) ?? null);
  const selectedAccount = $derived(
    selectedHost?.account_uuid ? ($accountByUuid.get(selectedHost.account_uuid) ?? null) : null,
  );
  const selectedSessions = $derived(
    selectedHost
      ? $sessions
          .filter((s) => s.host_alias === selectedHost.alias)
          .sort((a, b) => a.tmux_name.localeCompare(b.tmux_name))
      : [],
  );
  const onlineCount = $derived($hosts.filter((h) => h.reachable).length);

  function defaultSelection(): string | null {
    if (preselect && ordered.some((h) => h.alias === preselect)) return preselect;
    const attention = ordered.find((h) => !h.reachable || rowInfo.get(h.alias)?.attention);
    return (attention ?? ordered[0])?.alias ?? null;
  }

  // Preselect once hosts are known; re-pick when the selection disappears
  // (removed, or filtered out).
  $effect(() => {
    if (selectedAlias === null || !ordered.some((h) => h.alias === selectedAlias)) {
      selectedAlias = defaultSelection();
    }
  });

  $effect(() => {
    if (selectedAlias !== null) onSelectionChange?.(selectedAlias);
  });

  onMount(() => {
    listEl?.focus();
    // `list_host_tokens` and `refresh_account_usage` are both local-only in
    // remote mode (`host_tokens`, `refresh_account_usage` REASONS) — a hub
    // client must not fire either only to drop an E_LOCAL_ONLY each time.
    // "The Hosts view opening" is a fetch trigger; the backend keeps the
    // usage floor.
    if (ownsTheFleet($hubStatus)) {
      void loadHostTokens();
      for (const uuid of linkedUuids) void refreshAccountUsage(uuid);
    }
  });

  function select(alias: string) {
    selectedAlias = alias;
    refusal = null;
  }

  function move(delta: number | 'home' | 'end') {
    if (ordered.length === 0) return;
    const i = ordered.findIndex((h) => h.alias === selectedAlias);
    let next: number;
    if (delta === 'home') next = 0;
    else if (delta === 'end') next = ordered.length - 1;
    else next = i === -1 ? 0 : Math.min(ordered.length - 1, Math.max(0, i + delta));
    select(ordered[next].alias);
  }

  function moveInDetail(delta: number | 'home' | 'end') {
    const rows = Array.from(detailEl?.querySelectorAll<HTMLElement>('[data-nav-row]') ?? []);
    if (rows.length === 0) return;
    const i = rows.indexOf(document.activeElement as HTMLElement);
    let next: number;
    if (delta === 'home') next = 0;
    else if (delta === 'end') next = rows.length - 1;
    else next = i === -1 ? (delta > 0 ? 0 : rows.length - 1) : Math.min(rows.length - 1, Math.max(0, i + delta));
    rows[next].focus();
  }

  const focusList = () => listEl?.focus();
  const focusDetail = () => detailEl?.focus();

  // Gated in the handlers, not only on the buttons that call them: a
  // keyboard shortcut (`r` / `u` below) reaches these directly, bypassing
  // whatever a button's `disabled` attribute says.
  async function reprobe(alias: string) {
    if (hubActionBlocked('probe_host', $hubStatus, $hubConnection)) return;
    probing = alias;
    await reprobeHost(alias);
    probing = null;
  }

  async function refreshUsage(alias: string) {
    if (hubActionBlocked('refresh_account_usage', $hubStatus, $hubConnection)) return;
    const host = $hosts.find((h) => h.alias === alias);
    const uuid = host?.account_uuid;
    if (!uuid) return;
    now = readClock();
    const snap = $accountUsage[uuid];
    if (snap && refreshCountdown(snap.next_try_at, now) !== null) {
      refusal = { alias, nextTryAt: snap.next_try_at };
      return;
    }
    refusal = null;
    const r = await refreshAccountUsage(uuid);
    if (!r.ok) pushError(r.error, 'Usage refresh failed');
  }

  async function retryOutage() {
    if (hubActionBlocked('refresh_account_usage', $hubStatus, $hubConnection)) return;
    if (!outage) return;
    const r = await Promise.all(outage.accountUuids.map((u) => refreshAccountUsage(u)));
    const failed = r.find((x) => !x.ok);
    if (failed && !failed.ok) pushError(failed.error, 'Usage retry failed');
  }

  async function copyOutage() {
    if (outage) copied = await copyText(outage.copyDetails);
  }

  function startEdit(where: 'list' | 'detail', uuid: string | null | undefined) {
    // The `e` shortcut (below) reaches this directly, bypassing the
    // nickname button's own `disabled={blocked !== null}` — the same
    // non-button-path gap `r`/`u` had.
    if (hubBlock('set_account_nickname', $hubStatus)) return;
    if (uuid && $accountByUuid.has(uuid)) editing = { uuid, where };
  }

  function endEdit() {
    const where = editing?.where;
    editing = null;
    if (where === 'detail') focusDetail();
    else focusList();
  }

  function onFilterKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      move(e.key === 'ArrowDown' ? 1 : -1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      focusList();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (filter) filter = '';
      else focusList();
    }
  }

  function onKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    // A dialog (confirm, add-host) owns its own keys.
    if (e.defaultPrevented || target?.closest?.('dialog')) return;
    if (target === filterEl) return onFilterKeydown(e);
    if (e.metaKey || e.ctrlKey || e.altKey || isEditable(target)) return;

    const inDetail = !!detailEl && !!target && detailEl.contains(target);
    const alias = selectedAlias;
    let handled = true;
    switch (e.key) {
      case 'ArrowDown':
      case 'j':
        if (inDetail) moveInDetail(1);
        else move(1);
        break;
      case 'ArrowUp':
      case 'k':
        if (inDetail) moveInDetail(-1);
        else move(-1);
        break;
      case 'Home':
        if (inDetail) moveInDetail('home');
        else move('home');
        break;
      case 'End':
        if (inDetail) moveInDetail('end');
        else move('end');
        break;
      case 'Enter':
      case 'ArrowRight':
        // Enter on a detail button activates it natively.
        if (inDetail || target !== listEl) handled = false;
        else focusDetail();
        break;
      case 'ArrowLeft':
        if (inDetail) focusList();
        else handled = false;
        break;
      case 'Escape':
        if (legendOpen) legendOpen = false;
        else if (inDetail) focusList();
        else onClose();
        break;
      case 'r':
        if (alias) void reprobe(alias);
        break;
      case 'u':
        if (alias) void refreshUsage(alias);
        break;
      case 's':
        if (alias) onFilterSidebar(alias);
        break;
      case 'n':
        if (alias) onNewSession(alias);
        break;
      case 'e':
        startEdit(inDetail ? 'detail' : 'list', selectedHost?.account_uuid);
        break;
      case '/':
        if (inDetail) handled = false;
        else filterEl?.focus();
        break;
      case '?':
        legendOpen = !legendOpen;
        break;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  }

  // Adding a host is fleet administration: the hub refuses it to a paired
  // client, and this app guards it with `E_LOCAL_ONLY`. Say so on the button
  // rather than after the dialog has been filled in.
  const addHostBlocked = $derived(hubBlock('add_host', $hubStatus));
  const usageRefreshBlocked = $derived(hubBlock('refresh_account_usage', $hubStatus));

  // Same honest-empty-state fix as HostsList's own list: while the hub's
  // wire contract is skewed AND `$hosts` never arrived, "No hosts yet — add
  // one" beside HostsList's now-correct message would say the opposite thing
  // in the same view. Gated on `$hosts.length === 0` like HostsList's own
  // message — a skew discovered mid-session, with real hosts still sitting
  // in the list beside this pane, must not bump "Select a host." for a
  // sentence about a load that already happened.
  const hubSkewEmptyMessage = $derived(
    $hosts.length === 0 &&
      ($hubConnection.state === 'hub_too_old' || $hubConnection.state === 'hub_too_new')
      ? connectionBanner($hubConnection, $hubStatus.url)
      : null,
  );

  const LEGEND: [string, string][] = [
    ['↑ ↓  j k  Home End', 'move the selection (in the detail: between sessions)'],
    ['Enter  →', 'open the detail'],
    ['←  Esc', 'back to the list (Esc in the list closes Hosts)'],
    ['r', 're-probe the host'],
    ['u', "refresh the account's usage (at most every 5 min)"],
    ['s', "view this host's sessions (closes Hosts)"],
    ['n', 'new session on this host'],
    ['e', 'edit the account nickname'],
    ['/', 'filter hosts'],
    ['?', 'this legend'],
    ['Tab', 'the only way to reach Hide, Rotate token and Remove'],
  ];
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="hosts-view" aria-label="Hosts" data-testid="hosts-view" onkeydown={onKeydown}>
  <header class="view-head">
    <h1>Hosts</h1>
    <span class="summary" data-testid="hosts-summary">{$hosts.length} · {onlineCount} online</span>
    <span class="cadence">usage every 5 min</span>
    <span class="grow"></span>
    <button
      type="button"
      class="head-btn"
      data-testid="hosts-add"
      disabled={addHostBlocked !== null}
      title={addHostBlocked ?? ''}
      onclick={() => (showAddPicker = true)}>+ Add host</button>
    <button
      type="button"
      class="head-btn"
      aria-expanded={legendOpen}
      aria-label="Keyboard legend"
      data-testid="hosts-legend-toggle"
      onclick={() => (legendOpen = !legendOpen)}>?</button
    >
  </header>

  {#if outage}
    <div class="banner" role="status" data-testid="usage-outage-banner">
      <span class="banner-text"><span aria-hidden="true">⚠</span> {outage.text}</span>
      <button type="button" class="head-btn" data-testid="outage-copy" onclick={copyOutage}>{copied ? 'Copied' : 'Copy details'}</button>
      <button
        type="button"
        class="head-btn"
        data-testid="outage-retry"
        disabled={usageRefreshBlocked !== null}
        title={usageRefreshBlocked ?? ''}
        onclick={retryOutage}
        >Retry{outage.nextTryAt !== null ? ` ${clockText(outage.nextTryAt, locale, timeZone)}` : ''}</button
      >
    </div>
  {/if}

  {#if legendOpen}
    <div class="legend" data-testid="hosts-legend">
      <dl>
        {#each LEGEND as [keys, what] (keys)}
          <dt><kbd>{keys}</kbd></dt>
          <dd>{what}</dd>
        {/each}
      </dl>
    </div>
  {/if}

  {#if refusal && refreshCountdown(refusal.nextTryAt, now) !== null}
    <p class="notice" role="status" data-testid="hosts-refusal">
      Usage was checked recently — refresh available in {refreshCountdown(refusal.nextTryAt, now)}.
    </p>
  {/if}

  <div class="panes">
    <div class="list-pane">
      <HostsList
        {groups}
        {rowInfo}
        snapshots={$accountUsage}
        {selectedAlias}
        listId={LIST_ID}
        {now}
        {locale}
        {timeZone}
        editingUuid={editing?.where === 'list' ? editing.uuid : null}
        bind:filter
        bind:listEl
        bind:filterEl
        onselect={(a) => {
          select(a);
          focusList();
        }}
        oneditstart={(uuid) => (editing = { uuid, where: 'list' })}
        oneditdone={endEdit}
      />
    </div>
    <div class="detail-pane">
      {#if selectedHost}
        {#key selectedHost.alias}
          <HostDetail
            host={selectedHost}
            account={selectedAccount}
            snapshot={selectedHost.account_uuid ? ($accountUsage[selectedHost.account_uuid] ?? null) : null}
            sharedWith={sharedWith(selectedHost, $hosts)}
            hostSessions={selectedSessions}
            token={$hostTokens.get(selectedHost.alias) ?? null}
            tokensLoaded={$hostTokensLoaded}
            hook={hookHealth(selectedHost.alias, $hostTokens.has(selectedHost.alias), $sessions)}
            attention={rowInfo.get(selectedHost.alias)?.attention ?? null}
            {now}
            {locale}
            {timeZone}
            suppressUnavailable={outage !== null}
            editingNickname={editing?.where === 'detail' && editing.uuid === selectedAccount?.uuid}
            probing={probing === selectedHost.alias}
            bind:detailEl
            oneditstart={() => startEdit('detail', selectedAccount?.uuid)}
            oneditdone={endEdit}
            onreprobe={() => void reprobe(selectedHost.alias)}
            onrefreshusage={() => void refreshUsage(selectedHost.alias)}
          />
        {/key}
      {:else}
        <p class="empty" data-testid="hosts-detail-empty">
          {hubSkewEmptyMessage ?? ($hosts.length === 0 ? 'No hosts yet — add one.' : 'Select a host.')}
        </p>
      {/if}
    </div>
  </div>
</section>

{#if showAddPicker}
  <AddHostPicker onClose={() => (showAddPicker = false)} />
{/if}

<style>
  .hosts-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
  }
  .view-head {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
  }
  h1 { margin: 0; font-size: 1rem; }
  .summary { font-variant-numeric: tabular-nums; }
  .cadence { color: var(--fg-muted); font-size: 0.75rem; }
  .grow { flex: 1; }
  .head-btn {
    font-size: 0.75rem;
    padding: 0.2rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: transparent;
    color: var(--fg);
    cursor: pointer;
    white-space: nowrap;
  }
  .head-btn:hover { border-color: var(--accent); }
  .banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 1rem;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--usage-warn) 12%, transparent);
    font-size: 0.8rem;
  }
  .banner-text { flex: 1; }
  .legend {
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--border);
    font-size: 0.75rem;
  }
  .legend dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    column-gap: 0.8rem;
    row-gap: 0.15rem;
    margin: 0;
  }
  .legend dd { margin: 0; color: var(--fg-muted); }
  kbd {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    white-space: pre;
  }
  .notice {
    margin: 0;
    padding: 0.3rem 1rem;
    font-size: 0.75rem;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
  }
  .panes { display: flex; flex: 1; min-height: 0; }
  .list-pane {
    width: 320px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .detail-pane { flex: 1; min-width: 0; min-height: 0; }
  .empty { margin: 1rem; color: var(--fg-muted); font-size: 0.85rem; }
</style>
