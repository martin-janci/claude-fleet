<script lang="ts">
  import Skeleton from './states/Skeleton.svelte';
  import Icon from './kit/Icon.svelte';
  import { tablistKeys } from './tablist_keys';
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
  //
  // Layout (design 2026-09-29): **List** (the default) is `TaskList` — every
  // task by status, To do / Doing / Done, from its own read; **Grouped** is
  // the org → group tree below, unchanged. The header is the same for both.
  // **Board** is not a third layout here: it opens `WorkBoard` over the
  // terminal (it needs the width), and this sidebar keeps its layout.
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { workBoardOpen, workViewChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';
  import { isNewTaskChord, ownNewTaskChord } from './new_task';
  import { shortcutLabel } from './shortcuts';
  import NewTaskDialog from './NewTaskDialog.svelte';
  import WorkFiltersBar from './WorkFiltersBar.svelte';
  import { facetSentence, workFacets } from './filter_facets';
  import WorkReview from './WorkReview.svelte';
  import WorkMissions from './WorkMissions.svelte';
  import { missionOpenRequest } from './missions';
  import WorkPrs from './WorkPrs.svelte';
  import WorkRules from './WorkRules.svelte';
  import TaskList from './TaskList.svelte';
  import WorkTaskRow from './WorkTaskRow.svelte';
  import { formatCostMicros } from './sessions';
  import {
    buildSections,
    distributeTasks,
    filtersKey,
    isOccurrenceOf,
    mergeTasks,
    openTask,
    orgSectionKey,
    readErrorText,
    revealTaskRequest,
    sectionFilters,
    sectionKey,
    selectedTaskId,
    setExpanded,
    workExpanded,
    workReview,
    workTask,
    workTree,
    workTreeMeta,
    workTreeSessionIds,
    workViewFilters,
    workViewKey,
    workLayout,
    activeWorkViewId,
    normalizeFilters,
    parseSectionKey,
    type GroupSection,
    type OrgSection,
    type SectionState,
    type WorkTask,
    type WorkTaskLink,
    type WorkTreeGroup,
    type WorkTreePage,
    type WorkTreeSection,
    type WorkTreeSectionAsk,
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

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const chord = workViewChordLabel(isMac);
  const newTaskChord = shortcutLabel('work.new-task', isMac);

  let tab = $state<'tasks' | 'review' | 'missions' | 'prs'>('tasks');
  // A "Sent to a mission" chip in Control (redesign 9.3): the Missions tab,
  // where WorkMissions opens the mission.
  $effect(() => {
    if ($missionOpenRequest) tab = 'missions';
  });
  let page = $state.raw<WorkTreePage | null>(null);
  const archivedHidden = $derived(page?.archived_hidden ?? 0);
  const hiddenByFilters = $derived(page?.hidden_by_filters ?? 0);
  /** "Hidden by filters · Show": every filter off but the archived switch
   *  and the grouping (the saved view is left). */
  function showHidden() {
    activeWorkViewId.set(null);
    workViewFilters.update((f) => {
      const n = normalizeFilters(f);
      return normalizeFilters({ archived: n.archived, group_by: n.group_by });
    });
  }
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
  // The List layout's last read: the filter bar's orgs and trackers, and the
  // sessions it shows.
  let listPage = $state.raw<WorkTreePage | null>(null);
  const listMode = $derived($workLayout === 'list');
  function onListPage(p: WorkTreePage) {
    listPage = p;
    workTreeMeta.set({ orgs: p.orgs, trackers: p.trackers, groups: p.groups });
  }

  const sections: OrgSection[] = $derived(page ? buildSections(page.groups, page.orgs, states) : []);
  // A blocked task names what it waits for by key when that task is loaded.
  const loadedById = $derived(new Map(sections.flatMap((o) => o.groups.flatMap((g) => g.tasks.map((t) => [t.task_id, t] as const)))));
  const taskById = (id: string) => loadedById.get(id);
  const selectedSessionId = $derived($selectedSession?.id ?? null);
  const expanded = $derived($workExpanded[$workViewKey] ?? {});

  /** "32bit · claude-fleet": the org, then the group (one section per org
   *  when grouped by organisation). */
  function sectionTitle(o: OrgSection, g: GroupSection): string {
    if (g.group.source === 'org') return o.name;
    const group = g.group.source === 'none' ? (g.group.label || 'No group') : g.group.label;
    return `${o.name} · ${group}`;
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
      hidden_by_filters: typeof v?.hidden_by_filters === 'number' ? v.hidden_by_filters : 0,
      next_cursor: v?.next_cursor ?? null,
      generated_at: v?.generated_at,
    };
  }

  /** The most tasks one `work_tree` read returns. */
  const PAGE_MAX = 200;

  // The sections a load asks for in the same read (the open ones, as many
  // tasks as each showed), so one refresh is one `work_tree` on a hub that
  // pages them; an older hub answers without `sections` and each open
  // section is read by itself, as before.
  function sectionAsks(sameView: boolean, had: ReadonlyMap<string, number>): WorkTreeSectionAsk[] {
    const openNow = get(workExpanded)[get(workViewKey)] ?? {};
    const keys = new Set<string>();
    for (const [k, open] of Object.entries(openNow)) if (open) keys.add(k);
    // A section the last page filled is open unless closed — but one that
    // page covered whole is not asked for again (the new first page covers
    // it too, or `load` reads it by itself when it grew past it).
    if (sameView)
      for (const [k, st] of states) {
        if (openNow[k] === false) continue;
        if (!st.own) {
          const g = page?.groups.find((x) => sectionKey(x.org_id, x.group.id) === k);
          if (g && st.tasks.length >= g.count) continue;
        }
        keys.add(k);
      }
    const asks: WorkTreeSectionAsk[] = [];
    for (const k of keys) {
      const at = parseSectionKey(k);
      if (!at) continue;
      const want = sameView ? Math.max(pageSize, had.get(k) ?? 0) : pageSize;
      // More than one read's worth pages by itself (`readSection`).
      if (want > PAGE_MAX) continue;
      asks.push({ org_id: at.orgId, group_id: at.groupId, limit: want });
      if (asks.length >= SECTIONS_MAX) break;
    }
    return asks;
  }

  /** The most sections one read pages (the hub's `TREE_MAX_SECTIONS`). */
  const SECTIONS_MAX = 100;

  let loadSeq = 0;
  /** Read the view. `full`: nothing kept is current (a stream gap), so no
   *  section keeps what it showed. `review`: bring the Review tab's count
   *  along (in the same read where the hub can). */
  async function load(opts: { full?: boolean; review?: boolean } = {}) {
    // List reads its own page (`TaskList`): no tree read, only the Review
    // count when asked.
    if (get(workLayout) === 'list') {
      ++loadSeq;
      loading = false;
      if (opts.review) void loadReviewCount();
      return;
    }
    const mine = ++loadSeq;
    const filters = get(workViewFilters);
    const fk = filtersKey(filters);
    // What each section had, so a refresh re-reads as much as was shown.
    const had = new Map<string, number>();
    for (const [k, s] of states) if (s.own) had.set(k, s.tasks.length);
    const sameBefore = !opts.full && pageFiltersKey === fk && fk === lastFiltersKey;
    const asks = sectionAsks(sameBefore, had);
    loading = true;
    const r = await workTree({ filters, limit: pageSize, sections: asks, with_review_total: opts.review });
    if (mine !== loadSeq) return;
    loading = false;
    if (!r.ok) {
      if (page && pageFiltersKey === fk) {
        refreshError = r.error;
      } else {
        error = r.error;
        refreshError = null;
      }
      if (opts.review) void loadReviewCount();
      return;
    }
    error = null;
    refreshError = null;
    // A refresh of the view shown (same filters) keeps each open section it
    // re-reads below until that read answers, so a failed or slow re-read
    // never blanks what was loaded.
    const sameView = !opts.full && pageFiltersKey === fk && fk === lastFiltersKey;
    pageFiltersKey = fk;
    const p = pageOf(r.value);
    page = p;
    if (opts.review) {
      if (typeof r.value?.review_total === 'number') reviewTotal = r.value.review_total;
      else void loadReviewCount();
    }
    // The sections this read paged (a hub that pages them answers every one
    // asked for).
    const paged = new Map<string, WorkTreeSection>();
    if (asks.length > 0 && Array.isArray(r.value?.sections))
      for (const sec of r.value.sections) paged.set(sectionKey(sec.org_id, sec.group_id), sec);
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
    // Open sections the first page did not cover: from this read when it
    // paged them, else each by itself.
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
      const sec = paged.get(k);
      if (sec && Array.isArray(sec.tasks)) {
        applySection(k, sec.tasks, sec.next_cursor ?? null, false);
        continue;
      }
      const want = sameView ? Math.max(pageSize, had.get(k) ?? 0) : pageSize;
      void loadSection(g, false, want);
    }
    loadedOnce = true;
    flushReveal();
  }

  /** A section's read answered: from its start it replaces what the
   *  section had loaded by itself (a task that left it goes), and the first
   *  page's share is merged; `more` appends. */
  function applySection(k: string, got: WorkTask[], cursor: string | null, more: boolean) {
    const cur = states.get(k);
    const tasks = more || (cur && !cur.own) ? mergeTasks(cur?.tasks ?? [], got) : got;
    states = new Map(states).set(k, { tasks, cursor, own: true });
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
    applySection(k, p.tasks, p.next_cursor ?? null, more);
  }

  let reviewSeq = 0;
  async function loadReviewCount() {
    // Only the newest count lands (review r07).
    const mine = ++reviewSeq;
    const r = await workReview({ limit: 1 });
    if (mine !== reviewSeq) return;
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

  // List ↔ Grouped: Grouped reads the tree (List reads its own).
  let lastLayout = get(workLayout);
  const offLayout = workLayout.subscribe((v) => {
    if (v === lastLayout) return;
    lastLayout = v;
    if (v === 'grouped') void load();
  });

  // `work:changed` / session events: one debounced re-read — at most
  // `maxWaitMs` after the first change it waits for, so a steady stream of
  // changes (a busy fleet) cannot hold the view back forever. While Review
  // shows, the tree is not on screen: only its count is read (Review reads
  // its own list), and the tree once when Tasks is back. A `resync` (a gap
  // in the hub's stream) reloads the whole view.
  let staleWhileHidden = false;
  let staleFull = false;
  const offChanged = onWorkChangedDebounced(
    (kinds) => {
      const full = kinds.has('resync');
      if (tab === 'review') {
        staleWhileHidden = true;
        staleFull ||= full;
        void loadReviewCount();
        return;
      }
      void load({ full, review: true });
    },
    () => debounceMs,
    () => maxWaitMs,
  );

  // The tabs (step 3.10). The board is a destination of the
  // right column (`workBoardOpen`), so its tab reads that store; any other tab
  // leaves it.
  const shownTab = $derived<'tasks' | 'missions' | 'board' | 'prs'>(
    $workBoardOpen ? 'board' : tab === 'missions' || tab === 'prs' ? tab : 'tasks',
  );
  function pickTab(t: 'tasks' | 'review' | 'missions' | 'board' | 'prs') {
    if (t === 'board') {
      workBoardOpen.set(true);
      return;
    }
    workBoardOpen.set(false);
    if (t === 'tasks') showTasks();
    else tab = t;
  }

  function showTasks() {
    tab = 'tasks';
    if (!staleWhileHidden) return;
    const full = staleFull;
    staleWhileHidden = false;
    staleFull = false;
    void load({ full });
  }

  // ⌘N makes a task while Work is open (G2.1; the registry's `work`
  // scope): the quick switcher stands aside while this view owns it.
  let newTaskOpen = $state(false);
  const releaseNewTask = ownNewTaskChord();
  function onNewTaskChord(e: KeyboardEvent) {
    if (!isNewTaskChord(e, isMac)) return;
    // Another dialog owns the keyboard while open.
    if ((e.target as Element | null)?.closest?.('dialog')) return;
    e.preventDefault();
    e.stopPropagation();
    newTaskOpen = true;
  }

  onMount(() => {
    void load({ review: true });
    // Capture phase, as the switcher's: beat the terminal to the chord.
    window.addEventListener('keydown', onNewTaskChord, true);
  });
  onDestroy(() => {
    window.removeEventListener('keydown', onNewTaskChord, true);
    releaseNewTask();
    offFilters();
    offLayout();
    offChanged();
    offReveal();
    workTreeSessionIds.set(new Set());
  });

  // The sessions the tree shows: their status changes refresh it too.
  $effect(() => {
    const ids = new Set<number>();
    const shown = listMode ? [listPage?.tasks ?? []] : [...states.values()].map((s) => s.tasks);
    for (const tasks of shown)
      for (const t of tasks) for (const l of t.sessions ?? []) if (l.session_id != null) ids.add(l.session_id);
    workTreeSessionIds.set(ids);
  });

  // The people and tracker columns of the tasks loaded, for the filter
  // bar's Assignee picker and Tracker column chips (redesign step 6.2).
  function namesOf(pick: (t: WorkTask) => readonly (string | null | undefined)[]): string[] {
    const seen = new Map<string, string>();
    const lists = [page?.tasks ?? [], listPage?.tasks ?? [], ...[...states.values()].map((x) => x.tasks)];
    for (const tasks of lists)
      for (const t of tasks)
        for (const n of pick(t)) {
          const v = n?.trim();
          if (v && !seen.has(v.toLowerCase())) seen.set(v.toLowerCase(), v);
        }
    return [...seen.values()].sort((a, b) => a.localeCompare(b));
  }
  const people = $derived(namesOf((t) => t.assignees ?? []));
  const columns = $derived(namesOf((t) => [t.status_name]));

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
    openTask(t.task_id, t.sessions);
  }

  // Opening an occurrence opens its session — the same `session_id` from
  // every task it is under. A past link names no live session: the task's
  // detail shows it instead.
  function openOccurrence(t: WorkTask, l: WorkTaskLink) {
    const row =
      l.session_id != null && l.state !== 'ended' ? $sessions.find((r) => r.id === l.session_id) : undefined;
    if (row) selectSessionExplicitly(row, { task: t.task_id });
    else openTask(t.task_id, t.sessions);
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
    if (!req) return;
    // The task is revealed in the tree: Grouped reads it, and its load
    // flushes this again.
    if (get(workLayout) !== 'grouped') {
      workLayout.set('grouped');
      return;
    }
    if (!loadedOnce) return;
    revealTaskRequest.set(null);
    showTasks();
    void reveal(req.taskId);
  }
  const offReveal = revealTaskRequest.subscribe((req) => {
    if (req) flushReveal();
  });

</script>

<div class="work-tree" data-testid="work-tree" aria-busy={loading} bind:this={root}>
  <header class="work-header">
    <div class="row">
        <!-- The fixed Work tabs (redesign step 3.10): Tasks, Missions, Board
             and Pull requests, with the links waiting for review as a count
             after them (a tablist holds only tabs). Review is part of Tasks;
             the board is a Work view of its own, not an overlay toggled from
             the layout chips. -->
        <div class="tabs seg" role="tablist" aria-label="Work view" use:tablistKeys>
          <button
            class="seg-tab"
            role="tab"
            aria-selected={shownTab === 'tasks'}
            class:is-active={shownTab === 'tasks'}
            data-testid="work-tab-tasks"
            onclick={() => pickTab('tasks')}>Tasks</button
          >
          <button
            class="seg-tab"
            role="tab"
            aria-selected={shownTab === 'missions'}
            class:is-active={shownTab === 'missions'}
            data-testid="work-tab-missions"
            onclick={() => pickTab('missions')}>Missions</button
          >
          <button
            class="seg-tab"
            role="tab"
            aria-selected={shownTab === 'board'}
            class:is-active={shownTab === 'board'}
            data-testid="work-tab-board"
            title="These tasks on a board by status"
            onclick={() => pickTab('board')}>Board</button
          >
          <button
            class="seg-tab"
            role="tab"
            aria-selected={shownTab === 'prs'}
            class:is-active={shownTab === 'prs'}
            data-testid="work-tab-prs"
            onclick={() => pickTab('prs')}>Pull requests</button
          >
        </div>
        {#if reviewTotal}
          <button
            class="review-count"
            type="button"
            aria-pressed={shownTab === 'tasks' && tab === 'review'}
            class:is-active={shownTab === 'tasks' && tab === 'review'}
            title="{reviewTotal} {reviewTotal === 1 ? 'link waits' : 'links wait'} for review"
            aria-label="Review: {reviewTotal} waiting"
            data-testid="work-review-count"
            onclick={() => pickTab('review')}>{reviewTotal}</button
          >
        {/if}
      <button
        class="btn btn--quiet btn--icon"
        type="button"
        title="New task ({newTaskChord})"
        aria-label="New task"
        data-testid="work-new-task"
        onclick={() => (newTaskOpen = true)}>+</button
      >
      <button
        class="btn btn--quiet btn--icon"
        type="button"
        title="Placement rules"
        aria-label="Placement rules"
        data-testid="work-rules-open"
        onclick={() => (rulesOpen = true)}><Icon name="settings" size={14} /></button
      >
      <!-- Refresh is the sidebar's ↻ (it re-reads this view too), and
           collapse is the sidebar's ‹: one of each. -->
    </div>
    {#if tab === 'tasks'}
      {@const meta = listMode ? listPage : page}
      <WorkFiltersBar orgs={meta?.orgs ?? []} trackers={meta?.trackers ?? []} listLayout={listMode} {people} {columns} />
    {/if}
  </header>

  <div class="scroller">
    {#if tab === 'tasks' && !listMode && refreshError && page && !error}
      <p class="refresh-error" role="status" data-testid="work-tree-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="work-tree-refresh-retry" onclick={() => void load()}>Retry</button>
      </p>
    {/if}
    {#if tab === 'review' && !$workBoardOpen}
      <div class="review-strip" data-testid="work-review-strip">
        <span>Review: {reviewTotal ?? 0} waiting</span>
        <button
          class="btn btn--quiet btn--icon"
          type="button"
          title="Back to the tasks"
          aria-label="Close review"
          data-testid="work-review-strip-close"
          onclick={() => pickTab('tasks')}>×</button
        >
      </div>
    {/if}
    {#if tab === 'review'}
      <WorkReview onchanged={() => void loadReviewCount()} />
    {:else if tab === 'missions'}
      <WorkMissions />
    {:else if tab === 'prs'}
      <WorkPrs />
    {:else if listMode}
      <TaskList {debounceMs} {maxWaitMs} onpage={onListPage} />
    {:else if error}
      <div class="state error" role="alert" data-testid="work-tree-error">
        <p>{readErrorText(error)}</p>
        <button class="btn" type="button" data-testid="work-tree-retry" onclick={() => void load()}>Retry</button>
      </div>
    {:else if !page}
      <div class="state" data-testid="work-tree-loading"><Skeleton rows={4} label="Loading work" /></div>
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
      <!-- Board "Work · tasks with filters open": one flat list of
           sections, each named by its org and its group, and a row per
           task (WorkTaskRow). -->
      <ul class="groups" aria-label="Work">
        {#each sections as o (o.key)}
          {#each o.groups as g (g.key)}
            <li class="group" data-testid="work-group" data-group-id={g.group.id} data-org-id={o.orgId ?? 'none'}>
              <button
                class="of-sec group-head"
                type="button"
                data-testid="work-group-head"
                aria-expanded={groupOpen(g)}
                title={g.group.source === 'none' || g.group.source === 'org' ? undefined : `Grouped by ${g.group.source}`}
                onclick={() => toggleGroup(g)}
              >
                <span class="caret" class:open={groupOpen(g)} aria-hidden="true">▸</span>
                {#if o.color}<span class="org-dot" style="background: {o.color}" aria-hidden="true"></span>{/if}
                <span class="group-name">{sectionTitle(o, g)}</span>
                {#if g.cost > 0}<span class="spend" data-testid="work-group-spend" title="Spend of its tasks">{formatCostMicros(g.cost)}</span>{/if}
                <span class="of-count" data-testid="work-group-count">{g.count}</span>
              </button>
              {#if groupOpen(g)}
                <ul class="tasks">
                  {#each g.tasks as t (t.task_id)}
                    <li
                      class="task"
                      class:selected={$selectedTaskId === t.task_id}
                      class:lit={(t.sessions ?? []).some((l) => isOccurrenceOf(l, selectedSessionId))}
                      data-testid="work-task"
                      data-task-id={t.task_id}
                    >
                      <WorkTaskRow
                        task={t}
                        selected={$selectedTaskId === t.task_id}
                        currentSessionId={selectedSessionId}
                        lookup={taskById}
                        onselect={() => selectTask(t)}
                        onopen={(l) => openOccurrence(t, l)}
                      />
                    </li>
                  {/each}
                  {#if sectionBusy.has(g.key)}
                    <li class="state" data-testid="work-section-loading"><Skeleton rows={1} label="Loading" /></li>
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
        {/each}
      </ul>
      {#if hiddenByFilters > 0}
        <button class="of-btn quiet hidden-row" type="button" data-testid="work-hidden-by-filters" onclick={showHidden}
          ><span>Hidden by filters <span class="of-count">{hiddenByFilters}</span></span><span>Show</span></button
        >
      {/if}
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

{#if newTaskOpen}
  <NewTaskDialog onclose={() => (newTaskOpen = false)} />
{/if}
{#if rulesOpen}
  <WorkRules onclose={() => (rulesOpen = false)} />
{/if}

<style>
  .review-count {
    min-width: 18px;
    height: 16px;
    padding: 0 5px;
    border: none;
    border-radius: var(--radius-sm);
    background: var(--waiting-soft);
    color: var(--status-waiting);
    font: inherit;
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    cursor: pointer;
    align-self: center;
    margin-right: auto;
  }
  .review-count.is-active {
    outline: 1px solid var(--status-waiting);
  }
  .review-strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.25rem 0.5rem;
    margin: 0.25rem 0.5rem;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    font-size: var(--text-2xs);
  }
  .work-tree {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 0;
    font-size: var(--text-xs);
  }
  .work-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 4px 12px 8px;
    background: var(--bg-pane);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  /* Board: one segmented control for the four Work views. */
  .seg {
    display: flex;
    gap: 2px;
    padding: 2px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
    flex: 0 1 auto;
    min-width: 0;
  }
  .seg-tab {
    height: 22px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-xs);
    white-space: nowrap;
    cursor: pointer;
  }
  .seg-tab:hover {
    color: var(--fg);
  }
  .seg-tab[aria-selected='true'] {
    background: var(--bg-hover);
    color: var(--fg);
    font-weight: 500;
  }
  .seg-tab:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -1px;
  }
  .row:not(:has(.review-count)) .seg {
    margin-right: auto;
  }
  .scroller {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
    padding: 0 0 8px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .group-head {
    width: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-2xs);
    text-align: left;
    cursor: pointer;
  }
  .group-head:hover {
    color: var(--fg);
  }
  .group-head:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .group-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caret {
    font-size: var(--text-2xs);
    width: 8px;
    transition: transform var(--dur-fast) ease;
  }
  .caret.open {
    transform: rotate(90deg);
  }
  .org-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
  }
  .spend {
    margin-left: auto;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
  }
  .spend + .of-count {
    margin-left: var(--space-1);
  }
  .more-btn {
    margin: 2px 0 2px 18px;
  }
  .hidden-row {
    width: calc(100% - 12px);
    margin: 8px 6px 0;
    justify-content: space-between;
  }
  .state {
    padding: 6px 12px;
  }
  .archived-row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 6px 4px;
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
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .error {
    color: var(--usage-crit);
  }
  .error p {
    margin: 0 0 0.4rem;
  }
</style>
