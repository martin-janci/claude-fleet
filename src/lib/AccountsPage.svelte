<script lang="ts">
  // The Accounts & hosts page (Orbit Fleet redesign step 4.1, the Accounts
  // board). It opens on the overview: every Claude account as a card with
  // its 5-hour and weekly windows, its sessions and, at a limit, its paused
  // sessions and a switch to another account; the hosts table below. A card
  // opens the account's detail — plan, the windows with their reset times
  // and history, the hosts and login profiles signed in to it, and the
  // sessions running on it. Usage comes from the same snapshots the Hosts
  // view shows (`account_usage_store`); history from `account_usage_history`.
  import { accounts, accountByUuid } from './accounts';
  import { hosts } from './hosts';
  import { restartSession, sessions, type SessionRow } from './sessions';
  import { accountUsage, refreshAccountUsage } from './account_usage_store';
  import {
    checkedAgo,
    formatReset,
    formatResetShort,
    freshness,
    leftPct,
    severityLeft,
    severity,
    severityBadge,
    windowOf,
    type UsageWindowKind,
  } from './account_usage';
  import {
    accountSummaries,
    defaultAccountUuid,
    historyPoints,
    HISTORY_SPAN_SECS,
    limitOf,
    loadUsageHistory,
    pausedSessions,
    peakUsed,
    sparkPath,
    switchCandidate,
    usageRefreshedAt,
    type AccountSummary,
    type UsageSnapshotRow,
  } from './accounts_page';
  import { displayName, type AttentionLimit } from './attention';
  import { attentionFacts } from './attention_facts';
  import { showFriendlyNames } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { push, pushError } from './toasts';
  import { accountsPageRequest } from './account_pill';
  import { requestHostsView, requestNewSessionOnHost } from './app_views';
  import { moveToHeadroom, resetText } from './account_limits';
  import { hubStatus, hubBlock, hubActionBlocked, ownsTheFleet } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import { hostRowInfos, newestClaudeVersion } from './hosts_view';
  import { tableOrder } from './hosts_table';
  import { hookHealth } from './hook_health';
  import { hostTokens, hostTokensLoaded, loadHostTokens } from './host_actions';
  import { attentionIdleMinutes } from './notify';
  import { fleetSettings, SETTING_KEYS, settingInt, settingSecs } from './fleet_settings';
  import { healthCheck } from './ipc';
  import HostsTable from './HostsTable.svelte';
  import AddHostWizard from './AddHostWizard.svelte';
  import LimitActions from './LimitActions.svelte';
  import Meter from './kit/Meter.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { onMount, untrack } from 'svelte';

  let {
    clock = () => Math.floor(Date.now() / 1000),
    locale,
    timeZone,
  }: {
    clock?: () => number;
    locale?: string;
    timeZone?: string;
  } = $props();

  let now = $state(untrack(() => clock()));
  $effect(() => {
    const t = setInterval(() => (now = clock()), 30_000);
    return () => clearInterval(t);
  });

  const list = $derived(accountSummaries($accounts, $hosts, $sessions, $accountUsage));
  let picked = $state<string | null>(null);
  /** The page opens on the overview; a card (or a pill elsewhere) opens one
   *  account's detail, and "← Accounts & hosts" goes back. */
  let detailOpen = $state(false);
  // A pill elsewhere (step 4.3) asked for one account: show it, then clear
  // the request so a later visit keeps whatever was picked by hand.
  $effect(() => {
    const req = $accountsPageRequest;
    if (req === null) return;
    picked = req;
    detailOpen = true;
    accountsPageRequest.set(null);
  });
  // Nothing picked yet: the first account. A pick that names an account the
  // list does not hold (a pill for an account this fleet has no row for)
  // selects nothing and says so — never a different account (review r05).
  const selected: AccountSummary | null = $derived(
    picked === null ? (list[0] ?? null) : (list.find((a) => a.uuid === picked) ?? null),
  );
  const pickedMissing = $derived(picked !== null && selected === null);

  // History for the selected account, re-read when the selection changes or
  // its usage moves (a new snapshot was just written).
  let history = $state<UsageSnapshotRow[]>([]);
  let historyFor = $state<string | null>(null);
  const selUuid = $derived(selected?.uuid ?? null);
  const selFetchedAt = $derived(selected?.usage?.fetched_at ?? null);
  $effect(() => {
    const uuid = selUuid;
    void selFetchedAt;
    if (!uuid) {
      history = [];
      historyFor = null;
      return;
    }
    let live = true;
    const since = clock() - HISTORY_SPAN_SECS.weekly;
    void loadUsageHistory(uuid, since).then((r) => {
      if (!live) return;
      history = r.ok && Array.isArray(r.value) ? r.value : [];
      historyFor = uuid;
    });
    return () => {
      live = false;
    };
  });

  let refreshing = $state(false);
  async function refresh(uuid: string) {
    refreshing = true;
    const r = await refreshAccountUsage(uuid);
    refreshing = false;
    if (!r.ok) pushError(r.error);
  }

  interface WindowView {
    kind: UsageWindowKind;
    title: string;
    left: number | null;
    level: string;
    badge: string | null;
    reset: string;
    resetShort: string;
    /** The reading no longer holds (too old, or past its reset): `? left`. */
    unknown: boolean;
  }

  function windowView(a: AccountSummary, kind: UsageWindowKind): WindowView {
    const w = windowOf(a.usage?.usage ?? null, kind);
    const title = kind === '5h' ? '5-hour window' : 'Week';
    if (!w) {
      return { kind, title, left: null, level: 'none', badge: null, reset: 'no reading yet', resetShort: '', unknown: false };
    }
    // As `compactWindow`: a number past its reset or freshness limit is
    // withheld, never shown as `0% left` with a LIMIT badge.
    if (freshness(kind, a.usage?.fetched_at ?? null, w.resets_at, now) === 'expired') {
      return { kind, title, left: null, level: 'none', badge: null, reset: '', resetShort: '', unknown: true };
    }
    const left = leftPct(w);
    const level = severity(kind, severityLeft(w), w.resets_at, now, a.account.has_extra_usage);
    const b = severityBadge(level, a.account.has_extra_usage);
    return {
      kind,
      title,
      left,
      level,
      badge: b ? `${b.glyph} ${b.word}` : null,
      reset: formatReset(kind, w.resets_at, now, locale, timeZone),
      resetShort: formatResetShort(kind, w.resets_at, now, locale, timeZone),
      unknown: false,
    };
  }

  // ── the overview (Accounts board) ──

  const defaultUuid = $derived(defaultAccountUuid($hosts));
  const refreshedAt = $derived(usageRefreshedAt(list));
  const countLine = $derived(
    [
      `${list.length} ${list.length === 1 ? 'account' : 'accounts'}`,
      `${$hosts.length} ${$hosts.length === 1 ? 'host' : 'hosts'}`,
      refreshedAt !== null ? checkedAgo(refreshedAt, now).replace('checked', 'usage refreshed') : null,
    ]
      .filter(Boolean)
      .join(' · '),
  );
  /** Accounts a host is logged in to: the ones a usage refresh can read. */
  const linked = $derived(list.filter((a) => a.logins.length > 0).map((a) => a.uuid));
  const refreshAllBlocked = $derived(hubBlock('refresh_account_usage', $hubStatus));
  const addHostBlocked = $derived(hubBlock('add_host', $hubStatus));
  let showAddHost = $state(false);

  async function refreshAll() {
    if (refreshAllBlocked !== null || refreshing) return;
    refreshing = true;
    const r = await Promise.all(linked.map((u) => refreshAccountUsage(u)));
    refreshing = false;
    const failed = r.find((x) => !x.ok);
    if (failed && !failed.ok) pushError(failed.error, 'Usage refresh failed');
  }

  function openDetail(uuid: string) {
    picked = uuid;
    detailOpen = true;
  }

  /** A card's click opens its detail, unless it landed on one of the card's
   *  own controls (Show, Switch, a paused session). */
  function onCardClick(e: MouseEvent, uuid: string) {
    if ((e.target as HTMLElement | null)?.closest('button, a, input, select')) return;
    openDetail(uuid);
  }

  /** Which limited cards show their paused sessions. */
  let pausedOpen = $state<Set<string>>(new Set());
  function togglePaused(uuid: string) {
    const next = new Set(pausedOpen);
    if (next.has(uuid)) next.delete(uuid);
    else next.add(uuid);
    pausedOpen = next;
  }

  const accountName = (uuid: string) => list.find((a) => a.uuid === uuid)?.label ?? uuid.slice(0, 8);

  /**
   * "Switch to <account>" (step 4.4's bulk Switch account, from the card):
   * each paused session this person may restart resumes under the login on
   * its host with the most headroom; the rest stay as they are. The same
   * `moveToHeadroom` the sidebar's select mode runs.
   */
  let switching = $state<string | null>(null);
  const restartHubBlocked = $derived(hubActionBlocked('restart_session', $hubStatus, $hubConnection));
  function switchBlocked(paused: readonly SessionRow[]): string | null {
    if (restartHubBlocked !== null) return restartHubBlocked;
    return paused.length > 0 && bulkTargets(paused, 'restart_session', $sessionBlocked).length === 0
      ? 'None of these sessions is yours to restart.'
      : null;
  }
  async function switchPaused(a: AccountSummary, paused: readonly SessionRow[]) {
    if (switching !== null || switchBlocked(paused) !== null) return;
    switching = a.uuid;
    const r = await moveToHeadroom(bulkTargets(paused, 'restart_session', $sessionBlocked), restartSession);
    switching = null;
    const parts = [
      r.moved > 0 ? `Switched ${r.moved} session${r.moved === 1 ? '' : 's'}` : 'Nothing switched',
      r.stayed > 0 ? `${r.stayed} still under the line` : '',
      r.nowhere > 0 ? `${r.nowhere} with no other login that has room` : '',
      r.failed > 0 ? `${r.failed} failed` : '',
    ].filter(Boolean);
    push({ message: parts.join(' · '), kind: r.failed > 0 ? 'error' : r.moved > 0 ? 'success' : 'info' });
  }

  /** A card's window line: "75% left · resets 14:20", or at the limit
   *  "Limit reached · resets Fri 11:00". */
  function cardWindowText(w: WindowView, limit: AttentionLimit | null): string {
    if (limit) return limit.resets_at !== null ? `Limit reached · resets ${resetText(limit.resets_at)}` : 'Limit reached';
    if (w.unknown) return '? left';
    if (w.left === null) return 'no reading yet';
    return w.resetShort ? `${w.left}% left · ${w.resetShort}` : `${w.left}% left`;
  }

  /** The kit Meter's level for a window's severity. */
  function meterLevel(level: string): 'ok' | 'warn' | 'crit' {
    return level === 'caution' ? 'warn' : level === 'low' || level === 'limit' ? 'crit' : 'ok';
  }

  /** The card's health dot: at a limit, low, fine, or no reading. */
  function cardState(a: AccountSummary, five: WindowView, week: WindowView): 'failed' | 'waiting' | 'done' | 'idle' {
    if (limitOf(a.uuid, $attentionFacts, now)) return 'failed';
    if (five.left === null && week.left === null) return 'idle';
    if ([five.level, week.level].some((l) => l === 'low' || l === 'limit' || l === 'caution')) return 'waiting';
    return 'done';
  }

  // The hosts table, as the Hosts view draws it.
  const versionMaxAge = $derived(settingSecs($fleetSettings, SETTING_KEYS.healthVersionMaxAgeSecs));
  const diskLowPct = $derived(settingInt($fleetSettings, SETTING_KEYS.healthDiskLowPct));
  const newestClaude = $derived(newestClaudeVersion($hosts, now, versionMaxAge));
  let hubVersion = $state<string | null>(null);
  const tableHosts = $derived(tableOrder($hosts));
  const rowInfo = $derived(
    hostRowInfos({
      hosts: $hosts,
      sessions: $sessions,
      tokens: $hostTokens,
      tokensLoaded: $hostTokensLoaded,
      hookOf: (alias, hasToken) => hookHealth(alias, hasToken, $sessions),
      newestClaude,
      now,
      versionMaxAgeSecs: versionMaxAge,
      diskLowPct,
      hubVersion,
    }),
  );
  let selectedHost = $state<string | null>(null);
  let tableEl = $state<HTMLElement>();

  onMount(() => {
    void healthCheck().then((r) => {
      if (r.ok && r.value?.version) hubVersion = r.value.version;
    });
    // `list_host_tokens` is local-only in remote mode, as in the Hosts view.
    if (ownsTheFleet($hubStatus) && !$hostTokensLoaded) void loadHostTokens();
  });

  /** The table's keys (the Hosts view's, the few that apply here): move,
   *  and Enter opens the host in the Hosts view. */
  function onTableKeydown(e: KeyboardEvent) {
    if (e.target !== tableEl || e.metaKey || e.ctrlKey || e.altKey) return;
    const i = tableHosts.findIndex((h) => h.alias === selectedHost);
    if (e.key === 'ArrowDown' || e.key === 'j' || e.key === 'ArrowUp' || e.key === 'k') {
      e.preventDefault();
      const d = e.key === 'ArrowDown' || e.key === 'j' ? 1 : -1;
      const next = tableHosts[Math.min(tableHosts.length - 1, Math.max(0, i + d))];
      if (next) selectedHost = next.alias;
    } else if ((e.key === 'Enter' || e.key === 'ArrowRight') && selectedHost) {
      e.preventDefault();
      requestHostsView(selectedHost);
    }
  }

  const SPARK_W = 240;
  const SPARK_H = 36;

  function onListKeydown(e: KeyboardEvent) {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const i = list.findIndex((a) => a.uuid === selected?.uuid);
    const next = list[Math.min(list.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)))];
    if (next) {
      picked = next.uuid;
      (document.querySelector(`[data-account="${next.uuid}"]`) as HTMLElement | null)?.focus();
    }
  }
</script>

<section class="accounts" data-testid="accounts-page" aria-label="Accounts and hosts">
  <header class="head">
    {#if detailOpen && list.length > 0}
      <button type="button" class="btn" data-testid="accounts-back" onclick={() => (detailOpen = false)}
        >← Accounts &amp; hosts</button
      >
    {/if}
    <h2>Accounts &amp; hosts</h2>
    <span class="sub" data-testid="accounts-count">{countLine}</span>
    <span class="grow"></span>
    <button
      type="button"
      class="btn"
      data-testid="accounts-refresh"
      disabled={refreshing || refreshAllBlocked !== null || linked.length === 0}
      title={refreshAllBlocked ?? 'Read every account’s usage again'}
      onclick={refreshAll}>Refresh</button
    >
    <button
      type="button"
      class="btn"
      data-testid="accounts-add-host"
      disabled={addHostBlocked !== null}
      title={addHostBlocked ?? ''}
      onclick={() => (showAddHost = true)}>+ Add host</button
    >
    <!-- Review r08: the rail item is "Accounts & hosts", and Classic's Hosts
         tab was always in view; the Hosts view is one click from here. -->
    <button type="button" class="btn" data-testid="accounts-all-hosts" onclick={() => requestHostsView()}
      >All hosts ›</button
    >
  </header>

  {#if !detailOpen || list.length === 0}
    <div class="overview" data-testid="accounts-overview">
      <h3 class="section-label">Claude accounts</h3>
      {#if list.length === 0}
        <p class="empty" data-testid="accounts-empty">
          No Claude account yet. An account appears here once a host is logged in to it.
        </p>
      {:else}
        <ul class="grid" aria-label="Claude accounts">
          {#each list as a (a.uuid)}
            {@const five = windowView(a, '5h')}
            {@const week = windowView(a, 'weekly')}
            {@const limit = limitOf(a.uuid, $attentionFacts, now)}
            {@const paused = pausedSessions(a, $attentionFacts, now)}
            {@const other = limit ? switchCandidate(a, paused, list, $attentionFacts, now) : null}
            <!-- The card's title is its keyboard way in; a click anywhere
                 else on it opens the detail too. -->
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
            <li
              class="ocard"
              class:limited={limit !== null}
              data-account={a.uuid}
              data-testid="account-card"
              onclick={(e) => onCardClick(e, a.uuid)}
            >
              <div class="card-head">
                <StatusDot state={cardState(a, five, week)} label={null} />
                <button type="button" class="title" data-testid="account-card-open" onclick={() => openDetail(a.uuid)}
                  >{a.label}</button
                >
                <span class="grow"></span>
                {#if a.plan || a.uuid === defaultUuid}
                  <span class="tag" data-testid="account-card-tag"
                    >{[a.plan, a.uuid === defaultUuid ? 'default' : null].filter(Boolean).join(' · ')}</span
                  >
                {/if}
              </div>
              {#each [five, week] as w (w.kind)}
                {@const atLimit = limit !== null && (limit.window === 'five_hour') === (w.kind === '5h')}
                <div class="oline" data-level={atLimit ? 'limit' : w.level} data-testid="account-card-{w.kind}">
                  <span class="w-title">{w.title}</span>
                  <span class="w-val">{cardWindowText(w, atLimit ? limit : null)}</span>
                </div>
                <Meter
                  value={atLimit ? 1 : (w.left ?? 0) / 100}
                  level={atLimit ? 'crit' : meterLevel(w.level)}
                  label="{a.label} {w.title} left"
                />
              {/each}
              <div class="ofoot">
                <span class="meta">
                  {a.sessions.length} {a.sessions.length === 1 ? 'session' : 'sessions'}
                  {#if paused.length > 0}
                    ·
                    <button
                      type="button"
                      class="link"
                      aria-expanded={pausedOpen.has(a.uuid)}
                      data-testid="account-paused-show"
                      onclick={() => togglePaused(a.uuid)}
                      >{paused.length} paused {paused.length === 1 ? 'session' : 'sessions'} → {pausedOpen.has(a.uuid)
                        ? 'Hide'
                        : 'Show'}</button
                    >
                  {/if}
                </span>
                {#if paused.length > 0}
                  <button
                    type="button"
                    class="btn"
                    data-testid="account-switch"
                    disabled={switching !== null || switchBlocked(paused) !== null}
                    title={switchBlocked(paused) ??
                      'Resume each paused session under the login on its host with the most room left'}
                    onclick={() => void switchPaused(a, paused)}
                    >{switching === a.uuid ? 'Switching…' : other ? `Switch to ${other.label}` : 'Switch account'}</button
                  >
                {/if}
              </div>
              {#if paused.length > 0 && pausedOpen.has(a.uuid)}
                <ul class="paused" data-testid="account-paused-list">
                  {#each paused as p (p.id)}
                    <li>
                      <button type="button" class="link" onclick={() => selectSessionExplicitly(p)}
                        >{displayName(p, $showFriendlyNames)}</button
                      >
                      <span class="sub">Paused · {limit?.window === 'five_hour' ? '5-hour' : 'weekly'} limit · {p.host_alias}</span>
                      <LimitActions sess={p} resetsAt={limit?.resets_at ?? null} {accountName} />
                    </li>
                  {/each}
                </ul>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      <h3 class="section-label">Hosts</h3>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="table-wrap" onkeydown={onTableKeydown}>
        <HostsTable
          hosts={tableHosts}
          {rowInfo}
          sessions={$sessions}
          accountByUuid={$accountByUuid}
          {newestClaude}
          selectedAlias={selectedHost}
          {now}
          idleSecs={$attentionIdleMinutes * 60}
          bind:tableEl
          onselect={(alias) => {
            selectedHost = alias;
            tableEl?.focus();
          }}
          onopen={(alias) => requestHostsView(alias)}
        />
      </div>
    </div>
  {:else}
    <div class="split">
      <ul class="list" role="listbox" aria-label="Claude accounts" tabindex="-1" onkeydown={onListKeydown}>
        {#each list as a (a.uuid)}
          {@const five = windowView(a, '5h')}
          {@const week = windowView(a, 'weekly')}
          <li
            class="card"
            class:active={a.uuid === selected?.uuid}
            role="option"
            aria-selected={a.uuid === selected?.uuid}
            tabindex={a.uuid === selected?.uuid ? 0 : -1}
            data-account={a.uuid}
            data-testid="account-card"
            onclick={() => (picked = a.uuid)}
            onkeydown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                picked = a.uuid;
              }
            }}
          >
            <div class="card-head">
              <span class="label">{a.label}</span>
              {#if a.plan}<span class="plan">{a.plan}</span>{/if}
            </div>
            {#each [five, week] as w (w.kind)}
              <div class="line" data-level={w.level}>
                <span class="w-title">{w.title}</span>
                <span class="w-val">
                  {#if w.unknown}? left{:else if w.left === null}—{:else}{w.left}% left{#if w.resetShort} · {w.resetShort}{/if}{/if}
                </span>
              </div>
            {/each}
            <div class="meta">
              {a.sessions.length} {a.sessions.length === 1 ? 'session' : 'sessions'}
              · {a.logins.length} {a.logins.length === 1 ? 'login' : 'logins'}
            </div>
          </li>
        {/each}
      </ul>

      {#if selected}
        {@const a = selected}
        <div class="detail" data-testid="account-detail">
          <div class="d-head">
            <div>
              <h3>{a.label}</h3>
              <div class="sub">
                {#if a.account.email && a.account.email !== a.label}{a.account.email} · {/if}
                {a.plan ?? 'plan unknown'}
                {#if a.account.organization_name} · {a.account.organization_name}{/if}
              </div>
            </div>
            <button
              class="btn"
              disabled={refreshing}
              onclick={() => refresh(a.uuid)}
              data-testid="account-refresh">Refresh</button
            >
          </div>
          {#if a.usage?.fetched_at}
            <div class="sub" data-testid="account-checked">
              {checkedAgo(a.usage.fetched_at, now)}{#if a.usage.source_host} via {a.usage.source_host}{/if}
            </div>
          {/if}

          {#each ['5h', 'weekly'] as const as kind (kind)}
            {@const w = windowView(a, kind)}
            {@const pts = historyFor === a.uuid ? historyPoints(history, kind, now) : []}
            {@const path = sparkPath(pts, kind, now, SPARK_W, SPARK_H)}
            {@const peak = peakUsed(pts)}
            <div class="window" data-testid="account-window-{kind}" data-level={w.level}>
              <div class="w-row">
                <span class="w-title">{w.title}</span>
                <span class="w-val">
                  {#if w.unknown}? left{:else if w.left === null}no reading yet{:else}{w.left}% left{/if}
                  {#if w.badge}<span class="badge">{w.badge}</span>{/if}
                </span>
              </div>
              {#if w.left !== null}
                <div
                  class="meter"
                  role="meter"
                  aria-label="{w.title} left"
                  aria-valuemin="0"
                  aria-valuemax="100"
                  aria-valuenow={w.left}
                >
                  <div class="fill" style="width: {w.left}%"></div>
                </div>
                <div class="sub" data-testid="account-reset-{kind}">{w.reset}</div>
              {/if}
              <div class="hist">
                {#if path}
                  <svg
                    width={SPARK_W}
                    height={SPARK_H}
                    viewBox="0 0 {SPARK_W} {SPARK_H}"
                    role="img"
                    aria-label="{w.title} use over the last {kind === '5h' ? 'day' : 'week'}"
                    data-testid="account-history-{kind}"
                  >
                    <path d={path} />
                  </svg>
                  <span class="sub">
                    last {kind === '5h' ? 'day' : 'week'}{#if peak !== null} · peaked at {peak}% used{/if}
                  </span>
                {:else}
                  <span class="sub" data-testid="account-history-empty-{kind}">No history yet</span>
                {/if}
              </div>
            </div>
          {/each}

          <h4>Hosts and profiles</h4>
          {#if a.logins.length === 0}
            <p class="sub">No host is logged in to this account right now.</p>
          {:else}
            <ul class="rows" data-testid="account-logins">
              {#each a.logins as l (l.host + '/' + (l.profile ?? ''))}
                <li>
                  <button
                    type="button"
                    class="link mono"
                    title="Open {l.host} in Hosts"
                    data-testid="account-login-host"
                    onclick={() => requestHostsView(l.host)}>{l.host}</button
                  >
                  <span class="sub">{l.profile ? `profile ${l.profile}` : 'host login'}</span>
                </li>
              {/each}
            </ul>
          {/if}

          <h4>Sessions on it</h4>
          {#if a.sessions.length === 0}
            <p class="sub">No session runs on this account.</p>
          {:else}
            <ul class="rows" data-testid="account-sessions">
              {#each a.sessions as s (s.id)}
                <li>
                  <button class="link" onclick={() => selectSessionExplicitly(s)}>
                    {displayName(s, $showFriendlyNames)}
                  </button>
                  <span class="sub">{s.host_alias} · {s.claude_status ?? s.status}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {:else if pickedMissing}
        <div class="detail" data-testid="account-missing">
          <p class="empty">That account is not in this fleet’s list. Pick one on the left.</p>
        </div>
      {/if}
    </div>
  {/if}
</section>

{#if showAddHost}
  <!-- The add-host wizard (4.9), as the Hosts view opens it. -->
  <AddHostWizard onClose={() => (showAddHost = false)} onNewSession={requestNewSessionOnHost} />
{/if}

<style>
  .accounts {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg);
    color: var(--fg);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4);
    border-bottom: 1px solid var(--border);
  }
  .grow {
    flex: 1;
  }
  .overview {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding: var(--space-3) var(--space-4) var(--space-4);
  }
  .section-label {
    margin: var(--space-2) 0;
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--fg-muted);
  }
  .grid {
    list-style: none;
    margin: 0 0 var(--space-4);
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: var(--space-3);
  }
  .ocard {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    cursor: pointer;
    min-width: 0;
  }
  .ocard:hover {
    border-color: var(--accent);
  }
  .ocard.limited {
    border-color: color-mix(in srgb, var(--usage-crit) 55%, transparent);
  }
  .title {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    font-weight: 600;
    color: var(--fg);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: var(--ring-offset);
  }
  .tag {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    padding: 0 var(--space-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    white-space: nowrap;
  }
  .oline {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    font-size: var(--text-xs);
    margin-top: var(--space-1);
  }
  .ofoot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    margin-top: var(--space-2);
    flex-wrap: wrap;
  }
  .paused {
    list-style: none;
    margin: var(--space-1) 0 0;
    padding: var(--space-2) 0 0;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .paused li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-items: flex-start;
  }
  .table-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  h2 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
  }
  h3 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  h4 {
    margin: var(--space-4) 0 var(--space-2);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .sub {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .empty {
    padding: var(--space-4);
    color: var(--fg-muted);
  }
  .split {
    display: grid;
    grid-template-columns: minmax(220px, 300px) 1fr;
    min-height: 0;
    flex: 1 1 auto;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: var(--space-2);
    overflow: auto;
    border-right: 1px solid var(--border);
  }
  .card {
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    margin-bottom: var(--space-2);
    background: var(--bg-pane);
    cursor: pointer;
  }
  .card.active {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .card:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: var(--ring-offset);
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    margin-bottom: var(--space-1);
  }
  .label {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .plan {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .line,
  .w-row {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    font-size: var(--text-xs);
  }
  .w-title {
    color: var(--fg-muted);
  }
  [data-level='low'] .w-val,
  [data-level='limit'] .w-val {
    color: var(--usage-crit);
  }
  [data-level='caution'] .w-val {
    color: var(--usage-warn);
  }
  .meta {
    margin-top: var(--space-1);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .detail {
    padding: var(--space-3) var(--space-4);
    overflow: auto;
  }
  .d-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-3);
  }
  .btn {
    height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font: inherit;
    font-size: var(--control-font-sm);
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    background: var(--control-bg-hover);
  }
  .window {
    margin-top: var(--space-3);
    max-width: 420px;
  }
  .w-row {
    font-size: var(--text-sm);
  }
  .badge {
    margin-left: var(--space-1);
    font-size: var(--text-2xs);
  }
  .meter {
    height: 6px;
    margin: var(--space-1) 0;
    border-radius: var(--radius-pill);
    background: var(--control-bg);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--usage-ok);
  }
  [data-level='caution'] .fill {
    background: var(--usage-warn);
  }
  [data-level='low'] .fill,
  [data-level='limit'] .fill {
    background: var(--usage-crit);
  }
  .hist {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-1);
  }
  .hist svg {
    flex: none;
    border-bottom: 1px solid var(--border);
  }
  .hist path {
    fill: none;
    stroke: var(--accent);
    stroke-width: 1.5;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .rows li {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: 2px 0;
  }
  .mono {
    font-family: var(--mono);
    font-size: var(--text-sm);
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
</style>
