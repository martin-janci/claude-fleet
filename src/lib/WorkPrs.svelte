<script lang="ts">
  // Pull requests (redesign step 6.4): the Work view's fourth tab. Every PR a
  // session's branch has had, newest change first, with its state, CI and
  // review, when it merged, and the session that opened it. A read: the PR
  // opens on GitHub, the session opens in fleet; nothing here merges.
  //
  // Titles and branch names are text a person wrote: rendered as text.
  import Skeleton from './states/Skeleton.svelte';
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { timeAgo } from './session_status';
  import { openExternal } from './open_external';
  import { openMission } from './missions';
  import { readErrorText } from './work_view';
  import type { IpcError } from './result';
  import { listPullRequests, prChecksLabel, prDiffstat, prRef, prStateLabel, type PrFilter, type PullRequestRow } from './prs';

  const FILTERS: { id: PrFilter; label: string }[] = [
    { id: 'open', label: 'Open' },
    { id: 'merged', label: 'Merged' },
    { id: 'closed', label: 'Closed' },
    { id: 'all', label: 'All' },
  ];

  let filter = $state<PrFilter>('open');
  let items = $state<PullRequestRow[]>([]);
  let total = $state(0);
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  let seq = 0;

  async function load() {
    const mine = ++seq;
    const r = await listPullRequests({ state: filter });
    if (mine !== seq) return;
    loaded = true;
    if (r.ok) {
      items = r.value?.items ?? [];
      total = r.value?.total ?? items.length;
      loadError = null;
    } else {
      loadError = r.error;
    }
  }

  function pick(f: PrFilter) {
    if (f === filter) return;
    filter = f;
    void load();
  }

  // Reconcile moves a PR when it moves a session's `pr_url` / `ci_status` /
  // evidence: re-read when those change, not on a timer.
  let lastSig = '';
  let timer: ReturnType<typeof setTimeout> | null = null;
  const unsub = sessions.subscribe((rows) => {
    const sig = rows
      .map((r) => `${r.id}:${r.pr_url ?? ''}:${r.ci_status ?? ''}:${r.pr_evidence?.state ?? ''}`)
      .join('|');
    if (sig === lastSig) return;
    const first = lastSig === '';
    lastSig = sig;
    if (first) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void load(), 500);
  });

  onMount(() => void load());
  onDestroy(() => {
    unsub();
    if (timer) clearTimeout(timer);
  });

  function sessionRow(pr: PullRequestRow) {
    return pr.session_id == null ? undefined : get(sessions).find((r) => r.id === pr.session_id);
  }

  function openSession(pr: PullRequestRow) {
    const row = sessionRow(pr);
    if (row) selectSessionExplicitly(row);
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
    <ul class="items">
      {#each items as pr (pr.id)}
        {@const checks = prChecksLabel(pr)}
        {@const live = sessionRow(pr)}
        {@const stat = prDiffstat(pr)}
        <li class="item" data-testid="work-pr" data-state={pr.state}>
          <div class="head">
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
              <button class="link muted" type="button" title="Open the mission" data-testid="work-pr-mission" onclick={() => openMission(pr.mission_id!)}
                >{pr.mission_name}</button
              >
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
  .muted {
    color: var(--fg-muted);
  }
  .error {
    color: var(--usage-crit);
  }
</style>
