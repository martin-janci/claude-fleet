<script lang="ts">
  // The Today view (work graph M9.1): what waits on you, grouped by work;
  // what is in progress; what shipped today; what went stale. It is the
  // empty state of Details and opens over a selected session with ⌘⇧T.
  // Everything comes from the hub's `work_today` digest (rows, links and the
  // tracker cache — no network), cut to the scope the sidebar shows. Copy
  // standup puts the same four sections on the clipboard as plain text.
  // Tracker titles are rendered as text, never as markup.
  import { onDestroy } from 'svelte';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { effectiveScope, scopeOf } from './orgs';
  import { copyText } from './clipboard';
  import { openExternal } from './open_external';
  import { timeAgo } from './session_status';
  import { candidatesFor, inScope, refreshTidy, requestTidy, tidyReport } from './tidy';
  import {
    loadToday,
    localMidnight,
    scopeToday,
    standupText,
    isEmptyView,
    groupLabel,
    sessionPhrase,
    type Today,
    type TodayBucket,
    type TodayGroup,
    type TodaySession,
  } from './today';

  let {
    onclose,
    now = () => Date.now(),
  }: {
    /** Present when the view covers a selected session (⌘⇧T). */
    onclose?: () => void;
    /** Milliseconds; injectable for tests. */
    now?: () => number;
  } = $props();

  let today = $state<Today | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;

  // A hub without `work_today` (older than M9.1) answers one of these for the
  // call — by policy (E_FORBIDDEN: the gates fail closed on the tool name),
  // at the router (E_HUB_PROTOCOL) or as an unknown action (E_INVALID).
  // That is "no Today on this hub", not a failure: the view is the empty
  // state of Details and keeps its plain text, and the refresh on row events
  // stops instead of asking again after every turn. Like tidy.ts, matched by
  // code, never by message text.
  const HUB_HAS_NO_TODAY = ['E_INVALID', 'E_FORBIDDEN', 'E_HUB_PROTOCOL'];

  async function refresh() {
    loading = true;
    const r = await loadToday(localMidnight(now()));
    loading = false;
    if (r.ok) {
      today = r.value;
      error = null;
    } else if (HUB_HAS_NO_TODAY.includes(r.error.code)) {
      today = null;
      error = null;
      stopFollowing();
    } else {
      error = r.error.message;
    }
  }

  // Refresh on open, then on row changes — debounced, since a busy fleet
  // emits a row event per turn.
  let first = true;
  let unsub: (() => void) | null = sessions.subscribe(() => {
    if (first) {
      first = false;
      void refresh();
      return;
    }
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => void refresh(), 2_000);
  });
  function stopFollowing() {
    unsub?.();
    unsub = null;
    clearTimeout(refreshTimer);
  }
  onDestroy(() => {
    stopFollowing();
    clearTimeout(copyTimer);
  });

  const view = $derived(today ? scopeToday(today, $effectiveScope, $sessions, $scopeOf) : null);

  // Work graph M9 follow-up: the stale sessions fleet also suggests tidying
  // (M7) open the Tidy-up sheet with just those picked.
  void refreshTidy();
  const staleIds = $derived(view ? view.stale.flatMap((g) => g.sessions.map((s) => s.id)) : []);
  const staleTidy = $derived(
    candidatesFor(inScope($tidyReport.candidates, $sessions, $effectiveScope, $scopeOf), staleIds),
  );
  function tidyStale() {
    requestTidy(staleTidy.map((c) => c.session_id));
    onclose?.();
  }

  async function copyStandup() {
    if (!view) return;
    if (!(await copyText(standupText(view)))) return;
    copied = true;
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied = false), 1_500);
  }

  function jump(s: TodaySession) {
    const row: SessionRow | undefined = $sessions.find((r) => r.id === s.id);
    if (!row) return;
    selectSessionExplicitly(row);
    onclose?.();
  }
</script>

{#snippet groups(list: TodayGroup[], bucket: TodayBucket)}
  <ul class="groups">
    {#each list as g (`${g.key ?? ''}|${bucket}`)}
      <li class="group" data-testid="today-group">
        <div class="head">
          <span class="label" class:nowork={!g.key}>{groupLabel(g)}</span>
          {#if g.status_name}<span class="status">{g.status_name}</span>{/if}
          {#if g.url}
            <button class="btn btn--quiet link" type="button" onclick={() => void openExternal(g.url ?? '')}
              >Open ticket</button
            >
          {/if}
        </div>
        <ul class="sessions">
          {#each g.sessions as s (s.id)}
            <li>
              <button
                class="btn btn--quiet session"
                type="button"
                data-testid="today-session"
                title={`${s.host_alias} · active ${timeAgo(s.last_activity_at, now())}`}
                onclick={() => jump(s)}>{sessionPhrase(s, bucket)}</button
              >
              <span class="meta">{s.host_alias}{#if s.ci_status} · CI {s.ci_status}{/if}</span>
            </li>
          {/each}
        </ul>
      </li>
    {/each}
  </ul>
{/snippet}

<section class="today" data-testid="today-view" aria-label="Today">
  <header>
    <h2>Today</h2>
    <div class="actions">
      <button class="btn" type="button" data-testid="today-copy" disabled={!view} onclick={() => void copyStandup()}
        >{copied ? 'Copied' : 'Copy standup'}</button
      >
      <button class="btn btn--quiet" type="button" data-testid="today-refresh" disabled={loading} onclick={() => void refresh()}
        >Refresh</button
      >
      {#if onclose}
        <button class="btn btn--quiet" type="button" data-testid="today-close" aria-label="Close Today" onclick={onclose}
          >✕</button
        >
      {/if}
    </div>
  </header>

  {#if error}
    <p class="error" role="alert" data-testid="today-error">{error}</p>
  {/if}

  {#if view}
    {#if isEmptyView(view)}
      <p class="empty" data-testid="details-empty">Nothing running today. Pick a session, or start work from ⌘K.</p>
    {/if}
    {#if view.waiting.length > 0}
      <h3 data-testid="today-waiting">Waiting on you</h3>
      {@render groups(view.waiting, 'waiting')}
    {/if}
    {#if view.inProgress.length > 0}
      <h3 data-testid="today-in-progress">In progress</h3>
      {@render groups(view.inProgress, 'in_progress')}
    {/if}
    {#if view.shipped.length > 0}
      <h3 data-testid="today-shipped">Shipped today</h3>
      <ul class="groups">
        {#each view.shipped as x (`${x.how}|${x.key ?? x.pr_url ?? x.at}`)}
          <li class="group shipped">
            <span class="label">{x.key ? groupLabel({ key: x.key, title: x.title ?? '' }) : x.title || 'Untitled work'}</span>
            <span class="status">{x.how === 'done' ? 'done' : 'PR'}</span>
            {#if x.pr_url}
              <button class="btn btn--quiet link" type="button" onclick={() => void openExternal(x.pr_url ?? '')}>PR</button>
            {:else if x.url}
              <button class="btn btn--quiet link" type="button" onclick={() => void openExternal(x.url ?? '')}
                >Open ticket</button
              >
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if view.stale.length > 0}
      <h3 data-testid="today-stale">
        Stale
        {#if staleTidy.length > 0}
          <button
            class="btn btn--quiet tidy"
            type="button"
            data-testid="today-tidy"
            title="Open Tidy up with these sessions picked; nothing happens until you confirm there"
            onclick={tidyStale}>Tidy up · {staleTidy.length}</button
          >
        {/if}
      </h3>
      {@render groups(view.stale, 'stale')}
    {/if}
  {:else if !error}
    <p class="empty" data-testid="details-empty">Pick a session to see details.</p>
  {/if}
</section>

<style>
  .today {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    font-size: 0.9rem;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  h3 {
    margin: 0.6rem 0 0.2rem;
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  h3 .tidy {
    margin-left: 0.4rem;
    text-transform: none;
    letter-spacing: normal;
  }
  .actions {
    display: flex;
    gap: 0.3rem;
  }
  .groups,
  .sessions {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .group {
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--border, transparent);
  }
  .head,
  .shipped {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .label {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .label.nowork {
    font-weight: normal;
    color: var(--fg-muted);
  }
  .status,
  .meta {
    color: var(--fg-muted);
    font-size: 0.8rem;
  }
  .sessions {
    padding-left: 0.6rem;
  }
  .session {
    text-align: left;
  }
  .empty {
    color: var(--fg-muted);
  }
  .error {
    color: var(--danger, #c33);
  }
</style>
