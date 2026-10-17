<script lang="ts">
  // The Today view (work graph M9.1): what waits on you, grouped by work;
  // what is in progress; what shipped today; what went stale. It is the
  // empty state of Details and opens over a selected session with ⌘⇧T.
  // Everything comes from the hub's `work_today` digest (rows, links and the
  // tracker cache — no network), cut to the scope the sidebar shows. Copy
  // standup puts the same four sections on the clipboard as plain text.
  // Tracker titles are rendered as text, never as markup.
  //
  // Gap plan G3.2 (board Today): four KPI tiles, the date and fleet line,
  // a paused-limit row's Switch account and Wait right on it with an Open
  // per row, "from <routine>" on what shipped, and In progress cut to four
  // groups with "N more ›". Cut: "Release 0.5.3 · apps installed" (fleet
  // records no install per device).
  import { onDestroy } from 'svelte';
  import LimitActions from './LimitActions.svelte';
  import { classify } from './attention';
  import { attentionFacts } from './attention_facts';
  import { accountByUuid, accountLabel } from './accounts';
  import { hosts } from './hosts';
  import { attentionIdleMinutes } from './notify';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { effectiveScope, scopeOf } from './orgs';
  import { copyText } from './clipboard';
  import { openExternal } from './open_external';
  import { timeAgo } from './session_status';
  import { statusDotClass } from './trackers';
  import { candidatesFor, inScope, refreshTidy, requestTidy, tidyReport } from './tidy';
  import MorningBrief from './MorningBrief.svelte';
  import MissionNudge from './MissionNudge.svelte';
  import { openMission } from './missions';
  import { waitWords } from './mission_waits';
  import Skeleton from './states/Skeleton.svelte';
  import { errorText } from './error_copy';
  import {
    loadToday,
    localMidnight,
    scopeToday,
    standupText,
    isEmptyView,
    groupLabel,
    groupStatusLabel,
    sessionPhrase,
    todayKpis,
    todayLine,
    shippedWhen,
    IN_PROGRESS_SHOWN,
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
  /** The hub has no `work_today`: the plain empty state, for good. */
  let unsupported = $state(false);
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

  // Only the newest refresh lands: an older, slower answer would put
  // stale counts back over the newer ones (review r07).
  let refreshSeq = 0;
  async function refresh() {
    const mine = ++refreshSeq;
    loading = true;
    const r = await loadToday(localMidnight(now()));
    if (mine !== refreshSeq) return;
    loading = false;
    if (r.ok) {
      today = r.value;
      error = null;
    } else if (HUB_HAS_NO_TODAY.includes(r.error.code)) {
      today = null;
      error = null;
      unsupported = true;
      stopFollowing();
    } else {
      error = errorText(r.error);
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

  const kpis = $derived(view ? todayKpis(view) : null);
  const dateLine = $derived(
    todayLine(
      now(),
      $hosts.filter((h) => !h.hidden).map((h) => h.alias),
      $sessions,
      localMidnight(now()),
    ),
  );
  let allInProgress = $state(false);
  const inProgressShown = $derived(view ? (allInProgress ? view.inProgress : view.inProgress.slice(0, IN_PROGRESS_SHOWN)) : []);

  /** The live row behind a digest session, when it is paused on a limit:
   *  its Switch account and Wait go right on the row. */
  function limited(s: TodaySession): SessionRow | null {
    const row = $sessions.find((r) => r.id === s.id);
    if (!row) return null;
    const opts = { idleSecs: $attentionIdleMinutes * 60, now: Math.floor(now() / 1000), facts: $attentionFacts };
    return classify(row, opts) === 'account_limit' ? row : null;
  }

  /** The row's dot: a failed turn reads red, a wait amber, live work blue,
   *  stale grey (the status colours the session list draws). */
  function sessionTone(s: TodaySession, bucket: TodayBucket): string {
    if (bucket === 'waiting') return s.attention === 'failed' ? 'failed' : 'waiting';
    if (bucket === 'stale') return 'idle';
    return 'working';
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
          <div class="what">
          <span class="label" class:nowork={!g.key}>{groupLabel(g)}</span>
          <!-- The status the group is in: a tracker's own name, or the
               effective status of work fleet tracks itself (native item
               status — `status_category` here is already the live-lifted
               answer, so a local item's "in progress" shows too). The dot
               is the same vocabulary the sidebar's work chip draws. -->
          {#if groupStatusLabel(g)}
            <span class="status" data-testid="today-group-status">
              <span class="dot {statusDotClass(g.status_category)}" aria-hidden="true"></span
              >{groupStatusLabel(g)}</span
            >
          {/if}
          </div>
          {#if g.url}
            <button class="btn btn--quiet link" type="button" onclick={() => void openExternal(g.url ?? '')}
              >Open ticket</button
            >
          {/if}
        </div>
        <ul class="sessions">
          {#each g.sessions as s (s.id)}
            <li class="srow">
              <span class="sdot tone-{sessionTone(s, bucket)}" aria-hidden="true"></span>
              <div class="smain">
                <button
                  class="btn btn--quiet session"
                  type="button"
                  data-testid="today-session"
                  title={`${s.host_alias} · active ${timeAgo(s.last_activity_at, now())}`}
                  onclick={() => jump(s)}>{sessionPhrase(s, bucket)}</button
                >
                <span class="meta"
                  >{s.host_alias}{#if s.ci_status}{` · CI ${s.ci_status}`}{/if}{` · ${timeAgo(s.last_activity_at, now())}`}</span
                >
              </div>
              <button
                class="btn btn--quiet is-bounded open"
                type="button"
                data-testid="today-session-open"
                aria-label="Open {s.name}"
                onclick={() => jump(s)}>Open</button
              >
              {#if bucket === 'waiting'}
                {@const row = limited(s)}
                {#if row}
                  <div class="limit" data-testid="today-limit">
                    <LimitActions
                      sess={row}
                      resetsAt={$attentionFacts?.limited_accounts?.[row.account_uuid ?? '']?.resets_at ?? null}
                      accountName={(u) => accountLabel($accountByUuid.get(u))}
                    />
                  </div>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>
      </li>
    {/each}
  </ul>
{/snippet}

<section class="today" data-testid="today-view" aria-label="Today">
  <header>
    <div class="title">
      <h2>Today</h2>
      <p class="dateline" data-testid="today-dateline">{dateLine}</p>
    </div>
    <div class="actions">
      <button
        class="btn btn--quiet is-bounded"
        type="button"
        data-testid="today-copy"
        disabled={!view}
        onclick={() => void copyStandup()}>{copied ? 'Copied' : 'Copy standup'}</button
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
    <p class="error" role="alert" data-testid="today-error">Couldn't load Today: {error}. Refresh tries again.</p>
  {/if}

  {#if view && kpis}
    <!-- The tiles read label over number (the Today board); the DOM keeps
         number first so the text a screen reader and the copy see is
         "4 Needs you". -->
    <ul class="kpis" aria-label="Today in numbers" data-testid="today-kpis">
      <li class="kpi waiting" class:hot={kpis.needsYou > 0}><span class="n">{kpis.needsYou}</span>{' '}<span class="k">Needs you</span></li>
      <li class="kpi working" class:hot={kpis.inProgress > 0}><span class="n">{kpis.inProgress}</span>{' '}<span class="k">In progress</span></li>
      <li class="kpi done" class:hot={kpis.shipped > 0}><span class="n">{kpis.shipped}</span>{' '}<span class="k">Shipped today</span></li>
      <li class="kpi"><span class="n">{kpis.stale}</span>{' '}<span class="k">Stale</span></li>
    </ul>
  {/if}

  <!-- Redesign 9.10: stuck missions, with the next step Jev proposes. -->
  <MissionNudge />

  {#if view}
    {#if isEmptyView(view)}
      <p class="empty" data-testid="details-empty">Nothing running today. Pick a session, or start work from ⌘K.</p>
    {/if}
    {#if view.waiting.length > 0 || (view.missions?.length ?? 0) > 0}
      <section class="block">
        <h3 class="waiting" data-testid="today-waiting">
          Waiting on you <span class="count">{kpis?.needsYou}</span>
        </h3>
        {#if view.waiting.length > 0}{@render groups(view.waiting, 'waiting')}{/if}
        {#if view.missions?.length}
          <!-- G1.6: missions waiting on a person, beside the sessions. -->
          <ul class="groups" data-testid="today-missions">
            {#each view.missions as m (m.id)}
              <li class="group mission">
                <span class="sdot tone-waiting" aria-hidden="true"></span>
                <div class="smain">
                  <button class="btn btn--quiet session" type="button" data-testid="today-mission-open" onclick={() => openMission(m.id)}
                    >{m.name}</button
                  >
                  <span class="meta waits">{waitWords(m.waiting_on)}</span>
                </div>
                <span class="tag">Mission</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}
    {#if view.inProgress.length > 0}
      <section class="block">
        <h3 data-testid="today-in-progress">In progress <span class="count">{kpis?.inProgress}</span></h3>
        {@render groups(inProgressShown, 'in_progress')}
        {#if view.inProgress.length > inProgressShown.length}
          <button class="btn btn--quiet more" type="button" data-testid="today-more" onclick={() => (allInProgress = true)}
            >{view.inProgress.length - inProgressShown.length} more ›</button
          >
        {/if}
      </section>
    {/if}
    {#if view.shipped.length > 0}
      <section class="block">
        <h3 class="shipped-h" data-testid="today-shipped">Shipped today <span class="count">{view.shipped.length}</span></h3>
        <ul class="groups">
          {#each view.shipped as x (`${x.how}|${x.key ?? x.pr_url ?? x.at}`)}
            <li class="group shipped">
              <span class="glyph" aria-hidden="true">{x.how === 'pr' ? '⑂' : '✓'}</span>
              <div class="smain">
                <span class="label">{x.key ? groupLabel({ key: x.key, title: x.title ?? '' }) : x.title || 'Untitled work'}</span>
                <span class="meta"
                  ><span data-testid="today-shipped-when">{shippedWhen(x)}</span
                  >{#if x.from}{' · '}<span data-testid="today-shipped-from">from {x.from}</span>{/if}</span
                >
              </div>
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
      </section>
    {/if}
    {#if view.stale.length > 0}
      <section class="block">
        <h3 data-testid="today-stale">
          Stale <span class="count">{kpis?.stale}</span>
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
      </section>
    {/if}
  {:else if !error && !unsupported && today === null}
    <!-- Review r13 (step 10.6): the first read is loading, not empty. -->
    <Skeleton rows={4} label="Loading Today" />
  {:else if !error}
    <p class="empty" data-testid="details-empty">Pick a session to see details.</p>
  {/if}

  <!-- Redesign 9.11: the brief drafted last; a new one only on Refresh.
       Last, as on the Today board: what needs a hand comes first. -->
  <div class="brief-card"><MorningBrief /></div>
</section>

<style>
  /* The view fills a bare scroll body (ControlViewsPanel's .body has no
     padding), so it brings its own gutter. Spacing follows the Orbit Fleet
     Today board: sections 24 px apart, rows on an 8 px rhythm. */
  .today {
    container-type: inline-size;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-4) var(--space-6);
    font-size: var(--text-sm);
    line-height: var(--text-sm-lh);
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .title {
    min-width: 0;
  }
  h2 {
    margin: 0;
    font-size: var(--text-lg);
    line-height: var(--text-lg-lh);
    font-weight: var(--text-lg-weight);
  }
  .dateline {
    margin: 2px 0 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
  }
  .actions {
    display: flex;
    flex-shrink: 0;
    gap: var(--space-1);
  }

  /* ── KPI tiles: label over number, the number in its status colour ── */
  .kpis {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-2);
  }
  @container (min-width: 460px) {
    .kpis {
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }
  }
  .kpi {
    display: flex;
    flex-direction: column-reverse;
    justify-content: flex-end;
    gap: 2px;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
  }
  .kpi .n {
    font-size: var(--text-xl);
    line-height: var(--text-xl-lh);
    font-weight: var(--text-xl-weight);
    font-variant-numeric: tabular-nums;
  }
  .kpi .k {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .kpi.waiting.hot {
    border-color: var(--waiting-line);
    background: var(--waiting-faint);
  }
  .kpi.waiting.hot .n {
    color: var(--status-waiting);
  }
  .kpi.working.hot .n {
    color: var(--status-working);
  }
  .kpi.done.hot .n {
    color: var(--status-done);
  }

  /* ── sections ── */
  .block {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  h3 {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: 0 0 var(--space-1);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
    font-weight: 600;
    color: var(--fg-muted);
  }
  h3.waiting {
    color: var(--status-waiting);
  }
  h3.shipped-h {
    color: var(--status-done);
  }
  .count {
    min-width: 18px;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--count-bg);
    color: var(--fg-2);
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  h3 .tidy {
    margin-left: auto;
    font-weight: 400;
  }
  .groups,
  .sessions {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .groups {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
  }
  .groups + .groups {
    margin-top: var(--space-2);
  }
  .group {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .group:last-child {
    border-bottom: 0;
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
  }
  .what {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    column-gap: var(--space-2);
    padding: 3px 0;
  }
  .head .link {
    flex-shrink: 0;
  }
  .label {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .label.nowork {
    font-weight: 500;
    color: var(--fg-muted);
  }
  .status,
  .meta {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    display: inline-block;
    background: var(--fg-muted);
  }
  .dot-todo,
  .dot-unknown {
    background: var(--fg-muted);
  }
  .dot-progress {
    background: var(--accent);
  }
  .dot-done {
    background: var(--status-done);
  }

  /* ── one session: dot · name over host and age · Open ── */
  .srow,
  .mission,
  .shipped {
    display: grid;
    grid-template-columns: 12px minmax(0, 1fr) auto;
    column-gap: var(--space-2);
    align-items: center;
  }
  .srow {
    padding: var(--space-1) 0;
  }
  .sdot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-pill);
    background: var(--status-idle);
    justify-self: center;
  }
  .tone-waiting {
    background: var(--status-waiting);
  }
  .tone-failed {
    background: var(--status-failed);
  }
  .tone-working {
    background: var(--status-working);
  }
  .smain {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  /* The name is the row's main target: text, not a chip. */
  .btn.session {
    justify-content: flex-start;
    height: auto;
    min-height: var(--control-h);
    padding: 0 var(--space-1);
    margin-left: calc(-1 * var(--space-1));
    color: var(--fg);
    font-size: var(--text-sm);
    font-weight: 500;
    line-height: var(--text-sm-lh);
    white-space: normal;
    text-align: left;
    overflow-wrap: anywhere;
  }
  .smain .meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta.waits {
    color: var(--status-waiting);
  }
  .limit {
    grid-column: 2 / -1;
    padding-top: var(--space-1);
  }
  .glyph {
    justify-self: center;
    color: var(--status-done);
    font-size: var(--text-xs);
  }
  .more {
    align-self: flex-start;
  }
  .brief-card {
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
  }
  .empty {
    margin: 0;
    color: var(--fg-muted);
  }
  .error {
    margin: 0;
    color: var(--danger);
  }
</style>
