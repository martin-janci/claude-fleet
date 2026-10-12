<script lang="ts">
  // The Accounts page (Orbit Fleet redesign step 4.1, the Accounts board):
  // a list of every Claude account on the left, the picked one's detail on
  // the right — plan, the 5-hour and weekly windows with their reset times
  // and history, the hosts and login profiles signed in to it, and the
  // sessions running on it. Usage comes from the same snapshots the Hosts
  // view shows (`account_usage_store`); history from `account_usage_history`.
  // Each card also says what runs on the account and what it cost today
  // (`account_spend`, the `usage_daily_account` roll-up of step 4.2; local
  // only, so a paired desktop shows no $), and how many sessions its limit
  // paused, with Show and a one-click Switch to the account with headroom.
  import { accounts } from './accounts';
  import { hosts } from './hosts';
  import { sessions } from './sessions';
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
    historyPoints,
    HISTORY_SPAN_SECS,
    loadUsageHistory,
    peakUsed,
    sparkPath,
    countLine,
    loadAccountSpend,
    pausedSessions,
    routinesOn,
    routinesRunningAs,
    refreshedLine,
    fallbackRoutinesOn,
    spendByAccount,
    type AccountSummary,
    type UsageSnapshotRow,
  } from './accounts_page';
  import { formatCostMicros, restartSession, type SessionRow } from './sessions';
  import { listRoutines, type RoutineRow } from './routines';
  import { attentionFacts } from './attention_facts';
  import { accountByUuid, accountLabel as labelOf } from './accounts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import { moveToHeadroom, resetText, switchTarget, waitOut } from './account_limits';
  import LimitActions from './LimitActions.svelte';
  import { push } from './toasts';
  import { displayName } from './attention';
  import { showFriendlyNames } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { pushError } from './toasts';
  import { accountsPageRequest, accountsPausedRequest } from './account_pill';
  import { requestHostsView } from './app_views';
  import AddAccountDialog from './AddAccountDialog.svelte';
  import { untrack } from 'svelte';
  import { get } from 'svelte/store';

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

  // Today's spend per account. Re-read with the usage snapshots (a usage
  // pass books spend beside them) and every five minutes; a paired desktop
  // has no roll-up of its own, so it shows none rather than asking.
  let spend = $state<Map<string, number> | null>(null);
  let spendTick = $state(0);
  $effect(() => {
    const t = setInterval(() => (spendTick += 1), 5 * 60_000);
    return () => clearInterval(t);
  });
  $effect(() => {
    void $accountUsage;
    void spendTick;
    if ($hubStatus.remote) {
      spend = null;
      return;
    }
    let live = true;
    void loadAccountSpend(untrack(() => now)).then((r) => {
      if (live) spend = r.ok ? spendByAccount(r.value) : null;
    });
    return () => {
      live = false;
    };
  });
  const spendText = (uuid: string): string | null => (spend ? formatCostMicros(spend.get(uuid) ?? 0) : null);

  // Routines run as a login; the card counts the switched-on ones per account.
  let routines = $state<RoutineRow[]>([]);
  $effect(() => {
    let live = true;
    void listRoutines().then((r) => {
      if (live && r.ok && Array.isArray(r.value)) routines = r.value;
    });
    return () => {
      live = false;
    };
  });

  const pausedOf = (a: AccountSummary): SessionRow[] =>
    pausedSessions(a, $attentionFacts?.limited_accounts, now);

  // "Show": the detail's Sessions list narrows to the paused ones.
  let pausedOnly = $state<string | null>(null);
  /** + Add account's dialog is open (M15 G2.9). */
  let adding = $state(false);
  function showPaused(uuid: string) {
    picked = uuid;
    pausedOnly = uuid;
  }

  // "Switch to <account>": the login with the most headroom on the first
  // paused session's host names the target before anyone presses; the press
  // resumes each paused session this person may restart under the login
  // with the most headroom on its own host (step 4.4's bulk move).
  let switchTo = $state<Record<string, string | null>>({});
  let switching = $state<string | null>(null);
  const restartBlocked = $derived(hubActionBlocked('restart_session', $hubStatus, $hubConnection));
  $effect(() => {
    for (const a of list) {
      const first = pausedOf(a)[0];
      if (!first || a.uuid in untrack(() => switchTo)) continue;
      switchTo = { ...untrack(() => switchTo), [a.uuid]: null };
      void switchTarget(first).then((t) => {
        switchTo = { ...switchTo, [a.uuid]: t ? t.account_uuid : null };
      });
    }
  });
  async function switchPaused(a: AccountSummary) {
    const rows = bulkTargets(pausedOf(a), 'restart_session', $sessionBlocked);
    if (rows.length === 0) {
      push({ kind: 'info', message: 'None of the paused sessions is yours to restart' });
      return;
    }
    switching = a.uuid;
    const r = await moveToHeadroom(rows, restartSession);
    switching = null;
    const { [a.uuid]: _gone, ...rest } = switchTo;
    void _gone;
    switchTo = rest;
    const parts = [
      r.moved > 0 ? `Switched ${r.moved} session${r.moved === 1 ? '' : 's'}` : 'Nothing switched',
      r.nowhere > 0 ? `${r.nowhere} with no other login that has room` : '',
      r.failed > 0 ? `${r.failed} failed` : '',
    ].filter(Boolean);
    push({ message: parts.join(' · '), kind: r.failed > 0 ? 'error' : r.moved > 0 ? 'success' : 'info' });
  }
  let picked = $state<string | null>(null);
  // A pill elsewhere (step 4.3) asked for one account: show it, then clear
  // the request so a later visit keeps whatever was picked by hand.
  $effect(() => {
    const req = $accountsPageRequest;
    if (req === null) return;
    picked = req;
    // The limit-hit toast's "Show paused sessions" asks for the paused ones.
    pausedOnly = get(accountsPausedRequest) ? req : null;
    accountsPausedRequest.set(false);
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

  // M15 G7.12: the header's page-wide Refresh reads every account's usage
  // again; one failure is said once, the others still refresh.
  const refreshed = $derived(refreshedLine(list.map((a) => a.usage?.fetched_at), now));
  async function refreshAll() {
    refreshing = true;
    const results = await Promise.all(list.map((a) => refreshAccountUsage(a.uuid)));
    refreshing = false;
    const failed = results.find((r) => !r.ok);
    if (failed && !failed.ok) pushError(failed.error, 'Some usage was not refreshed');
  }

  const limitOf = (uuid: string): number | null => $attentionFacts?.limited_accounts?.[uuid]?.resets_at ?? null;

  /** "Wait until <reset>" on the paused panel: every paused session of the
   *  account waits out its limit, as each row's own Wait does. */
  function waitAll(a: AccountSummary) {
    const at = limitOf(a.uuid);
    if (at == null) return;
    for (const s of pausedOf(a)) waitOut(s.id, at);
    push({ kind: 'info', message: `Waiting until ${resetText(at)}; the paused sessions resume then.` });
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

<section class="accounts" data-testid="accounts-page" aria-label="Accounts">
  <header class="head">
    <h2>Accounts</h2>
    <span class="sub" data-testid="accounts-count">
      {list.length} {list.length === 1 ? 'account' : 'accounts'}{#if refreshed} · {refreshed}{/if}
    </span>
    {#if list.length > 0}
      <button type="button" class="btn-quiet" data-testid="accounts-refresh-all" disabled={refreshing} onclick={() => void refreshAll()}
        >{refreshing ? 'Refreshing…' : 'Refresh'}</button
      >
    {/if}
    <!-- Review r08: the rail item is "Accounts & hosts", and Classic's Hosts
         tab was always in view; the Hosts view is one click from here. -->
    <button type="button" class="btn-quiet hosts-link" data-testid="accounts-all-hosts" onclick={() => requestHostsView()}
      >All hosts ›</button
    >
    <!-- M15 step G2.9: a subscription login or an API key, as a new login
         profile on a host. -->
    <button type="button" class="btn" data-testid="accounts-add" onclick={() => (adding = true)}>+ Add account…</button>
  </header>

  {#if adding}
    <AddAccountDialog onclose={() => (adding = false)} />
  {/if}

  {#if list.length === 0}
    <p class="empty" data-testid="accounts-empty">
      No Claude account yet. Add one, or log a host in to it.
    </p>
  {:else}
    <div class="split">
      <ul class="list" role="listbox" aria-label="Claude accounts" tabindex="-1" onkeydown={onListKeydown}>
        {#each list as a (a.uuid)}
          {@const five = windowView(a, '5h')}
          {@const week = windowView(a, 'weekly')}
          {@const paused = pausedOf(a)}
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
            <div class="meta" data-testid="account-counts">
              {countLine(
                a.sessions.length,
                routinesOn(a.uuid, routines, $hosts),
                spendText(a.uuid),
                fallbackRoutinesOn(a.uuid, routines, $hosts),
              )}
            </div>
            {#if paused.length > 0}
              {@const target = switchTo[a.uuid]}
              <div class="paused" data-testid="account-paused">
                <button
                  type="button"
                  class="link"
                  data-testid="account-paused-show"
                  onclick={(e) => {
                    e.stopPropagation();
                    showPaused(a.uuid);
                  }}>{paused.length} paused {paused.length === 1 ? 'session' : 'sessions'} → Show</button
                >
                {#if limitOf(a.uuid) != null}
                  <button
                    type="button"
                    class="btn-quiet"
                    data-testid="account-paused-wait"
                    onclick={(e) => {
                      e.stopPropagation();
                      waitAll(a);
                    }}>Wait until {resetText(limitOf(a.uuid) ?? 0)}</button
                  >
                {/if}
                {#if target}
                  <button
                    type="button"
                    class="btn"
                    data-testid="account-paused-switch"
                    disabled={switching !== null || restartBlocked !== null}
                    title={restartBlocked ?? 'Resume each paused session under the login with the most headroom on its host'}
                    onclick={(e) => {
                      e.stopPropagation();
                      void switchPaused(a);
                    }}
                    >{switching === a.uuid ? 'Switching…' : `Switch to ${$accountByUuid.get(target) ? labelOf($accountByUuid.get(target)) : target.slice(0, 8)}`}</button
                  >
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>

      {#if selected}
        {@const a = selected}
        {@const onlyPaused = pausedOnly === a.uuid}
        {@const rows = onlyPaused ? pausedOf(a) : a.sessions}
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
          {#if spend && (spend.get(a.uuid) ?? 0) > 0}
            <div class="sub" data-testid="account-spend">{spendText(a.uuid)} today</div>
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

          <h4>
            {onlyPaused ? 'Paused by its limit' : 'Sessions on it'}
            {#if onlyPaused}<button type="button" class="link" data-testid="account-paused-all" onclick={() => (pausedOnly = null)}>Show all</button>{/if}
          </h4>
          {#if rows.length === 0}
            <p class="sub">No session runs on this account.</p>
          {:else}
            <ul class="rows" data-testid="account-sessions">
              {#each rows as s (s.id)}
                <li>
                  <button class="link" onclick={() => selectSessionExplicitly(s)}>
                    {displayName(s, $showFriendlyNames)}
                  </button>
                  <span class="sub">{s.host_alias} · {onlyPaused ? 'Paused · limit' : (s.claude_status ?? s.status)}</span>
                  {#if onlyPaused}
                    <LimitActions sess={s} resetsAt={limitOf(a.uuid)} accountName={(u) => ($accountByUuid.get(u) ? labelOf($accountByUuid.get(u)) : u.slice(0, 8))} />
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
          {#if onlyPaused}
            {@const held = routinesRunningAs(a.uuid, routines, $hosts)}
            {#if held.length > 0}
              <ul class="rows" data-testid="account-paused-routines">
                {#each held as r (r.id)}
                  <li>
                    <span>{r.name}</span>
                    <span class="sub"
                      >Routine · its runs meet the limit{#if limitOf(a.uuid) != null} until {resetText(limitOf(a.uuid) ?? 0)}, then run again on their own{/if}</span
                    >
                  </li>
                {/each}
              </ul>
            {/if}
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
  .hosts-link {
    margin-left: auto;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    padding: 0 var(--space-2);
    font: inherit;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .hosts-link:hover {
    border-color: var(--accent);
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
  .paused {
    margin-top: var(--space-1);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
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
