<script lang="ts">
  // Pull requests (redesign step 6.4): the Work view's fourth tab. Every PR a
  // session's branch has had, newest change first, with its state, CI and
  // review, when it merged, and the session that opened it. A read: the PR
  // opens on GitHub, the session opens in fleet; nothing here merges.
  //
  // Titles and branch names are text a person wrote: rendered as text.
  //
  // Multi-hub Phase 2: with extra fleets, the list is every fleet the
  // sidebar's fleet filter shows, each PR read from its own hub and marked
  // with its fleet's badge; a PR's session opens on its own fleet.
  import Skeleton from './states/Skeleton.svelte';
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { timeAgo } from './session_status';
  import { openExternal } from './open_external';
  import { openMission } from './missions';
  import { readErrorText } from './work_view';
  import type { IpcError } from './result';
  import { listPullRequests, prChecksLabel, prDiffstat, prRef, prStateLabel, type PrFilter, type PullRequestRow } from './prs';
  import {
    PRIMARY,
    fleetListPullRequests,
    fleetRows,
    fleets,
    multiFleet,
    openFleetSession,
    showPrimary,
    visibleExtraFleets,
    type FleetInfo,
  } from './fleets';

  /** A PR and the fleet whose hub listed it. */
  type Item = PullRequestRow & { fleet: FleetInfo | null };

  const FILTERS: { id: PrFilter; label: string }[] = [
    { id: 'open', label: 'Open' },
    { id: 'merged', label: 'Merged' },
    { id: 'closed', label: 'Closed' },
    { id: 'all', label: 'All' },
  ];

  let filter = $state<PrFilter>('open');
  let items = $state<Item[]>([]);
  let total = $state(0);
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  /** Review w17-7: a re-read of the list shown failed; the list stays. */
  let refreshError = $state<IpcError | null>(null);
  /** The filter and fleets the items shown were read for: a failed re-read
   *  keeps them only when it asked for the same. */
  let shownKey: string | null = null;
  /** Fleets whose hub could not answer while others did (review w01-7):
   *  said above the list, so it never looks complete when it is not. */
  let fleetErrors = $state<{ label: string; message: string }[]>([]);
  let seq = 0;

  async function load() {
    const mine = ++seq;
    const extras = get(multiFleet) ? get(visibleExtraFleets).map((g) => g.fleet).filter((f) => !f.unavailable && !f.duplicate_of) : [];
    const own = get(showPrimary);
    const key = JSON.stringify([filter, own, extras.map((f) => f.id)]);
    const [r, ...others] = await Promise.all([
      own ? listPullRequests({ state: filter }) : Promise.resolve(null),
      ...extras.map((f) => fleetListPullRequests(f.id, { state: filter })),
    ]);
    if (mine !== seq) return;
    loaded = true;
    // The primary alone failing is the page's error; beside extra fleets it
    // is one fleet's note, and theirs stay on screen.
    if (r && !r.ok && extras.length === 0) {
      if (shownKey === key && !loadError) refreshError = r.error;
      else {
        loadError = r.error;
        refreshError = null;
      }
      return;
    }
    const failed: { label: string; message: string }[] = [];
    if (r && !r.ok) {
      const label = get(fleets).find((f) => f.primary)?.label ?? 'this fleet';
      failed.push({ label, message: readErrorText(r.error) });
    }
    const list: Item[] = (r?.ok ? (r.value?.items ?? []) : []).map((pr) => ({ ...pr, fleet: null }));
    let count = r?.ok ? (r.value?.total ?? list.length) : 0;
    extras.forEach((f, i) => {
      const o = others[i];
      // One extra hub that cannot answer leaves the rest on screen, and says so.
      if (!o.ok) {
        failed.push({ label: f.label, message: readErrorText(o.error) });
        return;
      }
      list.push(...(o.value?.items ?? []).map((pr) => ({ ...pr, fleet: f })));
      count += o.value?.total ?? 0;
    });
    if (extras.length > 0) list.sort((a, b) => b.updated_at - a.updated_at);
    items = list;
    total = count;
    fleetErrors = failed;
    loadError = null;
    refreshError = null;
    shownKey = key;
  }

  // The fleet filter moved: re-read for the fleets it now shows.
  let lastFleets = '';
  const unsubFleets = visibleExtraFleets.subscribe((v) => {
    const sig = `${get(showPrimary)}|${v.map((g) => g.fleet.id).join(',')}`;
    if (sig === lastFleets) return;
    const first = lastFleets === '';
    lastFleets = sig;
    if (!first) void load();
  });

  function pick(f: PrFilter) {
    if (f === filter) return;
    filter = f;
    // Review w17-12: the old filter's list never stands under the new tab;
    // the skeleton shows (after its delay) until the hub answers.
    loaded = false;
    loadError = null;
    refreshError = null;
    void load();
  }

  // Reconcile moves a PR when it moves a session's `pr_url` / `ci_status` /
  // evidence: re-read when those change, not on a timer.
  let lastSig = '';
  let timer: ReturnType<typeof setTimeout> | null = null;
  const prSig = (rows: Iterable<SessionRow>) =>
    [...rows].map((r) => `${r.id}:${r.pr_url ?? ''}:${r.ci_status ?? ''}:${r.pr_evidence?.state ?? ''}`).join('|');
  function reloadSoon() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void load(), 500);
  }
  const unsub = sessions.subscribe((rows) => {
    const sig = prSig(rows);
    if (sig === lastSig) return;
    const first = lastSig === '';
    lastSig = sig;
    if (first) return;
    reloadSoon();
  });
  // The same signal from each extra fleet's sessions: a company session
  // opening a PR re-reads the page too (review w01-7).
  let lastFleetSig: string | null = null;
  const unsubFleetRows = fleetRows.subscribe((all) => {
    const sig = [...all].map(([id, rows]) => `${id}=${prSig(rows.sessions.values())}`).join('#');
    if (sig === lastFleetSig) return;
    const first = lastFleetSig === null;
    lastFleetSig = sig;
    if (first) return;
    reloadSoon();
  });

  onMount(() => void load());
  onDestroy(() => {
    unsub();
    unsubFleets();
    unsubFleetRows();
    if (timer) clearTimeout(timer);
  });

  function sessionRow(pr: Item) {
    if (pr.session_id == null) return undefined;
    if (pr.fleet) return $fleetRows.get(pr.fleet.id)?.sessions.get(pr.session_id);
    return get(sessions).find((r) => r.id === pr.session_id);
  }

  function openSession(pr: Item) {
    const row = sessionRow(pr);
    if (!row) return;
    if (pr.fleet && pr.fleet.id !== PRIMARY) openFleetSession(pr.fleet.id, row.id);
    else selectSessionExplicitly(row);
  }
</script>

<section class="work-prs" data-testid="work-prs" aria-label="Pull requests">
  <div class="filters" role="group" aria-label="Which pull requests">
    {#each FILTERS as f (f.id)}
      <button
        class="btn btn--chip btn--toggle"
        type="button"
        aria-pressed={filter === f.id}
        class:is-active={filter === f.id}
        data-testid="work-prs-filter-{f.id}"
        onclick={() => pick(f.id)}>{f.label}</button
      >
    {/each}
  </div>

  {#if !loadError && loaded}
    {#each fleetErrors as e (e.label)}
      <p class="error" role="status" data-testid="work-prs-fleet-error">Couldn't read PRs from {e.label}: {e.message}</p>
    {/each}
  {/if}
  {#if loadError}
    <div class="error" role="alert" data-testid="work-prs-error">
      <p>{readErrorText(loadError)}</p>
      <button class="btn" type="button" onclick={() => void load()}>Retry</button>
    </div>
  {:else if !loaded}
    <div data-testid="work-prs-loading"><Skeleton rows={3} label="Loading pull requests" /></div>
  {:else if items.length === 0}
    <p class="muted" data-testid="work-prs-empty">
      {filter === 'all' ? 'No pull requests yet. A session’s PR shows here once fleet sees it.' : `No ${filter} pull requests.`}
    </p>
  {:else}
    {#if refreshError}
      <p class="refresh-error" role="status" data-testid="work-prs-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="work-prs-refresh-retry" onclick={() => void load()}>Retry</button>
      </p>
    {/if}
    <ul class="items">
      {#each items as pr (`${pr.fleet?.id ?? PRIMARY}:${pr.id}`)}
        {@const checks = prChecksLabel(pr)}
        {@const live = sessionRow(pr)}
        {@const stat = prDiffstat(pr)}
        <li class="item" data-testid="work-pr" data-state={pr.state}>
          <div class="head">
            {#if pr.fleet}<span class="fleet-badge" style="--fleet: {pr.fleet.color}" data-testid="work-pr-fleet">{pr.fleet.label}</span>{/if}
            <span class="state state--{pr.draft && pr.state === 'OPEN' ? 'draft' : pr.state.toLowerCase()}" data-testid="work-pr-state"
              >{prStateLabel(pr)}</span
            >
            <button class="link" type="button" title="Open on GitHub" data-testid="work-pr-open" onclick={() => void openExternal(pr.url)}
              >{pr.title || prRef(pr)}</button
            >
          </div>
          <div class="sub muted">
            <span data-testid="work-pr-ref">{prRef(pr)}</span>
            {#if pr.head_ref}<span>· {pr.head_ref}</span>{/if}
            {#if stat}<span class="diffstat" data-testid="work-pr-diffstat" aria-label="{pr.additions ?? 0} lines added, {pr.deletions ?? 0} removed"
                >· <span class="add">+{pr.additions ?? 0}</span> <span class="del">−{pr.deletions ?? 0}</span></span
              >{/if}
            {#if checks}<span class="checks checks--{pr.ci_status ?? 'none'}" data-testid="work-pr-checks">· {checks}</span>{/if}
            {#if pr.state === 'MERGED' && pr.merged_at}<span data-testid="work-pr-merged">· merged {timeAgo(pr.merged_at)}</span>{/if}
          </div>
          {#if pr.mission_id != null && pr.mission_name}
            <div class="sub muted">
              <!-- G7.8 (MCViews board): "from <mission>", opening it. -->
              from
              {#if pr.fleet}
                <!-- Mission ids are per hub: this id would open the primary's
                     mission of that number (review w01-1). Text until missions
                     open per fleet. -->
                <span data-testid="work-pr-mission-text">{pr.mission_name}</span>
              {:else}
                <button class="link muted" type="button" title="Open the mission" data-testid="work-pr-mission" onclick={() => openMission(pr.mission_id!)}
                  >{pr.mission_name}</button
                >
              {/if}
            </div>
          {/if}
          {#if pr.session_name}
            <div class="sub muted">
              Opened by
              {#if live}
                <button class="link muted" type="button" title="Open the session" data-testid="work-pr-session" onclick={() => openSession(pr)}
                  >{pr.session_name}{#if pr.host_alias}&nbsp;· {pr.host_alias}{/if}</button
                >
              {:else}
                <span data-testid="work-pr-session-gone">{pr.session_name}{#if pr.host_alias}&nbsp;· {pr.host_alias}{/if} (gone)</span>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if total > items.length}
      <p class="muted" data-testid="work-prs-more">Showing {items.length} of {total}.</p>
    {/if}
  {/if}
</section>

<style>
  .fleet-badge {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    border: 1px solid var(--fleet);
    background: color-mix(in srgb, var(--fleet) 16%, transparent);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .diffstat .add {
    color: var(--status-done);
  }
  .diffstat .del {
    color: var(--status-failed);
  }
  .work-prs {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    font-size: var(--text-2xs);
  }
  .filters,
  .head,
  .sub {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .items {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item {
    padding: 0.35rem 0.3rem;
    border-bottom: 1px solid var(--border);
  }
  .sub {
    margin-left: 0.2rem;
    font-size: var(--text-2xs);
  }
  .state {
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 0 0.35rem;
    color: var(--fg-muted);
  }
  .state--open {
    color: var(--status-working);
    border-color: currentColor;
  }
  .state--merged {
    color: var(--accent);
    border-color: currentColor;
  }
  .checks--failing {
    color: var(--usage-crit);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--fg);
    text-align: left;
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .link:hover {
    text-decoration: underline;
  }
  .refresh-error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .muted {
    color: var(--fg-muted);
  }
  .error {
    color: var(--usage-crit);
  }
</style>
