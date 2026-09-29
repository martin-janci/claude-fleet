<script lang="ts">
  import { onWorkChangedDebounced } from './work';
  // The Work view's tree (work graph M14), shown in the sidebar in place of
  // the Sessions tree: organisation → group → task → every session of the
  // task (primary ★, secondary, suggested, past). A session under several
  // tasks is one session: opening any of its occurrences selects the same
  // `session_id`, and every occurrence of the selected session is lit.
  //
  // Scale: the first `work_tree` page draws every section header (the
  // `groups` counts cover the whole filtered result) and fills the first
  // sections; any other section loads by itself (`filters.group` + its org)
  // and pages with its own cursor ("Load more"). Expansion and the last
  // selection are kept per view (prefs). `work:changed` and session events
  // that touch work re-read what is loaded, debounced.
  //
  // Tracker text (titles, keys) is rendered as text, never as markup.
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { providerInfo, unavailableLabel } from './trackers';
  import { timeAgo } from './session_status';
  import { workViewChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';
  import WorkFiltersBar from './WorkFiltersBar.svelte';
  import { facetSentence, workFacets } from './filter_facets';
  import WorkReview from './WorkReview.svelte';
  import WorkRules from './WorkRules.svelte';
  import {
    buildSections,
    distributeTasks,
    filtersKey,
    isOccurrenceOf,
    mergeTasks,
    occurrenceKind,
    openTask,
    orgSectionKey,
    readErrorText,
    revealTaskRequest,
    sectionFilters,
    sectionKey,
    selectedTaskId,
    setExpanded,
    taskLabel,
    taskStatus,
    trackerDown,
    trackerDownLabel,
    workExpanded,
    workReview,
    workTask,
    workTree,
    workTreeMeta,
    workTreeSessionIds,
    workViewFilters,
    workViewKey,
    activeWorkViewId,
    normalizeFilters,
    type GroupSection,
    type OrgSection,
    type SectionState,
    type WorkTask,
    type WorkTaskLink,
    type WorkTreeGroup,
    type WorkTreePage,
  } from './work_view';
  import type { IpcError } from './result';

  let {
    /** Tasks per page; injectable for tests. */
    pageSize = 50,
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
    /** The longest a steady stream of changes may hold a refetch back, ms. */
    maxWaitMs = 3000,
  }: { pageSize?: number; debounceMs?: number; maxWaitMs?: number } = $props();

  const chord = workViewChordLabel(detectMac(typeof navigator === 'undefined' ? undefined : navigator));

  let tab = $state<'tasks' | 'review'>('tasks');
  let page = $state.raw<WorkTreePage | null>(null);
  const archivedHidden = $derived(page?.archived_hidden ?? 0);
  function setArchived(on: boolean) {
    workViewFilters.update((f) => {
      const { group: _g, archived: _a, ...rest } = normalizeFilters(f);
      return on ? { ...rest, archived: true } : rest;
    });
  }
  let states = $state.raw<Map<string, SectionState>>(new Map());
  let loading = $state(false);
  let error = $state<IpcError | null>(null);
  // A re-read of the view already shown that failed: the tree stays, with a
  // line to retry (the full error is for a first load, or new filters).
  let refreshError = $state<IpcError | null>(null);
  // The filters the page shown was read with.
  let pageFiltersKey: string | null = null;
  let sectionBusy = $state.raw<Set<string>>(new Set());
  let sectionErrors = $state.raw<Map<string, string>>(new Map());
  let reviewTotal = $state<number | null>(null);
  let rulesOpen = $state(false);
  let root = $state<HTMLDivElement | null>(null);

  const sections: OrgSection[] = $derived(page ? buildSections(page.groups, page.orgs, states) : []);
  const selectedSessionId = $derived($selectedSession?.id ?? null);
  const expanded = $derived($workExpanded[$workViewKey] ?? {});

  function orgOpen(o: OrgSection): boolean {
    return expanded[o.key] ?? true;
  }
  // A section the first page filled is open unless closed; any other is
  // closed until opened (it costs a read).
  function groupOpen(g: GroupSection): boolean {
    return expanded[g.key] ?? states.has(g.key);
  }

  /** A page as a newer or older hub may send it: never undefined arrays. */
  function pageOf(v: Partial<WorkTreePage> | null | undefined): WorkTreePage {
    return {
      tasks: Array.isArray(v?.tasks) ? v.tasks : [],
      groups: Array.isArray(v?.groups) ? v.groups : [],
      orgs: Array.isArray(v?.orgs) ? v.orgs : [],
      trackers: Array.isArray(v?.trackers) ? v.trackers : [],
      total: typeof v?.total === 'number' ? v.total : 0,
      archived_hidden: typeof v?.archived_hidden === 'number' ? v.archived_hidden : 0,
      next_cursor: v?.next_cursor ?? null,
      generated_at: v?.generated_at,
    };
  }

  /** The most tasks one `work_tree` read returns. */
  const PAGE_MAX = 200;

  let loadSeq = 0;
  async function load() {
    const mine = ++loadSeq;
    const filters = get(workViewFilters);
    const fk = filtersKey(filters);
    // What each section had, so a refresh re-reads as much as was shown.
    const had = new Map<string, number>();
    for (const [k, s] of states) if (s.own) had.set(k, s.tasks.length);
    loading = true;
    const r = await workTree({ filters, limit: pageSize });
    if (mine !== loadSeq) return;
    loading = false;
    if (!r.ok) {
      if (page && pageFiltersKey === fk) {
        refreshError = r.error;
      } else {
        error = r.error;
        refreshError = null;
      }
      return;
    }
    error = null;
    refreshError = null;
    // A refresh of the view shown (same filters) keeps each open section it
    // re-reads below until that read answers, so a failed or slow re-read
    // never blanks what was loaded.
    const sameView = pageFiltersKey === fk && fk === lastFiltersKey;
    pageFiltersKey = fk;
    const p = pageOf(r.value);
    page = p;
    const fresh = distributeTasks(p);
    const openNow = get(workExpanded)[get(workViewKey)] ?? {};
    const keep = new Map<string, SectionState>();
    if (sameView) for (const [k, st] of states) if (st.own && openNow[k]) keep.set(k, st);
    states = distributeTasks(p, keep);
    sectionErrors = new Map();
    // A section read still in flight answers for the view before this one:
    // cancel it (its answer is dropped), and re-issue it below when the
    // section is still open — so a section never stays "Loading…".
    cancelSections();
    workTreeMeta.set({ orgs: p.orgs, trackers: p.trackers, groups: p.groups });
    // Open sections the first page did not cover load by themselves.
    for (const g of p.groups) {
      const k = sectionKey(g.org_id, g.group.id);
      const open = openNow[k] ?? fresh.has(k);
      if (!open) continue;
      const shown = fresh.get(k)?.tasks.length ?? 0;
      if (shown >= g.count) {
        // The first page covers it all now: what was kept is stale.
        if (states.get(k)?.own) states = new Map(states).set(k, fresh.get(k)!);
        continue;
      }
      const want = sameView ? Math.max(pageSize, had.get(k) ?? 0) : pageSize;
      void loadSection(g, false, want);
    }
    loadedOnce = true;
    flushReveal();
  }

  // Each section's request generation: an answer whose generation is no
  // longer the section's was cancelled (a refresh re-issued it) and is
  // dropped — the newer request owns the section's busy flag.
  const sectionGen = new Map<string, number>();
  // The request in flight per section, so a caller that finds it busy (a
  // reveal) can wait for it.
  const sectionReq = new Map<string, Promise<void>>();
  function cancelSections() {
    for (const [k, n] of sectionGen) sectionGen.set(k, n + 1);
    sectionReq.clear();
    sectionBusy = new Set();
  }

  function loadSection(g: WorkTreeGroup, more: boolean, limit: number = pageSize): Promise<void> {
    const k = sectionKey(g.org_id, g.group.id);
    const inFlight = sectionReq.get(k);
    if (inFlight && sectionBusy.has(k)) return inFlight;
    const p = readSection(g, k, more, limit);
    sectionReq.set(k, p);
    return p;
  }

  /** Read one section: `limit` tasks from its cursor (`more`) or from its
   *  start, in pages of at most `PAGE_MAX` (a refresh re-reads as many as
   *  were shown, so a section paged past one page does not shrink back). */
  async function readSection(g: WorkTreeGroup, k: string, more: boolean, limit: number) {
    const gen = (sectionGen.get(k) ?? 0) + 1;
    sectionGen.set(k, gen);
    const fk = lastFiltersKey;
    const st = states.get(k);
    const cursor = more && st?.own ? st.cursor : null;
    sectionBusy = new Set(sectionBusy).add(k);
    const filters = sectionFilters(get(workViewFilters), g.org_id ?? null, g.group.id);
    let asked = Math.min(PAGE_MAX, limit);
    let r = await workTree({ filters, cursor, limit: asked });
    if (r.ok && !more) {
      let got = pageOf(r.value);
      let tasks = got.tasks;
      // Only past a full page: a short page is all the hub had to give.
      while (sectionGen.get(k) === gen && got.next_cursor && got.tasks.length >= asked && tasks.length < limit) {
        asked = Math.min(PAGE_MAX, limit - tasks.length);
        const next = await workTree({ filters, cursor: got.next_cursor, limit: asked });
        if (!next.ok) {
          r = next;
          break;
        }
        got = pageOf(next.value);
        tasks = mergeTasks(tasks, got.tasks);
      }
      if (r.ok) r = { ok: true, value: { ...got, tasks } };
    }
    if (sectionGen.get(k) !== gen) return;
    sectionReq.delete(k);
    const busy = new Set(sectionBusy);
    busy.delete(k);
    sectionBusy = busy;
    // The filters moved on while this was in flight: the view's reload
    // re-reads the section when it is still open.
    if (fk !== lastFiltersKey) return;
    const errs = new Map(sectionErrors);
    if (!r.ok) {
      errs.set(k, readErrorText(r.error));
      sectionErrors = errs;
      return;
    }
    errs.delete(k);
    sectionErrors = errs;
    const p = pageOf(r.value);
    const cur = states.get(k);
    // A re-read from the start replaces what the section had loaded by
    // itself (a task that left it goes); the first page's share is merged.
    const tasks = more || (cur && !cur.own) ? mergeTasks(cur?.tasks ?? [], p.tasks) : p.tasks;
    states = new Map(states).set(k, { tasks, cursor: p.next_cursor ?? null, own: true });
  }

  async function loadReviewCount() {
    const r = await workReview({ limit: 1 });
    reviewTotal = r.ok && r.value && typeof r.value.total === 'number' ? r.value.total : null;
  }

  // Filters: reload everything when they change (the search is debounced
  // in the bar).
  let lastFiltersKey = filtersKey(get(workViewFilters));
  const offFilters = workViewFilters.subscribe((f) => {
    const k = filtersKey(f);
    if (k === lastFiltersKey) return;
    lastFiltersKey = k;
    void load();
  });

  // `work:changed` / session events: one debounced re-read — at most
  // `maxWaitMs` after the first change it waits for, so a steady stream of
  // changes (a busy fleet) cannot hold the view back forever.
  const offChanged = onWorkChangedDebounced(
    () => {
      void load();
      void loadReviewCount();
    },
    () => debounceMs,
    () => maxWaitMs,
  );

  onMount(() => {
    void load();
    void loadReviewCount();
  });
  onDestroy(() => {
    offFilters();
    offChanged();
    offReveal();
    workTreeSessionIds.set(new Set());
  });

  // The sessions the tree shows: their status changes refresh it too.
  $effect(() => {
    const ids = new Set<number>();
    for (const s of states.values())
      for (const t of s.tasks) for (const l of t.sessions ?? []) if (l.session_id != null) ids.add(l.session_id);
    workTreeSessionIds.set(ids);
  });

  function toggleOrg(o: OrgSection) {
    setExpanded(get(workViewKey), o.key, !orgOpen(o));
  }

  function toggleGroup(g: GroupSection) {
    const open = !groupOpen(g);
    setExpanded(get(workViewKey), g.key, open);
    if (open && g.tasks.length < g.count && !states.get(g.key)?.own) {
      void loadSection({ org_id: g.orgId, group: g.group, count: g.count }, false);
    }
  }

  function loadMore(g: GroupSection) {
    void loadSection({ org_id: g.orgId, group: g.group, count: g.count }, true);
  }

  function selectTask(t: WorkTask) {
    openTask(t.task_id);
  }

  // Opening an occurrence opens its session — the same `session_id` from
  // every task it is under. A past link names no live session: the task's
  // detail shows it instead.
  function openOccurrence(t: WorkTask, l: WorkTaskLink) {
    const row =
      l.session_id != null && l.state !== 'ended' ? $sessions.find((r) => r.id === l.session_id) : undefined;
    if (row) {
      selectedTaskId.set(t.task_id);
      selectSessionExplicitly(row);
    } else {
      openTask(t.task_id);
    }
  }

  function occurrenceTitle(l: WorkTaskLink): string {
    const kind = occurrenceKind(l);
    const what =
      kind === 'primary'
        ? 'primary work of this session'
        : kind === 'secondary'
          ? 'also worked on by this session (not its primary)'
          : kind === 'suggested'
            ? 'a suggestion nobody has decided — it never groups a session'
            : kind === 'rejected'
              ? 'rejected'
              : `ended${l.ended_at ? ` ${timeAgo(l.ended_at)}` : ''}`;
    const parts = [what, l.host ?? '', l.why ?? ''].filter(Boolean);
    if (l.cross_org) parts.push('links two organisations');
    return parts.join(' · ');
  }

  // "Show in Work view": expand the task's section (loading it when the
  // first page did not reach it) and scroll to it.
  async function reveal(taskId: string) {
    const vk = get(workViewKey);
    let where: { orgId: number | null; groupId: string } | null = null;
    for (const s of states.values()) {
      const t = s.tasks.find((x) => x.task_id === taskId);
      if (t) {
        where = { orgId: t.org_id ?? null, groupId: t.group?.id ?? 'none' };
        break;
      }
    }
    if (!where) {
      const r = await workTask(taskId);
      if (!r.ok || !r.value?.task) return;
      where = { orgId: r.value.task.org_id ?? null, groupId: r.value.task.group?.id ?? 'none' };
    }
    const k = sectionKey(where.orgId, where.groupId);
    setExpanded(vk, orgSectionKey(where.orgId), true);
    setExpanded(vk, k, true);
    const g = page?.groups.find((x) => sectionKey(x.org_id, x.group.id) === k);
    if (g && !(states.get(k)?.tasks ?? []).some((t) => t.task_id === taskId)) await loadSection(g, false);
    await tick();
    const sel = typeof CSS !== 'undefined' && CSS.escape ? CSS.escape(taskId) : taskId.replace(/["\\]/g, '\\$&');
    const el = root?.querySelector<HTMLElement>(`[data-task-id="${sel}"]`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }
  // The request waits in its store until a tree has loaded to reveal in:
  // "Show in Work view" usually mounts this tree, so it is made before it.
  let loadedOnce = false;
  function flushReveal() {
    const req = get(revealTaskRequest);
    if (!req || !loadedOnce) return;
    revealTaskRequest.set(null);
    tab = 'tasks';
    void reveal(req.taskId);
  }
  const offReveal = revealTaskRequest.subscribe((req) => {
    if (req) flushReveal();
  });

  function badge(t: WorkTask): string {
    if (t.kind === 'local') return 'local';
    if (t.kind === 'ref') return 'key';
    return providerInfo(t.provider)?.icon ?? t.provider ?? 'tracker';
  }
</script>

<div class="work-tree" data-testid="work-tree" aria-busy={loading} bind:this={root}>
  <header class="work-header">
    <div class="row">
      <div class="tabs" role="tablist" aria-label="Work view">
        <button
          class="btn btn--chip btn--toggle"
          role="tab"
          aria-selected={tab === 'tasks'}
          class:is-active={tab === 'tasks'}
          data-testid="work-tab-tasks"
          onclick={() => (tab = 'tasks')}>Tasks</button
        >
        <button
          class="btn btn--chip btn--toggle"
          role="tab"
          aria-selected={tab === 'review'}
          class:is-active={tab === 'review'}
          data-testid="work-tab-review"
          onclick={() => (tab = 'review')}>Review{#if reviewTotal}&nbsp;· {reviewTotal}{/if}</button
        >
      </div>
      <button
        class="btn btn--quiet btn--icon"
        type="button"
        title="Placement rules"
        aria-label="Placement rules"
        data-testid="work-rules-open"
        onclick={() => (rulesOpen = true)}>⚙</button
      >
      <!-- Refresh is the sidebar's ↻ (it re-reads this view too), and
           collapse is the sidebar's ‹: one of each. -->
    </div>
    {#if tab === 'tasks'}
      <WorkFiltersBar orgs={page?.orgs ?? []} trackers={page?.trackers ?? []} />
    {/if}
  </header>

  <div class="scroller">
    {#if tab === 'tasks' && refreshError && page && !error}
      <p class="refresh-error" role="status" data-testid="work-tree-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="work-tree-refresh-retry" onclick={() => void load()}>Retry</button>
      </p>
    {/if}
    {#if tab === 'review'}
      <WorkReview onchanged={() => void loadReviewCount()} />
    {:else if error}
      <div class="state error" role="alert" data-testid="work-tree-error">
        <p>{readErrorText(error)}</p>
        <button class="btn" type="button" data-testid="work-tree-retry" onclick={() => void load()}>Retry</button>
      </div>
    {:else if !page}
      <p class="state muted" data-testid="work-tree-loading">Loading work…</p>
    {:else if sections.length === 0}
      {@const facets = workFacets($workViewFilters, {
        orgName: (id) => page?.orgs.find((o) => o.id === id)?.name,
        trackerName: (id) => page?.trackers.find((t) => t.id === id)?.name,
      })}
      <div class="state muted empty" data-testid="work-tree-empty">
        {#if facets.length > 0}
          <p>No tasks match <strong>{facetSentence(facets)}</strong>.</p>
          <button
            class="btn btn--quiet is-bounded"
            type="button"
            data-testid="work-tree-empty-clear"
            onclick={() => {
              activeWorkViewId.set(null);
              workViewFilters.set({});
            }}>Clear filters</button
          >
        {:else if archivedHidden === 0}
          <p>No work yet. Tasks appear here once a session is linked to a ticket, or you name its work. Back to Sessions: {chord}.</p>
        {/if}
        {#if archivedHidden > 0}
          {@render archivedRow()}
        {/if}
      </div>
    {:else}
      <ul class="orgs" aria-label="Work">
        {#each sections as o (o.key)}
          <li class="org" data-testid="work-org">
            <button class="org-head" type="button" data-testid="work-org-head" aria-expanded={orgOpen(o)} onclick={() => toggleOrg(o)}>
              <span class="caret" class:open={orgOpen(o)} aria-hidden="true">▸</span>
              {#if o.color}<span class="org-dot" style="background: {o.color}" aria-hidden="true"></span>{/if}
              <span class="org-name">{o.name}</span>
              <span class="count">{o.count}</span>
            </button>
            {#if orgOpen(o)}
              <ul class="groups">
                {#each o.groups as g (g.key)}
                  <li class="group" data-testid="work-group" data-group-id={g.group.id}>
                    <button class="group-head" type="button" data-testid="work-group-head" aria-expanded={groupOpen(g)} onclick={() => toggleGroup(g)}>
                      <span class="caret" class:open={groupOpen(g)} aria-hidden="true">▸</span>
                      <span class="group-name" class:none={g.group.source === 'none'}
                        >{g.group.source === 'none' ? 'No group' : g.group.label}</span
                      >
                      <span class="source" title="Where this group comes from">{g.group.source}</span>
                      <span class="count" data-testid="work-group-count">{g.count}</span>
                    </button>
                    {#if groupOpen(g)}
                      <ul class="tasks">
                        {#each g.tasks as t (t.task_id)}
                          {@const lit = (t.sessions ?? []).some((l) => isOccurrenceOf(l, selectedSessionId))}
                          <li
                            class="task"
                            class:selected={$selectedTaskId === t.task_id}
                            class:lit
                            data-testid="work-task"
                            data-task-id={t.task_id}
                          >
                            <button
                              class="task-row"
                              type="button"
                              data-testid="work-task-row"
                              aria-current={$selectedTaskId === t.task_id ? 'true' : undefined}
                              onclick={() => selectTask(t)}
                            >
                              <span class="tbadge" title={t.tracker_name ?? t.kind}>{badge(t)}</span>
                              {#if t.needs_you}<span class="needs" data-testid="work-task-needs-you" title="A session needs you" aria-label="needs you">●</span>{/if}
                              <span class="tlabel" class:unavailable={t.unavailable} title={t.unavailable ? unavailableLabel(t.unavailable_reason) : taskLabel(t)}>
                                {#if t.key}<span class="key">{t.key}</span>{/if}
                                <span class="title">{t.title || (t.key ? '' : t.task_id)}</span>
                              </span>
                              {#if t.review}<span class="review" data-testid="work-task-review" title="Something to review">?</span>{/if}
                            </button>
                            <div class="task-meta">
                              {#if taskStatus(t)}<span class="status">{taskStatus(t)}</span>{/if}
                              <span class="counts" data-testid="work-task-counts" title="active / past sessions"
                                >{t.counts?.active ?? 0} active · {t.counts?.ended ?? 0} past</span
                              >
                              {#if trackerDown(t)}<span class="down" data-testid="work-task-tracker-down" title={`tracker state: ${t.tracker_state}`}>{trackerDownLabel(t)}</span>{/if}
                            </div>
                            {#if (t.sessions ?? []).length > 0}
                              <ul class="occurrences">
                                {#each t.sessions ?? [] as l (l.link_id)}
                                  {@const kind = occurrenceKind(l)}
                                  <li>
                                    <button
                                      class="occ occ--{kind}"
                                      class:current={isOccurrenceOf(l, selectedSessionId)}
                                      type="button"
                                      data-testid="work-occurrence"
                                      data-kind={kind}
                                      data-session-id={l.session_id ?? ''}
                                      title={occurrenceTitle(l)}
                                      onclick={() => openOccurrence(t, l)}
                                    >
                                      <span class="mark" aria-hidden="true"
                                        >{kind === 'primary' ? '★' : kind === 'suggested' ? '?' : kind === 'past' ? '·' : '○'}</span
                                      >
                                      <span class="oname">{l.name ?? `session ${l.session_id ?? l.link_id}`}</span>
                                      {#if l.host}<span class="ohost">{l.host}</span>{/if}
                                      {#if kind === 'past'}<span class="ended">ended</span>{/if}
                                      {#if kind === 'suggested'}<span class="sr">suggested</span>{/if}
                                      {#if l.needs_you}<span class="needs" aria-label="needs you">●</span>{/if}
                                    </button>
                                  </li>
                                {/each}
                                {#if (t.sessions_more ?? 0) > 0}
                                  <li>
                                    <button class="occ more" type="button" onclick={() => selectTask(t)}
                                      >+{t.sessions_more} more</button
                                    >
                                  </li>
                                {/if}
                              </ul>
                            {/if}
                          </li>
                        {/each}
                        {#if sectionBusy.has(g.key)}
                          <li class="state muted" data-testid="work-section-loading">Loading…</li>
                        {/if}
                        {#if sectionErrors.get(g.key)}
                          <li class="state error" role="alert" data-testid="work-section-error">
                            {sectionErrors.get(g.key)}
                            <button class="btn btn--quiet" type="button" onclick={() => loadMore(g)}>Retry</button>
                          </li>
                        {:else if g.more && !sectionBusy.has(g.key)}
                          <li>
                            <button class="btn btn--quiet more-btn" type="button" data-testid="work-load-more" onclick={() => loadMore(g)}
                              >Load more ({g.count - g.tasks.length} left)</button
                            >
                          </li>
                        {/if}
                      </ul>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
      {#if archivedHidden > 0 || $workViewFilters.archived}
        {@render archivedRow()}
      {/if}
    {/if}
  </div>
</div>

{#snippet archivedRow()}
  <!-- Archived tasks (done, or every session archived, and nothing
       running) stay out of the way; one click brings them all back. -->
  <div class="archived-row" data-testid="work-archived-row">
    {#if $workViewFilters.archived}
      <span>Showing archived tasks</span>
      <button class="btn btn--quiet" type="button" data-testid="work-archived-toggle" onclick={() => setArchived(false)}
        >Hide archived</button
      >
    {:else}
      <span>{archivedHidden} archived task{archivedHidden === 1 ? '' : 's'} hidden</span>
      <button class="btn btn--quiet" type="button" data-testid="work-archived-toggle" onclick={() => setArchived(true)}
        >Show archived</button
      >
    {/if}
  </div>
{/snippet}

{#if rulesOpen}
  <WorkRules onclose={() => (rulesOpen = false)} />
{/if}

<style>
  .work-tree {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 0;
    font-size: 0.85rem;
  }
  .work-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }
  .tabs {
    display: flex;
    gap: 0.25rem;
    flex: 1 1 auto;
  }
  .scroller {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
    padding: 0.4rem 0.6rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .org-head,
  .group-head,
  .task-row,
  .occ {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    width: 100%;
    background: transparent;
    border: 0;
    color: var(--fg);
    font: inherit;
    text-align: left;
    padding: 0.2rem 0.3rem;
    border-radius: 4px;
    cursor: pointer;
  }
  .org-head:hover,
  .group-head:hover,
  .task-row:hover,
  .occ:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .org-head:focus-visible,
  .group-head:focus-visible,
  .task-row:focus-visible,
  .occ:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .org-head {
    font-weight: 600;
  }
  .groups {
    padding-left: 0.5rem;
  }
  .group-head {
    font-weight: 500;
  }
  .group-name.none {
    color: var(--fg-muted);
    font-style: italic;
  }
  .caret {
    color: var(--fg-muted);
    font-size: 0.65rem;
    width: 0.7rem;
    transition: transform 0.1s ease;
  }
  .caret.open {
    transform: rotate(90deg);
  }
  .org-dot {
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
  }
  .count {
    margin-left: auto;
    color: var(--fg-muted);
    font-size: 0.75rem;
  }
  .source {
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
  .tasks {
    padding-left: 0.6rem;
  }
  .task {
    border-radius: 4px;
    margin: 0.1rem 0;
  }
  .task.selected {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .task.lit {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .tbadge {
    font-size: 0.65rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.2rem;
    color: var(--fg-muted);
    flex: 0 0 auto;
  }
  .tlabel {
    display: flex;
    gap: 0.3rem;
    min-width: 0;
    flex: 1 1 auto;
    overflow: hidden;
  }
  .tlabel .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tlabel.unavailable {
    text-decoration: line-through;
    color: var(--fg-muted);
  }
  .key {
    font-family: var(--mono);
    flex: 0 0 auto;
  }
  .needs {
    color: var(--usage-crit, #c62828);
    font-size: 0.6rem;
  }
  .review {
    font-weight: 700;
    color: var(--usage-warn, #b45309);
  }
  .task-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0 0.4rem;
    padding-left: 1.6rem;
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  /* Each fact wraps as a whole in a narrow sidebar, never mid-phrase. */
  .task-meta > span {
    white-space: nowrap;
  }
  .down {
    color: var(--usage-warn, #b45309);
  }
  .occurrences {
    padding-left: 1.4rem;
  }
  .occ {
    font-size: 0.8rem;
    padding: 0.1rem 0.3rem;
    border: 1px solid transparent;
  }
  .occ .mark {
    width: 0.8rem;
    text-align: center;
    flex: 0 0 auto;
  }
  .occ--primary .mark {
    color: var(--accent);
  }
  .occ--suggested {
    border: 1px dashed var(--border);
    color: var(--fg-muted);
  }
  .occ--past,
  .occ--rejected {
    opacity: 0.6;
  }
  .occ.current {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    border-color: var(--accent);
  }
  .oname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ohost,
  .ended,
  .sr {
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .occ.more {
    color: var(--fg-muted);
  }
  .more-btn {
    margin: 0.15rem 0 0.15rem 0.6rem;
  }
  .state {
    padding: 0.4rem 0.2rem;
  }
  .archived-row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0 4px;
    padding: 4px 6px;
    border-top: 1px dashed var(--border);
    color: var(--fg-muted);
    font-size: var(--control-font);
  }
  .archived-row span {
    flex: 1;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }
  .empty p {
    margin: 0;
  }
  .empty strong {
    color: var(--fg);
    font-weight: 500;
  }
  .muted {
    color: var(--fg-muted);
  }
  .refresh-error {
    margin: 0.2rem 0.4rem;
    font-size: 0.75rem;
    color: var(--usage-warn, #b45309);
  }
  .error {
    color: var(--usage-crit, #c62828);
  }
  .error p {
    margin: 0 0 0.4rem;
  }
</style>
