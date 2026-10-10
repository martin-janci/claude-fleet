<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import { tick, untrack, type Snippet } from 'svelte';
  import { get } from 'svelte/store';
  import { projects, refreshProjects, type ProjectTreeRow } from './projects';
  import {
    sessions,
    loadSessions,
    killSession,
    recreateSession,
    restartSession,
    purgeProject,
    showBgAgents,
    sidebarGroupBy,
    sameSession,
    hasNoPane,
    sessionAgent,
    type SessionRow,
  } from './sessions';
  import { describePurge, purgeHostsForProject } from './purge';
  import { capRows, groupRows, isFlatGroupBy, moreRunningText } from './row_groups';
  import { startingSessions } from './session_starting';
  import { tablistKeys } from './tablist_keys';
  import {
    agentFilter,
    agentFilterLabel,
    inScopeTab,
    isSharedAccess,
    scopeTab,
    scopeTabCounts,
    sharedByLine,
    SCOPE_TAB_LABELS,
    SCOPE_TABS,
  } from './session_scope';
  import {
    inboxGroupBy,
    inboxRows,
    inboxSections,
    notWaiting,
    notWaitingSaid,
    notWaitingText,
    proposedRows,
    proposedText,
    sayNotWaiting,
  } from './inbox';
  import RoutineFailures from './automation/RoutineFailures.svelte';
  import MissionWaits from './MissionWaits.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { failingCount, loadFailing } from './routines';
  import { loadWaitingMissions, waitingMissionCount } from './mission_waits';
  import { sessionMatchesSearch } from './search';
  import { sessionFocus } from './session_focus';
  import { type ProjectRow } from './projects';
  import { selectedSession, selectSession, selectSessionExplicitly, revealSeq } from './selection';
  import { applySessionRename, renameKeyHandler } from './session_rename';
  import { readPref, writePref } from './prefs';
  import { accessOf, backendMode } from './access';
  import AddProjectDialog from './AddProjectDialog.svelte';
  import { hostFilter, effectiveHostFilter, hosts, hostByAlias } from './hosts';
  import { bootstrapError } from './bootstrap_state';
  import { errorText } from './error_copy';
  import EmptyState from './states/EmptyState.svelte';
  import HostOffline from './states/HostOffline.svelte';
  import LoadError from './states/LoadError.svelte';
  import { openSettingsAt } from './app_views';
  import { openToday } from './control';
  import type { IpcError } from './result';
  import { bulkTargets, sessionBlocked } from './share';
  import { moveToAccount } from './account_limits';
  import {
    effectiveScope,
    scopeFilter,
    scopeOf,
    scopes,
    scopeSelectorShown,
    orgColorById,
    orgColorOf,
    projectOwners,
    orgs as orgList,
    UNASSIGNED,
  } from './orgs';
  import { facetSentence, sessionFacets } from './filter_facets';
  import {
    hostsViewOpen,
    addProjectRequest,
    requestHostsView,
  } from './app_views';
  import { hintAnchor } from './hints';
  import { openNewSessionPicker } from './switcher_request';
  import { requestNewSession } from './new_session_request';
  import { foldedIds, lostFolds } from './lost_fold';
  import LostFoldRow from './LostFoldRow.svelte';
  import { setProjectPick } from './project_picks';
  import { detectMac, isEditable } from './terminal_keys';
  import { matchShortcut } from './shortcuts';
  import {
    buildSessionsByProject,
    buildOutsideFleet,
    buildRelatedCountById,
    buildSessionsByWork,
    sessionVisible,
    rowMatches,
    sortProjectsBySeverity,
    sortWorkGroups,
    type FilterRow,
    type SessionPredicate,
    type WorkGroup,
  } from './sidebar_index';
  import { workGroupPrSummary, workGroupTicket, workKeyFor, worktreeBranchById } from './work_keys';
  import { statusDotClass, trackers, unavailableLabel } from './trackers';
  import {
    bothPredicates,
    effectiveWorkFilters,
    loadMine,
    mineItemIds,
    mineLoaded,
    withMineReady,
    DEFAULT_WORK_FILTERS,
    pastWorkFields,
    statusNamesOf,
    toRowFilters,
    workFilterPredicate,
    type WorkFilters,
    workFilters,
  } from './work_filters';
  import {
    bucketState,
    ciStatusColor,
    ciStatusLabel,
    countNeedsYou,
    countsTowardBadge,
    needsYou,
    severity,
    worstSeverityByProject,
    type TriageBucket,
  } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { attentionFacts } from './attention_facts';
  import { snapshotRows } from './motion_catalog';
  import { push, pushError } from './toasts';
  import { hubStatus, hubBlock, hubActionBlocked } from './hub';
  import { hubConnection, connectionBanner } from './hub_connection';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import KillDialog from './KillDialog.svelte';
  import { announceArchive, archiveBlocked, archiveSessions } from './kill_check';
  import BulkPromptDialog from './BulkPromptDialog.svelte';
  import NameWorkDialog from './NameWorkDialog.svelte';
  import SidebarFilters from './SidebarFilters.svelte';
  import SessionRowItem from './SessionRowItem.svelte';
  import ResumeButton from './ResumeButton.svelte';
  import SummarizeButton from './SummarizeButton.svelte';
  import {
    reopenedBadge,
    reopenedByKey,
    reopenedWork,
    splitArchived,
    unarchiveSession,
  } from './tidy';
  import {
    loadPastWork,
    pastWork,
    pastWorkSummary,
    workPurgeImpact,
    type WorkLink,
  } from './work';
  import { timeAgo } from './session_status';
  import NewBgSessionDialog from './NewBgSessionDialog.svelte';
  import WorkTree from './WorkTree.svelte';
  import { sidebarView } from './work_view';
  import { isRecency, withinRecency, type Recency } from './session_status';

  // Optional collapse handler injected by the parent (App.svelte). When
  // present, a ‹ button appears in the sidebar header so the user can
  // hide the whole sidebar to make room for the terminal.
  let { onCollapse }: { onCollapse?: () => void } = $props();

  let loadError: string | null = $state(null);
  /** The last refresh's failure, kept whole for the list's error state. */
  let refreshError: IpcError | null = $state(null);
  let loading = $state(false);
  // Recency filter persists across app restarts. Default to "all" the first
  // time the user opens the app; otherwise honor whatever pill they last
  // clicked. The setter writes back to localStorage on every change.
  let recency: Recency = $state(readPref('recency', 'all' as Recency, isRecency));
  $effect(() => {
    writePref('recency', recency);
  });
  const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
  // "Outside fleet" (interactive Claude sessions running entirely outside
  // tmux) is collapsed by default — most users never need it — and its
  // open/closed state persists across restarts like the other section
  // toggles in this file.
  let outsideOpen = $state(readPref('outside-fleet-open', false, isBool));
  $effect(() => {
    writePref('outside-fleet-open', outsideOpen);
  });
  let search = $state('');
  // `filtered` re-derives the whole project tree on its dependencies; debounce
  // the search term so each keystroke doesn't trigger a full re-derive +
  // re-render. The input stays bound to `search` for instant feedback.
  let searchQuery = $state('');
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const q = search;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchQuery = q;
    }, 150);
    return () => clearTimeout(searchTimer);
  });

  // Per-session UI state. Kept here instead of on each row so collapse and
  // rename state survive a sessions store refresh that creates new row
  // objects. The row being renamed is pinned by its full identity (id +
  // host + old name) — a bare tmux_name is ambiguous across hosts, since
  // default names are project-derived and the same name on two hosts is
  // the normal case.
  //
  // `mode` picks what the inline editor changes: double-click edits the
  // display label (`friendly_name`, empty clears it); the row's ✎ action
  // renames the tmux session itself. `original` is the value the editor
  // opened with, so an unchanged commit is a no-op.
  let renaming: {
    id: number;
    host_alias: string;
    tmux_name: string;
    mode: 'label' | 'tmux';
    original: string;
  } | null = $state(null);
  let renameValue = $state('');
  let renameError: string | null = $state(null);
  // The live rename <input> (only one renders at a time). Bound directly so
  // focus targets the right element — a `data-testid` querySelector would
  // pick the first match if a tree row and an orphan row shared a name.
  let renameInput: HTMLInputElement | undefined = $state();
  // Synchronous in-flight guard: commitRename is wired to BOTH Enter and
  // onblur, and Enter blurs the input — without this the rename IPC fires
  // twice (the `!renamingName` check doesn't help: it's still set during the
  // first call's await).
  let committingRename = false;
  let pendingKill: SessionRow | null = $state(null);
  let pendingRecreate: SessionRow | null = $state(null);
  let pendingRestart: SessionRow | null = $state(null);

  // ── Triage (FE-3 / FE-4, now P13 / T1) ──
  // One ranked queue. The "Needs you" pill counts and filters the rows that
  // rank() places in a needs-you bucket: waiting on you, stuck, failed,
  // done-unread, broken lifecycle, or idle > N min. Session-scoped (not
  // persisted): a filter that hides healthy sessions should not survive a
  // restart unnoticed.
  let needsYouOnly = $state(false);
  // Coarse clock for the idle rule; a 30 s tick is plenty for a minutes-level
  // threshold and keeps the derived tree from re-running every second.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const attentionOpts = $derived({ idleSecs: $attentionIdleMinutes * 60, now: nowSec, facts: $attentionFacts });
  // The org scope (work graph M5): a view filter composed into every
  // builder below through `rowMatches`. `null` while no scope is chosen or
  // the selector is hidden (fewer than two scopes).
  const scopeSel = $derived(
    $effectiveScope === 'all' ? null : { id: $effectiveScope, of: $scopeOf },
  );
  // A clicked suggestion narrows the tree to its one session (see
  // session_focus.ts): past every other filter, so the row can't be hidden
  // by the host, bg-agent, scope, recency or search filters it arrived under
  // — nor by a work group's collapsed Done section when its link is archived.
  const focus = $derived($sessionFocus);
  // A focus asks for one session in the Sessions list: from the Work view
  // (LinkReview / TidyReview render in both), go there to show it.
  $effect(() => {
    if (focus) untrack(() => sidebarView.set('sessions'));
  });
  const viewHost = $derived(focus ? 'all' : $effectiveHostFilter);
  const viewBg = $derived(focus ? true : $showBgAgents);
  const viewScope = $derived(focus ? null : scopeSel);
  const viewSearch = $derived(focus ? '' : searchQuery);
  // The work filters (M10.4): tracker, status, mine, has-session, archived
  // — persisted chips in SidebarFilters, applied through `rowMatches` and
  // composed with needs-you. A focused suggestion is past them too.
  const workFilterView = $derived(
    withMineReady(
      effectiveWorkFilters($workFilters, $trackers, $sidebarGroupBy === 'work', statusNamesOf($sessions)),
      $mineLoaded,
    ),
  );
  const workFilterCtx = $derived({ trackers: $trackers, mine: $mineItemIds });
  const workPredicate = $derived(workFilterPredicate(workFilterView, workFilterCtx));
  // "Mine" reads the hub's `mine` view; re-read while the chip is on.
  const mineOn = $derived($workFilters.assignee === 'mine');
  $effect(() => {
    if (!mineOn) return;
    void $trackers.length;
    untrack(() => void loadMine());
    const t = setInterval(() => void loadMine(), 120_000);
    return () => clearInterval(t);
  });
  /** A past link passes the work filters `wf` (host and scope are the
   *  caller's). The list passes its own; the archived count, the same with
   *  archived shown. */
  function pastPassesWorkFilters(key: string, l: WorkLink, wf: WorkFilters = workFilterView): boolean {
    return rowMatches(
      { ...pastFilterRow(l), ...pastWorkFields(key, l.item_id, workFilterCtx) },
      toRowFilters(wf),
    );
  }
  /** A past link shows: its host and scope are in view, and it passes the
   *  work filters. One rule for a past-only group and a live group's Done,
   *  which used to disagree on host and scope. */
  function pastVisible(key: string, l: WorkLink, wf: WorkFilters = workFilterView): boolean {
    return (
      withinRecency(l.ended_at, recency, nowSec) &&
      rowMatches(pastFilterRow(l), { host: $effectiveHostFilter, scope: $effectiveScope }) &&
      pastPassesWorkFilters(key, l, wf)
    );
  }
  /** A past-only group matches a search by its key or a link's name. */
  function pastGroupMatchesSearch(key: string, links: WorkLink[], q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    return key.toLowerCase().includes(needle) || links.some((l) => (l.snap_name ?? '').toLowerCase().includes(needle));
  }
  /** The triage predicate (needs-you, recency, then `work` — the work
   *  filters), with no focus: the list's own, and the archived count's with
   *  archived shown. */
  function triagePredicate(work: SessionPredicate): SessionPredicate {
    const opts = attentionOpts;
    // Last active: by each session's own activity, in every section (it
    // used to weigh only a project's newest session, and only in the tree).
    const r = recency;
    const recent: SessionPredicate = r === 'all' ? null : (s) => withinRecency(s.last_activity_at, r, opts.now);
    // "Agent: any" (Sessions board): only the rows that run the chosen agent.
    const ag = $agentFilter;
    const agent: SessionPredicate = ag === 'any' ? null : (s) => sessionAgent(s) === ag;
    return bothPredicates(
      bothPredicates(bothPredicates(needsYouOnly ? (s) => needsYou(s, opts) : null, recent), agent),
      work,
    );
  }
  const rowBase = $derived.by((): SessionPredicate => {
    if (focus) {
      const id = focus.id;
      return (s) => s.id === id;
    }
    // A mass loss's rows live in their fold row (redesign 1.1), not the tree.
    const folded = foldedIdSet;
    const unfolded: SessionPredicate = folded.size === 0 ? null : (s) => !folded.has(s.id);
    return bothPredicates(unfolded, triagePredicate(workPredicate));
  });
  // ── Shared with me (redesign step 5.8) ──
  // The sessions someone shared with this person (at any level) leave the
  // tree and the groups for one group of their own, except in focus. An
  // unknown access (null) is not a share and stays put.
  const splitShared = $derived(!focus);
  function isSharedWithMe(s: SessionRow): boolean {
    return isSharedAccess($accessOf(s));
  }
  // ── All / Mine / Shared with me (Sessions board) ──
  // Counted over what All would show; the tabs show on a fleet with people
  // (a window onto a hub) or once something is shared, since a standalone
  // desktop owns every row and the tabs would only repeat All.
  const tabCounts = $derived.by(() => {
    const q = viewSearch.toLowerCase();
    const pool = $sessions.filter(
      (s) => s.kind !== 'external' && sessionVisible(s, viewHost, viewBg, rowBase, viewScope) && sessionMatchesSearch(s, q),
    );
    return scopeTabCounts(pool, $accessOf);
  });
  const tabsShown = $derived(!focus && (backendMode($hubStatus) !== 'local' || tabCounts.shared > 0));
  const tab = $derived(tabsShown ? $scopeTab : 'all');
  const rowPredicate = $derived.by((): SessionPredicate => {
    if (!splitShared) return rowBase;
    // Shared with me: the tree and the groups hold nothing; the shared
    // group below is the list.
    if (tab === 'shared') return () => false;
    const mine = tab === 'mine';
    return bothPredicates((s) => (mine ? inScopeTab('mine', $accessOf(s)) : !isSharedWithMe(s)), rowBase);
  });

  // What narrows the list, for the empty state (the chrome shows the same
  // facets as chips).
  // ── Archived (hidden by default) ──
  // Archived live sessions and past work stay out of the list until asked
  // for; the list says how many it holds back under the other filters, and
  // brings them all back in one click.
  // Counted the way the list matches a search: a work group by its key, a
  // project by owner / repo, a past-only group by its key or a link's name
  // (any of which then shows every row in it), else the session itself.
  // Matching each archived row on its own used to disagree with the list.
  // Cheap gate: most of the time nothing is archived, and the count below
  // (a second grouping pass) is skipped.
  const anyArchivedSession = $derived($sessions.some((s) => s.work?.archived_at != null));
  const archivedHidden = $derived.by((): number => {
    if (focus || workFilterView.archived) return 0;
    const workMode = $sidebarGroupBy === 'work';
    // Past links always count as archived (see `pastFilterRow`); needs-you
    // lists live sessions only.
    const pastCounts = workMode && !needsYouOnly && [...$pastWork.values()].some((l) => l.length > 0);
    if (!anyArchivedSession && !pastCounts) return 0;
    const shown: WorkFilters = { ...workFilterView, archived: true };
    const shownPred = triagePredicate(workFilterPredicate(shown, workFilterCtx));
    const q = searchQuery.toLowerCase();
    const isArchived = (s: SessionRow) => s.work?.archived_at != null;
    const byWork = workMode
      ? buildSessionsByWork($sessions, viewHost, viewBg, shownPred, (s) => workKeyFor(s, branchById), viewScope)
      : null;
    let n = 0;
    const liveKeys = new Set<string>();
    const matchedKeys = new Set<string>();
    for (const g of byWork?.groups ?? []) {
      liveKeys.add(g.key);
      if (!workGroupMatchesSearch(g, q)) continue;
      matchedKeys.add(g.key);
      n += g.sessions.filter(isArchived).length;
    }
    const rest = untakenBy(byWork?.keyed ?? null, shownPred);
    const byProject = buildSessionsByProject($sessions, viewHost, viewBg, rest, viewScope);
    for (const p of $projects) {
      const rows = byProject.get(p.project.id) ?? [];
      const archived = rows.filter(isArchived).length;
      if (archived > 0 && matchesSearch(p, q, rows)) n += archived;
    }
    n += orphansOf(rest).filter(isArchived).length;
    if (pastCounts) {
      for (const [key, links] of $pastWork) {
        const past = links.filter((l) => pastVisible(key, l, shown));
        if (past.length === 0) continue;
        // A live group's past work sits in its Done row, shown when the
        // group matches; a past-only group matches on its own.
        const hit = liveKeys.has(key) ? matchedKeys.has(key) : pastGroupMatchesSearch(key, past, q);
        if (hit) n += past.length;
      }
    }
    return n;
  });
  function setShowArchived(on: boolean) {
    workFilters.update((f) => ({ ...f, archived: on }));
  }

  const listFacets = $derived(
    focus
      ? []
      : sessionFacets({
          scope: $scopeSelectorShown ? $effectiveScope : 'all',
          scopeLabel:
            $effectiveScope === UNASSIGNED ? 'Unassigned' : $scopes.find((x) => x.id === $effectiveScope)?.label,
          host: $effectiveHostFilter,
          agent: $agentFilter === 'any' ? undefined : agentFilterLabel($agentFilter),
          recency,
          search: searchQuery,
          needsYou: needsYouOnly,
          showBgAgents: $showBgAgents,
          work: workFilterView,
          trackerName: (id) => $trackers.find((t) => t.id === id)?.name,
        }),
  );
  function clearListFilters() {
    hostFilter.set('all');
    scopeFilter.set('all');
    agentFilter.set('any');
    recency = 'all';
    search = '';
    searchQuery = '';
    needsYouOnly = false;
    showBgAgents.set(true);
    workFilters.set({ ...DEFAULT_WORK_FILTERS });
  }

  // Multi-select for bulk Kill / Send prompt. Rows are toggled with
  // shift/cmd/ctrl-click, or with the checkboxes once select mode is on.
  let selectMode = $state(false);
  let selectedIds: Set<number> = $state(new Set());
  let bulkKillOpen = $state(false);
  /** Which bulk press opened the Kill dialog: Kill or Clean up (step 1.7). */
  let bulkKillMode = $state<'kill' | 'cleanup'>('kill');
  let bulkPromptOpen = $state(false);
  const selectedRows = $derived($sessions.filter((s) => selectedIds.has(s.id)));

  function toggleSelected(sess: SessionRow) {
    // Outside-fleet rows are read-only: bulk kill / send would only fail.
    if (sess.kind === 'external') return;
    const next = new Set(selectedIds);
    if (next.has(sess.id)) next.delete(sess.id);
    else next.add(sess.id);
    selectedIds = next;
  }
  function clearSelected() {
    selectedIds = new Set();
  }
  function toggleSelectMode() {
    selectMode = !selectMode;
    if (!selectMode) clearSelected();
  }
  // Drop ids a filter now hides: a bulk Kill / Send must act only on rows
  // the user can see (it used to reach rows a later filter had hidden).
  const visibleIds = $derived.by(() => {
    const ids = new Set<number>();
    for (const list of filteredSessionsByProject.values()) for (const s of list) ids.add(s.id);
    for (const g of workGroups) for (const s of g.sessions) ids.add(s.id);
    for (const s of orphanSessions) ids.add(s.id);
    return ids;
  });
  $effect(() => {
    const vis = visibleIds;
    const cur = untrack(() => selectedIds);
    if ([...cur].some((id) => !vis.has(id))) selectedIds = new Set([...cur].filter((id) => vis.has(id)));
  });
  // Drop ids whose rows left the store (killed / reaped) so the bulk bar
  // never counts phantoms.
  $effect(() => {
    const live = new Set($sessions.map((s) => s.id));
    if ([...selectedIds].some((id) => !live.has(id))) {
      selectedIds = new Set([...selectedIds].filter((id) => live.has(id)));
    }
  });

  /**
   * The per-host unclaimed counts (multi-user M1, rule 6) — a COUNT, with no
   * rows and no expand, which is the whole point: an unclaimed session is one
   * fleet did not start, and spec §4.3 says the only thing anyone out of its
   * scope may learn about it is a number. Rendering rows here would be exactly
   * the metadata leak the number exists instead of.
   *
   * Deliberately NOT from `hosts_view.ts::sessionCounts`, which derives every
   * other host badge from the rows this client holds — there are no rows to
   * derive this from, by design. It comes from `HostRow.unclaimed_sessions`,
   * straight off the host row.
   *
   * A host is skipped when its count is absent or `null` — which is the NORMAL
   * case on a hub with more than one person (R5-d): the backend serves `null`
   * rather than a number there, because a zero is itself a claim about the host
   * that would let a second person infer one. Nothing is rendered for it: not
   * "0", not a dash. A genuine `0` is skipped too — true, but it describes
   * nothing, and `fleet-hub session unclaimed` is where a human reads the
   * counts anyway. The sidebar's host filter applies, like every other list
   * here.
   */
  const unclaimedByHost = $derived(
    $hosts
      .filter(
        (h) =>
          !h.hidden &&
          typeof h.unclaimed_sessions === 'number' &&
          h.unclaimed_sessions > 0 &&
          (viewHost === 'all' || viewHost === h.alias),
      )
      .map((h) => ({ alias: h.alias, count: h.unclaimed_sessions as number })),
  );
  const unclaimedTotal = $derived(unclaimedByHost.reduce((n, h) => n + h.count, 0));

  /**
   * The bulk fan-outs, narrowed to the rows this client may actually act on
   * (multi-user M1). Select mode already excludes outside-fleet rows as
   * read-only; a session shared with this person at `watch` — or at `drive`,
   * for Kill, which spec §4.3 puts in the owner-only tier — is the same kind
   * of row, and both filter through the SAME predicate as every single-row
   * button, so there is one rule and not a second copy of the level table.
   *
   * Narrowed here rather than in `toggleSelected`, because the two actions
   * need different levels: a `drive` row is a legitimate bulk-prompt target
   * and an illegitimate bulk-kill one, and a selection that refused it
   * outright would take the first away to prevent the second.
   */
  const bulkKillTargets = $derived(bulkTargets(selectedRows, 'kill_session', $sessionBlocked));
  const bulkPromptTargets = $derived(bulkTargets(selectedRows, 'send_prompt', $sessionBlocked));

  /** Clean up was accepted. A direct remove already dropped the row, so
   *  the pane stops attaching to it; a Safe remove finishes later through
   *  the agent and keeps it until then. */
  function cleanedUp(removed: SessionRow[]) {
    pendingKill = null;
    const cur = $selectedSession;
    if (cur && removed.some((r) => sameSession(cur, r))) selectSession(null);
  }

  /** Bulk Archive (step 1.7): the selected sessions with work go to their
   *  work's Done, with Undo. Nothing is killed. */
  async function bulkArchive() {
    const rows = bulkTargets(selectedRows, 'tidy_apply', $sessionBlocked);
    const r = await archiveSessions(rows);
    if (!r.ok) {
      pushError(r.error, 'Archive failed');
      return;
    }
    clearSelected();
    announceArchive(r.value);
  }
  const bulkArchiveBlocked = $derived(archiveBlocked(selectedRows));
  /** Bulk Switch account (step 4.4): each selected row this person may
   *  restart and whose account is past `accounts.pause_at` resumes under the
   *  login on its host with the most headroom; the rest stay as they are. */
  const bulkMoveTargets = $derived(bulkTargets(selectedRows, 'restart_session', $sessionBlocked));
  const bulkMoveAccountBlocked = $derived(
    hubActionBlocked('restart_session', $hubStatus, $hubConnection) ??
      (selectedRows.length > 0 && bulkMoveTargets.length === 0 ? 'None of the selected sessions is yours to restart.' : null),
  );
  async function bulkMoveAccount(accountUuid: string | null) {
    const r = await moveToAccount(bulkMoveTargets, accountUuid, restartSession);
    clearSelected();
    const parts = [
      r.moved > 0 ? `Switched ${r.moved} session${r.moved === 1 ? '' : 's'}` : 'Nothing switched',
      r.stayed > 0 ? `${r.stayed} ${accountUuid === null ? 'still under the line' : 'already there'}` : '',
      r.nowhere > 0 ? `${r.nowhere} with ${accountUuid === null ? 'no other login that has room' : 'no login on that account'}` : '',
      r.failed > 0 ? `${r.failed} failed` : '',
    ].filter(Boolean);
    push({ message: parts.join(' · '), kind: r.failed > 0 ? 'error' : r.moved > 0 ? 'success' : 'info' });
  }
  /** Clean up's targets: the rows this person may Safe remove (`own`). */
  const bulkCleanUpTargets = $derived(bulkTargets(selectedRows, 'safe_kill_session', $sessionBlocked));
  const bulkCleanUpBlocked = $derived(
    hubActionBlocked('safe_kill_session', $hubStatus, $hubConnection) ??
      (selectedRows.length > 0 && bulkCleanUpTargets.length === 0 ? 'None of the selected sessions is yours to clean up.' : null),
  );

  async function confirmBulkKill() {
    bulkKillOpen = false;
    const targets = bulkKillTargets;
    clearSelected();
    const results = await Promise.allSettled(
      targets.map(async (sess) => {
        const r = await killSession(sess.host_alias, sess.tmux_name);
        if (!r.ok) {
          pushError(r.error, `Kill ${sess.tmux_name} failed`);
          return;
        }
        const cur = $selectedSession;
        if (cur && sameSession(cur, sess)) selectSession(null);
      }),
    );
    void results;
  }
  // Projects intentionally collapsed by the user. Anything not in this set
  // is open by default — most users have one or two projects and want to
  // see their sessions immediately.
  let collapsed: Set<number> = $state(new Set());
  // Work groups the user collapsed ("group by work" mode), by key.
  let collapsedWork: Set<string> = $state(new Set());

  let sidebarEl: HTMLElement | undefined = $state();

  // Expand the session's project if collapsed, then scroll its row into
  // view. Shared by both reveal effects below — always safe regardless of
  // what moved the selection, so callers don't gate it on anything. Every
  // caller wraps its call in `untrack` so reading `collapsed`/`sidebarEl`
  // here doesn't become an extra tracked dependency of whichever effect is
  // calling it.
  function expandAndScrollTo(sess: SessionRow): void {
    if (sess.project_id !== null && collapsed.has(sess.project_id)) {
      const next = new Set(collapsed);
      next.delete(sess.project_id);
      collapsed = next;
    }
    const workKey = workKeyed?.get(sess.id)?.key;
    if (workKey !== undefined && collapsedWork.has(workKey)) {
      const next = new Set(collapsedWork);
      next.delete(workKey);
      collapsedWork = next;
    }
    void tick().then(() => {
      const el = sidebarEl?.querySelector<HTMLElement>(`[data-session-id="${sess.id}"]`);
      if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'nearest' });
    });
  }

  // Whatever moved the selection — a click, the quick switcher, a
  // rename/recreate resync, a completed move's follow reselect, the
  // selection store's own re-sync: expand its project if collapsed and
  // scroll its row into view. Keyed on the id so reconcile updates (a new
  // row object every tick) neither re-scroll nor undo a later collapse.
  const revealId = $derived($selectedSession?.id ?? null);
  // The id this pair of effects last revealed, so the same session is never
  // scrolled into view twice for one user action.
  let revealedId: number | null = null;
  $effect(() => {
    const id = revealId;
    if (id === null) return;
    untrack(() => {
      // Only when the id actually changed, and not when an explicit select
      // is mid-flight: `selectSessionExplicitly` moves the selection and THEN
      // bumps `revealSeq`, so both writes land in one flush — the effect
      // below is about to reveal this very session (widening the filter
      // first), and revealing here too would scroll for it twice.
      if (id === revealedId || get(revealSeq) !== appliedSeq) return;
      revealedId = id;
      const sess = $selectedSession;
      if (sess) expandAndScrollTo(sess);
    });
  });

  // Widening `hostFilter` is reserved for a deliberate "open this session"
  // action. `selectSessionExplicitly` bumps `revealSeq` AFTER applying the
  // selection; this effect tracks the SEQUENCE NUMBER rather than the
  // session id:
  //  - a non-explicit reselect (a rename/recreate resync, a move's follow
  //    reselect, the selection store's own re-sync) never bumps it, so it
  //    can never widen the filter — no matter how many times the id changes;
  //  - re-selecting the SAME session explicitly still reveals it, even
  //    though the id-keyed effect above wouldn't re-run for that (no id
  //    change) — a bump is a distinct event regardless of the id it targets.
  //
  // `appliedSeq` is captured once, when THIS Sidebar instance is created,
  // and the effect only reacts to a bump that lands AFTER that point — never
  // to `$revealSeq`'s absolute value. That matters because the Sidebar is
  // destroyed and recreated on collapse/expand (App.svelte's `{#if
  // sidebarCollapsed}`): without this, a fresh mount would see whatever
  // `$revealSeq` already was (non-zero after the first-ever explicit select)
  // and treat it as a brand new bump, widening the filter for whatever
  // happens to be selected right then — even a session that arrived via a
  // later NON-explicit reselect while the Sidebar was unmounted. Comparing
  // against this instance's own baseline means a remount never replays a
  // bump from before it existed.
  let appliedSeq = get(revealSeq);
  $effect(() => {
    const seq = $revealSeq;
    if (seq === appliedSeq) return;
    appliedSeq = seq;
    untrack(() => {
      const sess = $selectedSession;
      if (!sess) return;
      revealedId = sess.id;
      if ($effectiveHostFilter !== 'all' && $effectiveHostFilter !== sess.host_alias) hostFilter.set('all');
      expandAndScrollTo(sess);
    });
  });

  // Stores are bootstrapped once by App.svelte's onMount; Sidebar just reads
  // them. (A second bootstrap here would double every startup IPC call.)

  async function onRefresh() {
    loading = true;
    loadError = null;
    refreshError = null;
    const pr = await refreshProjects();
    // Explicit user refresh: bypass the backend's freshness window.
    const sr = await loadSessions({ force: true });
    loading = false;
    // Review r13: the line under the toolbar is a sentence; the toast keeps
    // the code under Details.
    if (!pr.ok) {
      loadError = `Couldn't refresh projects: ${errorText(pr.error)}`;
      refreshError = pr.error;
      pushError(pr.error, 'Refresh projects failed');
    } else if (!sr.ok) {
      loadError = `Couldn't refresh sessions: ${errorText(sr.error)}`;
      refreshError = sr.error;
      pushError(sr.error, 'Refresh sessions failed');
    } else {
      bootstrapError.set(null);
    }
  }

  /** A project matches a search by owner / repo, or through one of `rows`
   *  (its sessions in the list). */
  function matchesSearch(p: ProjectTreeRow, q: string, rows: SessionRow[]): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (p.project.owner.toLowerCase().includes(needle)) return true;
    if (p.project.repo.toLowerCase().includes(needle)) return true;
    return rows.some((s) => sessionMatchesSearch(s, needle));
  }

  // Sessions under the host / bg filters only (no triage predicate): the
  // counters must keep reporting while a triage filter is active, and the
  // project sort must weigh every visible session, not just the filtered ones.
  const hostVisibleSessions = $derived(
    $sessions.filter((s) => sessionVisible(s, $effectiveHostFilter, $showBgAgents, null, scopeSel)),
  );
  // countNeedsYou() classifies each row, and classify() files an external
  // (Outside fleet) row as working/idle, so a read-only row never inflates
  // the pill (spec §5).
  // Redesign 1.1: a mass loss (a host reboot, a tmux server restart) folds
  // into one "12 stopped on trn · Restore" row per host, and its rows leave
  // the badge and the project sort: nobody can answer a stopped pane until it
  // is restored, so counting them would bury the sessions that do need you.
  const lostFoldList = $derived(focus ? [] : lostFolds(hostVisibleSessions));
  const foldedIdSet = $derived(foldedIds(lostFoldList));
  const countedSessions = $derived(
    foldedIdSet.size === 0 ? hostVisibleSessions : hostVisibleSessions.filter((s) => !foldedIdSet.has(s.id)),
  );
  let openFolds = $state<Set<string>>(new Set());
  function toggleFold(host: string) {
    const next = new Set(openFolds);
    if (next.has(host)) next.delete(host);
    else next.add(host);
    openFolds = next;
  }
  const needsYouTotal = $derived(countNeedsYou(countedSessions, attentionOpts));
  const severityByProject = $derived(worstSeverityByProject(countedSessions));

  // Only show projects that either match the filter directly OR have at least
  // one active session. Without sessions the sidebar would be flooded with
  // every cloned repo on disk — most of which the user isn't working on.
  const filtered = $derived(
    sortProjectsBySeverity(
      $projects.filter(
        (p) =>
          matchesSearch(p, viewSearch, sessionsForProject(p.project.id)) &&
          sessionsForProject(p.project.id).length > 0,
      ),
      severityByProject,
    ),
  );

  // Repos that appear under more than one owner — only those need the owner
  // prefix in the sidebar label to disambiguate.
  const collidingRepos = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const r of $projects) {
      counts.set(r.project.repo, (counts.get(r.project.repo) ?? 0) + 1);
    }
    return new Set(Array.from(counts.entries()).filter(([, c]) => c > 1).map(([n]) => n));
  });

  // --- Memoised indices (rebuilt once per $sessions change, not per row) ---

  // ── Group by work (roadmap M1) ──
  // worktree id → branch, and the sessions that carry a work key (tag,
  // branch or worktree name — see work_keys.ts). Only built in work mode.
  const branchById = $derived(worktreeBranchById($projects));
  const workIndex = $derived(
    $sidebarGroupBy === 'work'
      ? buildSessionsByWork(
          $sessions,
          viewHost,
          viewBg,
          rowPredicate,
          (s) => workKeyFor(s, branchById),
          viewScope,
        )
      : null,
  );
  const workKeyed = $derived(workIndex?.keyed ?? null);
  function workGroupMatchesSearch(g: WorkGroup, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (g.key.toLowerCase().includes(needle)) return true;
    return g.sessions.some((s) => sessionMatchesSearch(s, needle));
  }
  const workGroups = $derived(
    workIndex
      ? sortWorkGroups(
          workIndex.groups.filter((g) => workGroupMatchesSearch(g, viewSearch)),
          severity,
        )
      : [],
  );
  // The project tree (and "Other sessions") keep only what no work group
  // took: hybrid grouping, no "Unclassified" bucket. In project mode this is
  // just the triage predicate.
  function untakenBy(keyed: ReadonlyMap<number, unknown> | null, base: SessionPredicate): SessionPredicate {
    if (!keyed || keyed.size === 0) return base;
    return (s) => !keyed.has(s.id) && (base ? base(s) : true);
  }
  const treePredicate = $derived(untakenBy(workKeyed, rowPredicate));

  // ── Past work (roadmap M2.5) ──
  // Ended links: a live group's collapsed "Done · n", and a group of its own
  // (collapsed) for work that has only ended sessions — so reopened work has
  // somewhere to show. Reloaded when the live keys or the session count
  // change (a session ending is what makes past work).
  let openDone: Set<string> = $state(new Set());
  let openPast: Set<string> = $state(new Set());
  const liveWorkKeys = $derived(workGroups.map((g) => g.key).join('\n'));
  $effect(() => {
    if ($sidebarGroupBy !== 'work') return;
    const keys = liveWorkKeys;
    void $sessions.length;
    untrack(() => void loadPastWork(keys ? keys.split('\n') : []));
  });
  const pastOnlyGroups = $derived.by((): { key: string; links: WorkLink[] }[] => {
    // Needs you lists live sessions only: past work never waits on you.
    if ($sidebarGroupBy !== 'work' || focus || needsYouOnly) return [];
    const live = new Set(workGroups.map((g) => g.key));
    const q = searchQuery.toLowerCase();
    const out: { key: string; links: WorkLink[] }[] = [];
    for (const [key, links] of $pastWork) {
      if (live.has(key) || links.length === 0) continue;
      // Hosts (and scopes) outside the filter hide their past work too.
      const shown = links.filter((l) => pastVisible(key, l));
      if (shown.length === 0) continue;
      if (!pastGroupMatchesSearch(key, shown, q)) continue;
      out.push({ key, links: shown });
    }
    return out.sort((a, b) => (b.links[0].ended_at ?? 0) - (a.links[0].ended_at ?? 0));
  });
  // ── Lifecycle (work graph M7.3) ──
  // Archived live sessions collapse into their group's Done (tmux keeps
  // running; one click brings them back); reopened work carries a badge.
  const reopenedKeys = $derived(reopenedByKey($reopenedWork));
  let unarchiving: Set<number> = $state(new Set());
  /**
   * Why this client may not un-archive `sess` (multi-user M1, F2b). The Done
   * section's `archived · show` chip was the one work control in this file
   * gated by neither half: `unarchive_session_work` is `drive` in
   * `share.ts::SESSION_TIER` and ROUTES, so both halves apply. Asked per ROW
   * — the chip is drawn once per archived session, and a group can hold rows
   * of more than one owner.
   */
  function unarchiveBlocked(sess: SessionRow): string | null {
    return (
      hubActionBlocked('unarchive_session_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'unarchive_session_work')
    );
  }
  async function unarchive(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const id = sess.id;
    // Re-asked at the call, not only on the chip: a revoke can arrive while
    // the group is on screen.
    if (unarchiving.has(id) || unarchiveBlocked(sess) !== null) return;
    unarchiving = new Set([...unarchiving, id]);
    const r = await unarchiveSession(id);
    unarchiving = new Set([...unarchiving].filter((x) => x !== id));
    if (!r.ok) pushError(r.error, 'Un-archive failed');
  }
  /** A past (ended) link as a filter row: its snapshot's host, and its
   *  org — else its snapshot project's owner — as its scope. */
  function pastFilterRow(l: WorkLink): FilterRow {
    const owner = l.snap_project_id != null ? $projectOwners.get(l.snap_project_id) : undefined;
    return {
      host: l.snap_host ?? null,
      scope: l.org_id != null ? `org:${l.org_id}` : owner ? `owner:${owner}` : 'unassigned',
      live: false,
      archived: true,
    };
  }
  function toggleIn(set: Set<string>, key: string): Set<string> {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  }

  function toggleWorkCollapse(key: string) {
    const next = new Set(collapsedWork);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    collapsedWork = next;
  }


  // Map: project_id → sessions filtered by current hostFilter. This derived
  // value is read directly in the template so Svelte tracks it reactively —
  // using a plain function via {@const} doesn't establish the dependency.
  const filteredSessionsByProject = $derived(
    buildSessionsByProject($sessions, viewHost, viewBg, treePredicate, viewScope),
  );


  // Map: session.id → count of other sessions sharing the same (project, worktree_key)
  const relatedCountById = $derived(buildRelatedCountById($sessions));

  function sessionsForProject(projectId: number): SessionRow[] {
    return filteredSessionsByProject.get(projectId) ?? [];
  }

  function relatedCountFor(sess: SessionRow): number {
    return relatedCountById.get(sess.id) ?? 0;
  }

  // Sessions whose tmux working directory didn't map to any known project.
  // `external` rows never land here — they have their own read-only
  // "Outside fleet" section below.
  function orphansOf(pred: SessionPredicate): SessionRow[] {
    const q = viewSearch.toLowerCase();
    return $sessions.filter(
      (s) =>
        s.project_id === null &&
        s.kind !== 'external' &&
        sessionVisible(s, viewHost, viewBg, pred, viewScope) &&
        sessionMatchesSearch(s, q),
    );
  }
  const orphanSessions = $derived(orphansOf(treePredicate));
  const sharedWithMe = $derived.by((): SessionRow[] => {
    if (!splitShared || tab === 'mine') return [];
    const q = viewSearch.toLowerCase();
    const base = rowBase;
    return $sessions.filter(
      (s) =>
        s.kind !== 'external' &&
        isSharedWithMe(s) &&
        sessionVisible(s, viewHost, viewBg, base, viewScope) &&
        sessionMatchesSearch(s, q),
    );
  });
  let sharedOpen = $state(true);

  // ── Flat groups (redesign step 3.6): state, host or agent ──
  // The same rows the project tree and "Other sessions" would show, in the
  // same order, regrouped. Collapsed groups are remembered by key.
  const flatBy = $derived(isFlatGroupBy($sidebarGroupBy) ? $sidebarGroupBy : null);
  const flatGroups = $derived(
    flatBy
      ? groupRows(
          [...filtered.flatMap((r) => sessionsForProject(r.project.id)), ...orphanSessions],
          flatBy,
          attentionOpts,
          $startingSessions,
          { scopeOf: $scopeOf, scopes: $scopes },
        )
      : [],
  );
  let collapsedFlat: Set<string> = $state(new Set());
  // "4 more running ›" (Sessions board): the Working group shows its first
  // rows; the line opens the rest. The selected row always shows.
  let expandedFlat: Set<string> = $state(new Set());
  const keepIds = $derived(new Set($selectedSession ? [$selectedSession.id] : []));

  // ── Inbox (redesign step 3.3) ──
  // The rows the list would show under the same filters, narrowed to what
  // raises the badge (`inbox.ts`); the rest is counted in one line.
  const inboxPool = $derived(
    $sidebarView === 'inbox'
      ? [...filtered.flatMap((r) => sessionsForProject(r.project.id)), ...orphanSessions, ...sharedWithMe]
      : [],
  );
  const inboxList = $derived(inboxRows(inboxPool, attentionOpts));
  const inboxNeeding = $derived(inboxList.length + $failingCount + $waitingMissionCount);
  // G1.6: Jev's "probably waiting" rows, listed apart and never counted.
  const inboxProposed = $derived(proposedRows(inboxPool, attentionOpts, $notWaitingSaid));
  // G3.1: the model's sections ("Group: state"), each carrying the model's
  // other rows: missions and Jev's proposals in Needs you, failed routines
  // in Failed. Their counts add up to the badge.
  const inboxSectionList = $derived(
    inboxSections(inboxPool, attentionOpts, $inboxGroupBy, {
      missions: $waitingMissionCount,
      failingRoutines: $failingCount,
      proposed: inboxProposed.length,
    }),
  );
  // Opening the Inbox reads the failed routines and the waiting missions
  // once more, so they are current as it shows (`trackFailingRoutines` and
  // `trackWaitingMissions` keep them fresh after).
  $effect(() => {
    if ($sidebarView === 'inbox')
      untrack(() => {
        void loadFailing();
        void loadWaitingMissions();
      });
  });
  // The footer's row count (UX audit L4): what this list holds right now.
  const listCountText = $derived.by(() => {
    if ($sidebarView === 'work') return '';
    const n = $sidebarView === 'inbox' ? inboxList.length : visibleIds.size + sharedWithMe.length;
    return `${n} ${$sidebarView === 'inbox' ? 'waiting' : n === 1 ? 'session' : 'sessions'}`;
  });
  // Redesign step 7.4: a row whose state moves it to another group is a new
  // element there; snapshot every row's place before the groups re-render so
  // the new one slides from where the old one was (motion_catalog.slideIn).
  $effect.pre(() => {
    void [flatGroups, workGroups, inboxList, orphanSessions];
    untrack(() => snapshotRows(sidebarEl));
  });
  // A folded mass loss is its own Restore line below, not "12 paused" here.
  const inboxRestText = $derived(
    notWaitingText(notWaiting(inboxPool.filter((s) => !foldedIdSet.has(s.id)), attentionOpts)),
  );

  // Interactive Claude sessions running entirely outside fleet (Claude
  // Desktop, a bare terminal). Read-only; the host filter applies but the
  // bg-agent toggle does not.
  const outsideFleet = $derived(
    focus
      ? []
      : buildOutsideFleet($sessions, $effectiveHostFilter, scopeSel).filter(
          (s) => (!rowPredicate || rowPredicate(s)) && sessionMatchesSearch(s, viewSearch.toLowerCase()),
        ),
  );

  let showAddProject = $state(false);
  /** Clone URL the switcher's Add row hands over, prefilled in the dialog. */
  let initialCloneUrl: string | undefined = $state(undefined);
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);

  // Add project ROUTES to the hub now (the clone runs on the host through the
  // hub's transport), so it is gated on the live link like every other routed
  // mutation. Purge still uses this machine's SSH and stays refused with
  // E_LOCAL_ONLY in remote mode (`commands/sessions.rs`).
  const addProjectBlocked = $derived(hubActionBlocked('add_project', $hubStatus, $hubConnection));
  const purgeProjectBlocked = $derived(hubBlock('purge_project', $hubStatus));

  // While the hub's wire contract is outside this build's range, every list
  // load fails with `E_HUB_CONTRACT` and never will heal itself (unlike
  // `reconnecting`/`offline`, where the stores already hold a real last-known
  // list). An empty tree then reads as "you have no projects" instead of
  // "this couldn't load" — so borrow the connection banner's own sentence for
  // this state rather than inventing a second wording.
  // Review r13: why the list cannot be trusted to be empty. A hub that is
  // configured but unusable (its banner says why) or a failed startup load
  // reads as a failure, never as "No projects yet" or "Nothing needs you".
  const listUnavailable = $derived(
    $hubStatus.unavailable !== null ? 'hub' : ($bootstrapError ?? refreshError) !== null ? 'load' : null,
  );

  const hubSkewEmptyMessage = $derived(
    $hubConnection.state === 'hub_too_old' || $hubConnection.state === 'hub_too_new'
      ? connectionBanner($hubConnection, $hubStatus.url)
      : null,
  );

  // The empty list's actions (review r13): the Hosts view to add one, and
  // New session.
  const openAddHost = () => requestHostsView();
  const openNewSession = () => openNewSessionPicker();

  // A project row's own `+`: straight to New session for that project.
  // (The switcher's New session mode is the one place a project is picked.)
  // The dialog is App's one mount, reached through `newSessionRequest`
  // (redesign 1.9): the Sidebar no longer mounts a second copy.
  function openNew(p: ProjectTreeRow, e?: Event) {
    e?.stopPropagation();
    requestNewSession({ project: p });
  }

  // The switcher's Add row asks for the Add project dialog (App-level stores
  // cannot reach this component's state directly).
  $effect(() => {
    const req = $addProjectRequest;
    if (req === null) return;
    addProjectRequest.set(null);
    initialCloneUrl = req.cloneUrl;
    showAddProject = true;
  });

  // The user added a project in order to start a session in it: go straight
  // to NewSessionDialog on the new row (already merged into `projects`).
  function onProjectAdded(row: ProjectTreeRow, host: string) {
    // Adding a project is the person saying it matters: keep it in the picker
    // (quietly: they asked to add a project, not to save a picker choice).
    void setProjectPick(row.project.owner, row.project.repo, { vis: 'keep' }, { quiet: true });
    showAddProject = false;
    // Preselect the host Add project put it on.
    requestNewSession({ project: row, initialHost: host });
  }

  function toggleCollapse(projectId: number) {
    if (collapsed.has(projectId)) {
      collapsed.delete(projectId);
    } else {
      collapsed.add(projectId);
    }
    // Reassign so Svelte detects the Set mutation.
    collapsed = new Set(collapsed);
  }

  function onSelectSession(sess: SessionRow, e?: MouseEvent) {
    // Shift / cmd / ctrl-click (or select mode) toggles the row in the
    // multi-select instead of opening it.
    if (selectMode || (e && (e.shiftKey || e.metaKey || e.ctrlKey))) {
      toggleSelected(sess);
      return;
    }
    // Stop rename mode if the user clicks away to another row.
    if (renaming !== null && !sameSession(renaming, sess)) {
      cancelRename();
    }
    const cur = $selectedSession;
    // While the Hosts view covers the terminal, clicking the open session
    // means "go to it", not "deselect".
    if (cur && cur.id === sess.id && !$hostsViewOpen) {
      selectSession(null);
    } else {
      selectSessionExplicitly(sess);
    }
  }

  /** True only when the key event started on the row element itself.
   *
   *  The row is a tabbable treeitem that CONTAINS real buttons (the
   *  action cluster, revealed by `:focus-within`). `keydown` bubbles from a
   *  focused descendant up to the row, and activating a `<button>` is the
   *  DEFAULT ACTION of that keydown — so an ancestor calling
   *  `preventDefault()` on the way up cancels it. Without this guard the
   *  row's handler swallowed Enter/Space for every nested control (round-20
   *  F4: UX-132 added the tab stops and reached none of the actions). */
  function fromRowItself(e: Event): boolean {
    return e.target === e.currentTarget;
  }

  function onKeySession(e: KeyboardEvent, sess: SessionRow) {
    if (!fromRowItself(e)) return;
    // The list keys (redesign step 3.8), from the shortcut registry.
    const key = matchShortcut('session-list', e, isMac);
    if (!key) return;
    e.preventDefault();
    if (key === 'session-list.open') onSelectSession(sess);
    else if (key === 'session-list.down') focusRowAt(rowIndexOf(sess) + 1);
    else if (key === 'session-list.up') focusRowAt(rowIndexOf(sess) - 1);
    else if (key === 'session-list.pick') {
      if (!selectMode) selectMode = true;
      toggleSelected(sess);
    }
  }

  // ── List keys from anywhere (redesign step 3.8) ──
  // The rows as drawn, top to bottom, whatever the grouping: what j/k walk,
  // what ⌘1–9 count and what "next needs you" searches.
  function shownRows(): HTMLElement[] {
    return Array.from(sidebarEl?.querySelectorAll<HTMLElement>('[data-testid="sess-row"]') ?? []);
  }
  function rowIndexOf(sess: SessionRow): number {
    return shownRows().findIndex((el) => el.dataset.sessionId === String(sess.id));
  }
  function focusRowAt(i: number) {
    const rows = shownRows();
    if (i < 0 || i >= rows.length) return;
    rows[i].focus();
    rows[i].scrollIntoView?.({ block: 'nearest' });
  }
  function openRow(el: HTMLElement | undefined) {
    const sess = el && $sessions.find((s) => String(s.id) === el.dataset.sessionId);
    if (!el || !sess) return;
    if ($selectedSession?.id !== sess.id) selectSessionExplicitly(sess);
    el.focus();
    el.scrollIntoView?.({ block: 'nearest' });
  }
  /** The next row after the open one (wrapping) whose state raises the
   *  Needs you badge: Needs you, Failed or Blocked (step 0.4). */
  function nextNeedingYou(): HTMLElement | undefined {
    const rows = shownRows();
    const cur = rows.findIndex((el) => el.dataset.sessionId === String($selectedSession?.id));
    for (let step = 1; step <= rows.length; step++) {
      const el = rows[(cur + step + rows.length) % rows.length];
      const bucket = el.dataset.bucket as TriageBucket | undefined;
      if (bucket && countsTowardBadge(bucketState(bucket))) return el;
    }
    return undefined;
  }
  function onWindowKeydown(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    const id = matchShortcut('global', e, isMac);
    if (id !== 'next-needs-you' && id !== 'jump-n') return;
    const target = e.target as HTMLElement | null;
    // A modal owns the keyboard, and a field keeps its keys.
    if (target?.closest?.('dialog') || isEditable(target)) return;
    e.preventDefault();
    if (id === 'next-needs-you') openRow(nextNeedingYou());
    else openRow(shownRows()[Number(e.key) - 1]);
  }

  /** Same guard for the project row, which holds + New session and Purge. */
  function onKeyProject(e: KeyboardEvent, projectId: number) {
    if (!fromRowItself(e)) return;
    if (e.key === 'Enter' || e.key === ' ') toggleCollapse(projectId);
  }

  async function beginEdit(sess: SessionRow, mode: 'label' | 'tmux', e?: Event) {
    e?.stopPropagation();
    const original = mode === 'label' ? (sess.friendly_name ?? '') : sess.tmux_name;
    renaming = { id: sess.id, host_alias: sess.host_alias, tmux_name: sess.tmux_name, mode, original };
    renameValue = original;
    renameError = null;
    await tick();
    renameInput?.focus();
    renameInput?.select();
  }

  /** ✎ action: rename the tmux session (new row identity on the backend). */
  function beginRename(sess: SessionRow, e?: Event) {
    return beginEdit(sess, 'tmux', e);
  }

  /** Double-click: edit the display label. */
  function beginLabelEdit(sess: SessionRow, e?: Event) {
    return beginEdit(sess, 'label', e);
  }

  function cancelRename() {
    renaming = null;
    renameValue = '';
    renameError = null;
  }

  async function commitRename() {
    if (committingRename || !renaming) return;
    // Target the exact row that was double-clicked — host + old name from
    // the pinned identity, never a lookup by name alone.
    const target = renaming;
    committingRename = true;
    try {
      const outcome = await applySessionRename(
        { ...target, friendly_name: target.mode === 'label' ? target.original : null },
        target.mode,
        renameValue,
      );
      if (outcome.kind === 'error') {
        renameError = outcome.error.message;
        return;
      }
      // If the renamed session was the selected one, follow the rename.
      const cur = $selectedSession;
      if (outcome.kind === 'ok' && outcome.row && cur && sameSession(cur, target)) {
        selectSession(outcome.row, { follow: true });
      }
      cancelRename();
    } finally {
      committingRename = false;
    }
  }

  const onRenameKey = renameKeyHandler(() => void commitRename(), cancelRename);

  function askKill(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingKill = sess;
  }

  async function confirmKill() {
    if (!pendingKill) return;
    const sess = pendingKill;
    // Re-asked at the call (multi-user M1, F2b): the confirm dialog stays open,
    // so a grant can be narrowed between `askKill` and this click. The row's
    // own Kill button is disabled by `SessionRowItem`'s `killBlocked`, but the
    // dialog in front of it is not that button.
    if ($sessionBlocked(sess, 'kill_session') !== null) return;
    pendingKill = null;
    const r = await killSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) {
      pushError(r.error, 'Kill failed');
      return;
    }
    // If we just killed the selected session, drop the selection so the
    // terminal pane shows the empty state instead of trying to attach.
    const cur = $selectedSession;
    if (cur && sameSession(cur, sess)) {
      selectSession(null);
    }
  }

  function cancelKill() {
    pendingKill = null;
  }

  function askRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingRecreate = sess;
  }

  // Restart kills the running claude process and loses whatever it was in
  // the middle of. Its two neighbours in the same hover strip (Recreate,
  // Kill) both confirm, and its glyph is the same `↻` the sidebar and Files
  // use for a harmless Refresh — so unguarded it was the easiest destructive
  // action in the app to fire by accident.
  function askRestart(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingRestart = sess;
  }

  function cancelRestart() {
    pendingRestart = null;
  }

  async function confirmRestart() {
    if (!pendingRestart) return;
    const sess = pendingRestart;
    if ($sessionBlocked(sess, 'restart_session') !== null) return;
    pendingRestart = null;
    const r = await restartSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) pushError(r.error, 'Restart failed');
  }

  function cancelRecreate() {
    pendingRecreate = null;
  }

  async function confirmRecreate() {
    if (!pendingRecreate) return;
    const sess = pendingRecreate;
    if ($sessionBlocked(sess, 'recreate_session') !== null) return;
    pendingRecreate = null;
    const r = await recreateSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Recreate failed');
      return;
    }
    // kill-session severed the PTY; the tmux_name is unchanged so TerminalView
    // won't auto-reopen. Force a re-attach when this session is selected by
    // dropping and (after the close effect runs) restoring the selection.
    const cur = $selectedSession;
    if (cur && sameSession(cur, sess)) {
      selectSession(null);
      await tick();
      selectSession(r.value, { follow: true });
    }
  }

  // --- New BG Session modal ---
  let showBgModal = $state(false);
  let bgModalHost = $state('local');
  let bgModalName = $state('');
  let bgModalPrompt = $state('');
  let bgModalError = $state<string | null>(null);
  let bgModalLoading = $state(false);

  // --- Name this work… (work graph M11.1) ---
  // In work mode a project group holds the sessions no work group took: its
  // header names one piece of work for them (each session chosen in the
  // dialog; every one by default).
  const nameWorkBlocked = $derived(hubActionBlocked('name_session_work', $hubStatus, $hubConnection));
  let nameWorkFor: { id: number; label: string }[] | null = $state(null);
  function openNameWork(list: readonly SessionRow[], e: Event) {
    e.stopPropagation();
    // Empty means every session in the group was narrowed away by the access
    // gate (multi-user M1): there is nothing to name, and a dialog with no
    // target would submit a write that the hub then refuses.
    if (list.length === 0) return;
    nameWorkFor = list.map((s) => ({ id: s.id, label: s.friendly_name || s.tmux_name }));
  }

  // --- Purge Project ---
  let pendingPurge: ProjectRow | null = $state(null);

  // The work keys whose conversations the purge would take away, named in
  // the confirmation (M2.5). Empty when there are none or the hub is older.
  let purgeKeys: string[] = $state([]);
  $effect(() => {
    const p = pendingPurge;
    purgeKeys = [];
    if (!p) return;
    const hosts = untrack(() => purgeHostsForProject(p.id, $sessions));
    void workPurgeImpact(p.id, hosts).then((r) => {
      if (r.ok && pendingPurge === p) purgeKeys = r.value;
    });
  });

  async function confirmPurge() {
    if (!pendingPurge) return;
    const project = pendingPurge;
    pendingPurge = null;
    // Projects carry no host; purge on every host the project's sessions ran
    // on, in one call. The backend keeps the row unless every host succeeds.
    const result = await purgeProject(
      purgeHostsForProject(project.id, $sessions),
      project.base_path,
      project.id,
    );
    if (!result.ok) {
      pushError(result.error, 'Purge failed; project kept');
    } else {
      push(describePurge(result.value));
      // Refresh stores since the backend doesn't emit row-level events for project deletion
      await loadSessions();
      await refreshProjects();
    }
  }

  function cancelPurge() {
    pendingPurge = null;
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div class="sidebar" data-testid="sidebar-tree" bind:this={sidebarEl}>
  <!-- Review r13: what an empty Sessions list or Inbox says. A list that
       could not load says so, with Retry; a calm Inbox says what is going
       on instead (States board "Inbox empty · calm"). -->
  {#snippet listState(where: 'sessions' | 'inbox')}
    {#if listUnavailable === 'hub'}
      <EmptyState
        kind="none"
        testid="{where}-unavailable"
        title="Not connected to the hub"
        body="Sessions show here once this app can use its hub; the banner above says what is wrong."
        actions={[{ label: 'Hub settings', onclick: () => openSettingsAt('hub'), testid: `${where}-unavailable-settings` }]}
      />
    {:else if listUnavailable === 'load'}
      <LoadError
        title="Couldn't load sessions"
        error={($bootstrapError ?? refreshError)!}
        onretry={() => void onRefresh()}
        retrying={loading}
        testid="{where}-load-error"
      />
    {:else if where === 'inbox'}
      <EmptyState
        kind="calm"
        testid="inbox-calm"
        title="Nothing needs you right now."
        body={inboxRestText ? `Not waiting · ${inboxRestText}` : null}
        actions={[
          { label: 'See running', onclick: () => sidebarView.set('sessions'), testid: 'inbox-calm-running' },
          { label: 'Today', onclick: openToday, testid: 'inbox-calm-today' },
        ]}
      />
    {/if}
  {/snippet}

  {#snippet sessionRow(sess: SessionRow, readOnly = false, inWorkGroup = false, trailing: Snippet | undefined = undefined)}
    <SessionRowItem
      {sess}
      {trailing}
      workKey={inWorkGroup || readOnly ? null : workKeyFor(sess, branchById)}
      workOf={readOnly ? null : workKeyFor(sess, branchById)}
      {selectMode}
      isChecked={selectedIds.has(sess.id)}
      isRenaming={renaming !== null && renaming.id === sess.id}
      renameMode={renaming !== null && renaming.id === sess.id ? renaming.mode : null}
      bind:renameValue
      bind:renameInput
      {renameError}
      relatedCount={relatedCountFor(sess)}
      {nowSec}
      {readOnly}
      {onSelectSession}
      {onKeySession}
      {toggleSelected}
      {beginRename}
      {beginLabelEdit}
      {onRenameKey}
      {commitRename}
      {askRecreate}
      {askRestart}
      {askKill}
      orgColor={orgColorOf(sess, $orgColorById)}
    />
  {/snippet}
  {#snippet foldSessionRow(sess: SessionRow)}
    {@render sessionRow(sess)}
  {/snippet}

  <!-- The shared chrome (Refresh, Needs you, bulk actions, Settings,
       Attention) stays in both views; only the list below swaps. -->
  {#snippet headActions()}
    <button
      class="btn btn--quiet btn--icon"
      title="Launch a supervised Claude background session"
      aria-label="New background session"
      onclick={() => (showBgModal = true)}
      data-testid="new-bg-session-btn"
      use:hintAnchor={{ id: 'bg-session', when: $sessions.some((s) => !hasNoPane(s)) && !$sessions.some((s) => s.kind === 'bg') }}
    ><Icon name="bolt" size={14} /></button>
    <button
      class="btn btn--quiet is-bounded new-btn"
      onclick={() => openNewSessionPicker()}
      data-testid="new-session-head"
      title={isMac ? 'New session (⌘N)' : 'New session (Ctrl+Shift+N)'}
      aria-keyshortcuts={isMac ? 'Meta+N' : 'Control+Shift+N'}
    >+ New…{#if isMac}<kbd class="of-kbd">⌘N</kbd>{/if}</button>
  {/snippet}

  <SidebarFilters
    {headActions}
    listView={$sidebarView}
    bind:search
    bind:recency
    bind:needsYouOnly
    {loading}
    {loadError}
    {onRefresh}
    {onCollapse}
    needsYouCount={needsYouTotal}
    {selectMode}
    {toggleSelectMode}
    selectedCount={selectedIds.size}
    onBulkSend={() => (bulkPromptOpen = true)}
    onBulkKill={() => ((bulkKillMode = 'kill'), (bulkKillOpen = true))}
    onBulkCleanUp={() => ((bulkKillMode = 'cleanup'), (bulkKillOpen = true))}
    onBulkArchive={() => void bulkArchive()}
    onBulkMoveAccount={(a) => void bulkMoveAccount(a)}
    {bulkMoveAccountBlocked}
    {bulkArchiveBlocked}
    {bulkCleanUpBlocked}
    {clearSelected}
  />

  {#if $sidebarView === 'work'}
  <WorkTree />
  {:else if $sidebarView === 'inbox'}
  <!-- The Inbox (redesign step 3.3): only what raises the badge, worst
       first, then one line for everything else and the way to it. -->
  <div class="scroller inbox" data-testid="inbox">
    <div class="section-header inbox-head" data-testid="inbox-head">
      {listUnavailable && inboxNeeding === 0
        ? 'Inbox'
        : inboxNeeding === 0
          ? 'Nothing needs you'
          : `${inboxNeeding} need${inboxNeeding === 1 ? 's' : ''} you`}
      <!-- One queue has no section header: "+1 proposed" sits here. -->
      {#if $inboxGroupBy === 'none' && inboxProposed.length > 0}<span class="muted" data-testid="inbox-proposed-count"
          >{proposedText(inboxProposed.length)}</span
        >{/if}
    </div>
    {#if inboxNeeding === 0}
      {@render listState('inbox')}
    {/if}
    {#each inboxSectionList as sec (sec.key)}
      <div class="inbox-section" data-testid="inbox-section" data-key={sec.key}>
        {#if sec.label}
          <div class="section-header inbox-sec" class:failed={sec.key === 'failed'} data-testid="inbox-section-head">
            {sec.label}<span class="count" data-testid="inbox-section-count">{sec.count}</span>
            {#if sec.proposed && inboxProposed.length > 0}<span class="muted" data-testid="inbox-proposed-count"
                >{proposedText(inboxProposed.length)}</span
              >{/if}
          </div>
        {/if}
        <div class="tree" role="tree" aria-label={sec.label ?? 'Needs you'}>
          {#each sec.rows as sess (sess.id)}
            {@render sessionRow(sess)}
          {/each}
        </div>
        <!-- Redesign 8.6: routines whose newest run failed, with Fix, Retry, Pause. -->
        {#if sec.routines}<RoutineFailures />{/if}
        <!-- G1.6: missions waiting on a person (a grant to sign, a question). -->
        {#if sec.missions}<MissionWaits />{/if}
        {#if sec.proposed && inboxProposed.length > 0}
          <!-- G1.6/G3.1: Jev's "probably waiting", at the foot of Needs you,
               never in its count; "Not waiting" sets the reading aside. -->
          <div class="tree" role="tree" aria-label="Probably waiting" data-testid="inbox-proposed">
            {#each inboxProposed as sess (sess.id)}
              {@render sessionRow(sess)}
              <div class="proposed-line">
                <ProposedBy
                  proposal={{ value: 'waiting', source: 'jev', reason: 'last turn ended with a question' }}
                  field="probably_waiting"
                  stated
                  changeLabel="Not waiting"
                  onchange={() => sayNotWaiting(sess)}
                  testid="inbox-proposed-by"
                />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    <div class="inbox-rest" data-testid="inbox-rest">
      {#if inboxRestText && !(inboxNeeding === 0 && !listUnavailable)}<span class="muted">Not waiting · {inboxRestText}</span>{/if}
      <button
        type="button"
        class="btn btn--quiet"
        data-testid="inbox-all-sessions"
        onclick={() => sidebarView.set('sessions')}>All sessions →</button
      >
    </div>
    <!-- G3.1: a mass loss is the Sessions list's one Restore line here too. -->
    {#each lostFoldList as fold (fold.host)}
      <LostFoldRow {fold} open={openFolds.has(fold.host)} ontoggle={() => toggleFold(fold.host)} row={foldSessionRow} />
    {/each}
  </div>
  {:else}
  {#if tabsShown}
    <!-- All / Mine / Shared with me (Sessions board), above the list. -->
    <div class="scope-tabs" role="tablist" aria-label="Whose sessions" data-testid="scope-tabs" use:tablistKeys>
      {#each SCOPE_TABS as t (t)}
        <button
          type="button"
          role="tab"
          class="scope-tab"
          class:on={tab === t}
          aria-selected={tab === t}
          tabindex={tab === t ? 0 : -1}
          data-testid="scope-tab-{t}"
          onclick={() => scopeTab.set(t)}
          >{SCOPE_TAB_LABELS[t]}{#if t !== 'mine' && tabCounts[t] > 0}<span class="count">{tabCounts[t]}</span>{/if}</button
        >
      {/each}
    </div>
  {/if}
  <div class="scroller">
    {#snippet pastRow(key: string, l: WorkLink)}
      <div
        class="past-row"
        role="treeitem"
        aria-selected="false"
        data-testid="past-work-row"
        title="Ended {l.ended_at ? timeAgo(l.ended_at, nowSec * 1000) : ''}{l.snap_worktree ? ` · worktree ${l.snap_worktree}` : ''}"
      >
        <span class="past-label">{l.snap_name ?? l.snap_tmux ?? key}</span>
        <span class="past-meta"
          >{[l.snap_host, l.snap_branch].filter(Boolean).join(' · ')}{l.ended_at
            ? ` · ended ${timeAgo(l.ended_at, nowSec * 1000)}`
            : ''}</span
        >
        {#if l.resumable === false}<span class="past-purged" title="Its transcripts were purged: only a fresh start is possible">purged</span>{/if}
        <ResumeButton workKey={key} link={l} />
        <SummarizeButton workKey={key} link={l} />
      </div>
    {/snippet}
    {#if workGroups.length > 0 || pastOnlyGroups.length > 0}
      <ul class="tree work-tree" data-testid="work-groups" role="tree" aria-label="Work">
        {#each workGroups as g (g.key)}
          {@const isCollapsed = collapsedWork.has(g.key)}
          {@const pr = workGroupPrSummary(g.sessions)}
          {@const ticket = workGroupTicket(g.key, g.sessions)}
          {@const split = focus
            ? { live: g.sessions, archived: [] }
            : splitArchived(g.sessions, (s) => needsYou(s, attentionOpts))}
          {@const reopened = reopenedKeys.get(g.key)}
          {@const groupColor = orgColorOf(
            { org_id: g.sessions[0]?.work?.org_id ?? g.sessions[0]?.org_id ?? null },
            $orgColorById,
          )}
          <li class="proj" role="none">
            <div
              class="proj-row work-row"
              class:work-done={ticket?.status?.category === 'done'}
              data-testid="work-row"
              data-org-color={groupColor ?? undefined}
              style:box-shadow={groupColor ? `inset 3px 0 0 ${groupColor}` : undefined}
              title="Sessions whose tag, branch or worktree names {g.key}"
              role="treeitem"
              aria-selected="false"
              tabindex="0"
              aria-expanded={!isCollapsed}
              onclick={() => toggleWorkCollapse(g.key)}
              onkeydown={(e) => {
                if (!fromRowItself(e)) return;
                if (e.key === 'Enter' || e.key === ' ') toggleWorkCollapse(g.key);
              }}
            >
              <span class="caret" class:collapsed={isCollapsed}>▾</span>
              <span class="label"
                ><span class="work-key" class:unavailable={ticket?.status?.unavailable}
                  >{g.key}</span
                >{#if ticket?.status && !ticket.status.unavailable}<span
                    class="work-dot {statusDotClass(ticket.status.category)}"
                    data-testid="work-header-dot"
                    title={ticket.status.name ?? ticket.status.category}
                  ></span>{/if}{#if ticket?.title}<span
                    class="work-title"
                    data-testid="work-header-title"
                    title={ticket.status?.unavailable
                      ? `${ticket.title} — ${unavailableLabel('not_found_or_no_permission')}`
                      : ticket.title}>{ticket.title}</span
                  >{/if}</span
              >
              {#if pr.prCount > 0}
                <span
                  class="work-pr"
                  data-testid="work-pr"
                  title="{pr.prCount} pull request{pr.prCount === 1 ? '' : 's'}{pr.ci ? ` · CI ${pr.ci}` : ''}"
                >PR{pr.prCount > 1 ? ` ×${pr.prCount}` : ''}{#if pr.ci}<span
                      class="work-ci"
                      style="color: {ciStatusColor(pr.ci)};"> {ciStatusLabel(pr.ci)}</span
                    >{/if}</span>
              {/if}
              {#if reopened}
                <span class="work-reopened" data-testid="work-reopened-badge">{reopenedBadge(reopened)}</span>
                <ResumeButton workKey={g.key} />
              {/if}
              <span class="count">{split.live.length}</span>
            </div>

            {#if !isCollapsed}
              {@const past = needsYouOnly || focus ? [] : ($pastWork.get(g.key) ?? []).filter((l) => pastVisible(g.key, l))}
              <div role="group">
              {#each split.live as sess (sess.id)}
                {@render sessionRow(sess, false, true)}
              {/each}
              {#if past.length + split.archived.length > 0}
                <div
                  class="done-row"
                  data-testid="work-done"
                  role="treeitem"
                  aria-selected="false"
                  tabindex="0"
                  aria-expanded={openDone.has(g.key)}
                  onclick={() => (openDone = toggleIn(openDone, g.key))}
                  onkeydown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') openDone = toggleIn(openDone, g.key);
                  }}
                >
                  <span class="caret" class:collapsed={!openDone.has(g.key)}>▾</span>
                  Done · {past.length + split.archived.length}
                </div>
                {#if openDone.has(g.key)}
                  {#each split.archived as sess (sess.id)}
                    <!-- The chip sits inside the row, so the treeitem owns it. -->
                    {#snippet archivedChip()}
                      <button
                        class="archived-chip"
                        data-testid="archived-chip"
                        disabled={unarchiving.has(sess.id) || unarchiveBlocked(sess) !== null}
                        title={unarchiveBlocked(sess) ??
                          'Archived: collapsed here, tmux still running. Click to bring it back (a prompt or an attach does too)'}
                        onclick={(e) => void unarchive(sess, e)}>archived · show</button
                      >
                    {/snippet}
                    <div class="archived-wrap" data-testid="archived-session" role="none">
                      {@render sessionRow(sess, false, true, archivedChip)}
                    </div>
                  {/each}
                  {#each past as l (l.id)}{@render pastRow(g.key, l)}{/each}
                {/if}
              {/if}
              </div>
            {/if}
          </li>
        {/each}
        {#each pastOnlyGroups as pg (pg.key)}
          {@const isOpen = openPast.has(pg.key)}
          <li class="proj past-only" data-testid="past-work-group" role="none">
            <div
              class="proj-row work-row"
              data-testid="past-work-header"
              title="Past work on {pg.key}: no session is running it"
              role="treeitem"
              aria-selected="false"
              tabindex="0"
              aria-expanded={isOpen}
              onclick={() => (openPast = toggleIn(openPast, pg.key))}
              onkeydown={(e) => {
                if (!fromRowItself(e)) return;
                if (e.key === 'Enter' || e.key === ' ') openPast = toggleIn(openPast, pg.key);
              }}
            >
              <span class="caret" class:collapsed={!isOpen}>▾</span>
              <span class="label"><span class="work-key">{pg.key}</span></span>
              {#if reopenedKeys.get(pg.key)}
                <span class="work-reopened" data-testid="work-reopened-badge"
                  >{reopenedBadge(reopenedKeys.get(pg.key)!)}</span
                >
              {:else}
                <span class="past-note">{pastWorkSummary(pg.links, nowSec * 1000)}</span>
              {/if}
              <ResumeButton workKey={pg.key} link={pg.links[0]} />
            </div>
            {#if isOpen}
              <div role="group">
                {#each pg.links as l (l.id)}{@render pastRow(pg.key, l)}{/each}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if flatBy && flatGroups.length > 0}
      <ul class="tree flat-groups" data-testid="flat-groups" data-group-by={flatBy} role="tree" aria-label="Sessions by {flatBy}">
        {#each flatGroups as g (g.key)}
          {@const isCollapsed = collapsedFlat.has(g.key)}
          <li class="proj" role="none">
            <div
              class="proj-row"
              data-testid="flat-group"
              data-group={g.key}
              role="treeitem"
              aria-selected="false"
              tabindex="0"
              aria-expanded={!isCollapsed}
              onclick={() => (collapsedFlat = toggleIn(collapsedFlat, g.key))}
              onkeydown={(e) => {
                if (!fromRowItself(e)) return;
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  collapsedFlat = toggleIn(collapsedFlat, g.key);
                }
              }}
            >
              <span class="caret" class:collapsed={isCollapsed}>▾</span>
              <span class="label">{g.label}</span>
              <span class="count">{g.rows.length}</span>
            </div>
            {#if !isCollapsed}
              {@const offlineHost = flatBy === 'host' ? $hostByAlias.get(g.key) : undefined}
              {#if offlineHost && offlineHost.reachable === false}
                <!-- Review r13 (step 3.14): an unreachable host is said inside
                     its own group, with what it means for its sessions. -->
                <HostOffline
                  alias={offlineHost.alias}
                  lastSeen={offlineHost.health_at ?? null}
                  sessions={g.rows.length}
                  ontry={() => void onRefresh()}
                  trying={loading} />
              {/if}
              {@const capped = capRows(g, expandedFlat, keepIds)}
              <div role="group">
                {#each capped.shown as sess (sess.id)}
                  {@render sessionRow(sess)}
                {/each}
                {#if capped.hidden > 0}
                  <button
                    type="button"
                    class="btn btn--quiet group-more"
                    data-testid="group-more"
                    onclick={() => (expandedFlat = toggleIn(expandedFlat, g.key))}
                    >{moreRunningText(capped.hidden)} ›</button
                  >
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if !flatBy && filtered.length > 0}
      <ul class="tree" role="tree" aria-label="Projects">
        {#each filtered as row (row.project.id)}
          {@const projectSessions = filteredSessionsByProject.get(row.project.id) ?? []}
          {@const isCollapsed = collapsed.has(row.project.id)}
          <li class="proj" role="none">
            <div
              class="proj-row"
              data-testid="proj-row"
              title={row.project.base_path}
              role="treeitem"
              aria-selected="false"
              tabindex="0"
              aria-expanded={!isCollapsed}
              onclick={() => toggleCollapse(row.project.id)}
              onkeydown={(e) => onKeyProject(e, row.project.id)}
            >
              <span class="caret" class:collapsed={isCollapsed}>▾</span>
              <span class="label">
                {#if collidingRepos.has(row.project.repo)}<span class="owner"
                    >{row.project.owner}/</span
                  >{/if}<span class="repo">{row.project.repo}</span>
              </span>
              <span class="count">{projectSessions.length}</span>
              {#if $sidebarGroupBy === 'work' && projectSessions.length > 0}
                <!-- Narrowed to the sessions this client may actually write a
                     work link to (multi-user M1), the same way `bulkKillTargets`
                     narrows select mode: `name_session_work` is `drive` in
                     `share.ts::SESSION_TIER`, and `SessionRowItem`'s per-session
                     "Rename…" has composed both halves since F2 while this group
                     header asked only the hub's — the same control, two surfaces,
                     two answers. When nothing in the group qualifies the button
                     says why, in `share.ts`' own words rather than a new
                     sentence. -->
                {@const nameTargets = bulkTargets(projectSessions, 'name_session_work', $sessionBlocked)}
                {@const groupNameBlocked =
                  nameWorkBlocked ??
                  (nameTargets.length === 0
                    ? $sessionBlocked(projectSessions[0], 'name_session_work')
                    : null)}
                <button
                  class="icon-btn small"
                  data-testid="name-work-group"
                  disabled={groupNameBlocked !== null}
                  title={groupNameBlocked ??
                    `Name this work… (${nameTargets.length} session${nameTargets.length === 1 ? '' : 's'} with no work)`}
                  aria-label="Name this work"
                  onclick={(e) => openNameWork(nameTargets, e)}
                >#</button>
              {/if}
              <button
                class="icon-btn"
                onclick={(e) => openNew(row, e)}
                title="New session in this project"
                aria-label="New session"
              >
                +
              </button>
              <button
                class="icon-btn small purge-btn"
                title={purgeProjectBlocked ?? 'Purge Claude Code project state (irreversible)'}
                disabled={purgeProjectBlocked !== null}
                onclick={(e) => { e.stopPropagation(); pendingPurge = row.project; }}
                data-testid="purge-project"
                aria-label="Purge project"
              ><Icon name="trash" size={12} /></button>
            </div>

            {#if !isCollapsed}
              <div role="group">
                {#each projectSessions as sess (sess.id)}
                  {@render sessionRow(sess)}
                {/each}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if listUnavailable && orphanSessions.length === 0 && sharedWithMe.length === 0 && workGroups.length === 0 && pastOnlyGroups.length === 0}
      {@render listState('sessions')}
    {:else if !loadError && orphanSessions.length === 0 && sharedWithMe.length === 0 && workGroups.length === 0 && pastOnlyGroups.length === 0}
      {#if !hubSkewEmptyMessage && listFacets.length > 0 && $projects.length > 0}
        <!-- Filters hide every row: say which, and offer the way back,
             instead of "no sessions" over a fleet that has some, as the states
             kit's no-results (review r13, States board "Search with no
             results"). -->
        
          <EmptyState
            kind="none"
            testid="sidebar-empty"
            title="No sessions match {facetSentence(listFacets)}."
            actions={[
              { label: 'Clear filters', onclick: clearListFilters, primary: true, testid: 'sidebar-empty-clear' },
              ...(archivedHidden > 0
                ? [{ label: `Include archived (${archivedHidden})`, onclick: () => setShowArchived(true), testid: 'sidebar-empty-archived' }]
                : []),
              { label: 'Start a new session', onclick: openNewSession, testid: 'sidebar-empty-new' },
            ]}
          />
        
      {:else if !hubSkewEmptyMessage && $hosts.length === 0}
        <!-- Review r13, States board "First run": one host to start with. -->
        <EmptyState
          kind="first"
          testid="sidebar-empty"
          title="Start with one host"
          body="Sessions run in tmux on a host you can reach over SSH. Add one, or pair this app with a hub that already has them."
          actions={[
            { label: 'Add a host…', onclick: openAddHost, primary: true, testid: 'sidebar-empty-add-host' },
            { label: 'Pair with a hub', onclick: () => openSettingsAt('hub'), testid: 'sidebar-empty-pair' },
          ]}
        />
      {:else}
        <p class="empty" data-testid="sidebar-empty">
          {hubSkewEmptyMessage ??
            ($projects.length === 0
              ? 'No projects yet. Set a projects base in Settings → Projects, or click ↻ to scan.'
              : 'No active sessions. Click + below to start one.')}
        </p>
      {/if}
    {/if}

    {#if !flatBy && orphanSessions.length > 0}
      <div class="orphan-section" data-testid="orphan-sessions">
        <div class="section-header">Other sessions ({orphanSessions.length})</div>
        <div class="tree" role="tree" aria-label="Other sessions">
          {#each orphanSessions as sess (sess.id)}
            {@render sessionRow(sess)}
          {/each}
        </div>
      </div>
    {/if}

    {#if sharedWithMe.length > 0}
      <div class="orphan-section" data-testid="shared-with-me">
        <button
          class="section-header section-toggle"
          data-testid="shared-with-me-toggle"
          aria-expanded={sharedOpen}
          onclick={() => (sharedOpen = !sharedOpen)}
        >
          <span class="caret" class:collapsed={!sharedOpen}>▾</span>
          Shared with me ({sharedWithMe.length})
        </button>
        {#if sharedOpen || tab === 'shared'}
          <div class="tree" role="tree" aria-label="Shared with me">
            {#each sharedWithMe as sess (sess.id)}
              {@const level = $accessOf(sess)}
              {#snippet sharedBy()}
                {#if isSharedAccess(level)}
                  <div class="shared-by" data-testid="shared-by">{sharedByLine(sess, level, $orgList)}</div>
                {/if}
              {/snippet}
              {@render sessionRow(sess, false, false, sharedBy)}
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    {#if archivedHidden > 0 || ($workFilters.archived && !focus)}
      <div class="archived-row" data-testid="archived-row">
        {#if $workFilters.archived}
          <span>Showing archived work</span>
          <button class="btn btn--quiet" type="button" data-testid="archived-toggle" onclick={() => setShowArchived(false)}
            >Hide archived</button
          >
        {:else}
          <span>{archivedHidden} archived hidden</span>
          <button class="btn btn--quiet" type="button" data-testid="archived-toggle" onclick={() => setShowArchived(true)}
            >Show archived</button
          >
        {/if}
      </div>
    {/if}

    {#each lostFoldList as fold (fold.host)}
      <LostFoldRow {fold} open={openFolds.has(fold.host)} ontoggle={() => toggleFold(fold.host)} row={foldSessionRow} />
    {/each}

    {#if outsideFleet.length > 0}
      <div class="orphan-section" data-testid="outside-fleet-section">
        <button
          class="section-header section-toggle"
          data-testid="outside-fleet"
          aria-expanded={outsideOpen}
          onclick={() => (outsideOpen = !outsideOpen)}
        >
          <span class="caret" class:collapsed={!outsideOpen}>▾</span>
          Outside fleet ({outsideFleet.length})
        </button>
        {#if outsideOpen}
          <div class="tree" role="tree" aria-label="Outside fleet">
            {#each outsideFleet as sess (sess.id)}
              {@render sessionRow(sess, true)}
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    {#if unclaimedTotal > 0}
      <!-- A count, and nothing else. No caret, no toggle, no rows: there is
           deliberately no way to expand this, because there is nothing behind
           it — fleet serves a number for an unclaimed session and no metadata
           at all (multi-user M1, rule 6). Claiming one is done from the
           session's own pane, or with `fleet-hub session claim`; a button here
           would need a session id this surface does not have, and spec §4.3
           forbids it outright. -->
      <div class="orphan-section" data-testid="unclaimed-section">
        <div class="section-header unclaimed-header" data-testid="unclaimed-count">
          Unclaimed ({unclaimedTotal})
        </div>
        <p class="unclaimed-note" data-testid="unclaimed-hosts">
          {#each unclaimedByHost as h, i (h.alias)}{i > 0 ? ' · ' : ''}{h.alias}
            {h.count}{/each}
        </p>
        <p class="unclaimed-note">
          tmux sessions fleet did not start and nobody has claimed. Nothing else
          about them is shown — claim one from its own pane, or with
          <code>fleet-hub session claim</code>.
        </p>
      </div>
    {/if}
  </div>
  {/if}

  <!-- UX audit L4: + New moved to the list's header; the footer names the
       list's keys and how many rows it holds. -->
  <footer class="sidebar-footer" data-testid="sidebar-chrome-bottom">
    <div class="footer-row">
      <span class="keys" data-testid="sidebar-keys"
        ><kbd class="of-kbd">j</kbd><kbd class="of-kbd">k</kbd> move <kbd class="of-kbd">↵</kbd> open
        <kbd class="of-kbd">x</kbd> select</span
      >
      <span class="spacer"></span>
      <span class="list-count" data-testid="sidebar-count">{listCountText}</span>
    </div>
  </footer>
</div>

{#if showAddProject}
  <AddProjectDialog
    onCreated={onProjectAdded}
    onCancel={() => (showAddProject = false)}
    {initialCloneUrl}
    blocked={addProjectBlocked}
  />
{/if}


{#if pendingKill}
  <KillDialog
    targets={[pendingKill]}
    onkill={confirmKill}
    oncleaned={cleanedUp}
    oncancel={cancelKill}
    confirmTestId="confirm-kill"
  />
{/if}

{#if bulkKillOpen}
  <KillDialog
    targets={bulkKillMode === 'cleanup' ? bulkCleanUpTargets : bulkKillTargets}
    mode={bulkKillMode}
    onkill={confirmBulkKill}
    oncleaned={(removed) => {
      bulkKillOpen = false;
      clearSelected();
      cleanedUp(removed);
    }}
    oncancel={() => (bulkKillOpen = false)}
    confirmTestId="confirm-bulk-kill"
  >
    {#snippet notes()}
      {#if bulkKillTargets.length === 0}
        <span data-testid="bulk-kill-none"
          >None of the selected sessions is yours to kill — a session shared with you
          can be watched or driven, never killed.</span
        >
      {:else if bulkKillTargets.length < selectedRows.length}
        <!-- Said out loud rather than silently dropped: the count in the title
             no longer matches the selection, and the reason is a rule. -->
        <span data-testid="bulk-kill-skipped"
          >{selectedRows.length - bulkKillTargets.length} selected session{selectedRows.length -
            bulkKillTargets.length ===
          1
            ? ' is'
            : 's are'} not yours to kill and will be left alone.</span
        >
      {/if}
    {/snippet}
  </KillDialog>
{/if}

{#if bulkPromptOpen}
  <BulkPromptDialog targets={bulkPromptTargets} onClose={() => (bulkPromptOpen = false)} />
{/if}

{#if pendingRestart}
  <ConfirmDialog
    title="Restart claude?"
    confirmLabel="Restart"
    danger
    onconfirm={confirmRestart}
    oncancel={cancelRestart}
    confirmTestId="confirm-restart"
  >
    This stops the claude process in <code>{pendingRestart.tmux_name}</code> on
    <code>{pendingRestart.host_alias}</code> and starts a fresh one. Anything it is
    working on right now is lost; the tmux session and the worktree are kept. Continue?
  </ConfirmDialog>
{/if}

{#if pendingRecreate}
  <ConfirmDialog
    title="Recreate session?"
    confirmLabel="Recreate"
    danger
    onconfirm={confirmRecreate}
    oncancel={cancelRecreate}
    confirmTestId="confirm-recreate"
  >
    This kills the tmux session <code>{pendingRecreate.tmux_name}</code> on
    <code>{pendingRecreate.host_alias}</code> and the running claude state inside it,
    then starts a fresh session in the same worktree. Continue?
  </ConfirmDialog>
{/if}

{#if nameWorkFor}
  <NameWorkDialog
    target={{ mode: 'name', sessions: nameWorkFor }}
    onclose={() => (nameWorkFor = null)}
  />
{/if}
{#if pendingPurge}
  <ConfirmDialog
    title="Purge project?"
    confirmLabel="Purge"
    danger
    onconfirm={confirmPurge}
    oncancel={cancelPurge}
    confirmTestId="confirm-purge"
  >
    This will permanently delete all Claude Code state for <code>{pendingPurge.repo}</code>. This is irreversible.
    {#if purgeKeys.length > 0}
      <p class="purge-work" data-testid="purge-work-keys">
        Past work loses its conversations: {purgeKeys.join(', ')}. It can only be restarted
        fresh (with a brief) afterwards, not continued.
      </p>
    {/if}
  </ConfirmDialog>
{/if}

{#if showBgModal}
  <NewBgSessionDialog
    bind:bgModalHost
    bind:bgModalName
    bind:bgModalPrompt
    bind:bgModalError
    bind:bgModalLoading
    onClose={() => (showBgModal = false)}
  />
{/if}

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    /* The pane host (App.svelte) renders us inside a full-bleed Pane with
       no padding, so we add our own — and keep header/footer pinned via
       flex layout (header = flex:0, scroller = flex:1, footer = flex:0).
       That way search/filter and theme/new-session are always visible no
       matter how long the project list grows. */
  }

  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    line-height: 1;
    cursor: pointer;
    min-width: var(--control-h);
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }
  .icon-btn.small {
    padding: 0.1rem 0.35rem;
    font-size: var(--text-xs);
    min-width: var(--control-h);
    border-color: transparent;
  }
  .icon-btn.small:hover { border-color: var(--border); }

  .scroller {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
    padding: 0.4rem 0.6rem;
  }

  .tree { list-style: none; margin: 0; padding: 0; }
  .proj { margin-bottom: 0.15rem; }

  .proj-row {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-weight: 500;
    font-size: var(--text-xs);
    padding: 0.3rem 0.4rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    user-select: none;
  }
  .proj-row:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  /* A tabbable role="button" with no visible focus was the whole project
     tree's WCAG 2.4.7 gap. Drawn inward: the sidebar list clips. */
  .proj-row:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .caret {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    width: 0.7rem;
    text-align: center;
    transition: transform var(--dur-fast) ease;
    display: inline-block;
  }
  .caret.collapsed { transform: rotate(-90deg); }
  .proj-row .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .owner { color: var(--fg-muted); font-weight: 400; }
  .repo { color: var(--fg); }
  .work-key {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-weight: 600;
  }
  .work-key.unavailable {
    text-decoration: line-through;
    opacity: 0.6;
  }
  .work-dot {
    display: inline-block;
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    margin-left: 0.3rem;
    background: var(--fg-muted);
    vertical-align: middle;
  }
  .work-dot.dot-progress {
    background: var(--accent);
  }
  .work-dot.dot-done {
    background: var(--status-done);
  }
  .work-title {
    margin-left: 0.4rem;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .done-row {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.1rem 0.5rem 0.1rem 1.6rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    cursor: pointer;
  }
  /* Work graph M7.3: a done item's group reads as finished; an archived live
     session sits in Done with a chip that brings it back. */
  .work-row.work-done {
    opacity: 0.6;
  }
  .work-reopened {
    font-size: var(--text-2xs);
    color: var(--accent);
    white-space: nowrap;
  }
  .archived-wrap {
    position: relative;
    opacity: 0.75;
  }
  .archived-chip {
    position: absolute;
    right: 0.5rem;
    top: 0.15rem;
    font-size: var(--text-2xs);
    padding: 0 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
  }
  .past-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    padding: 0.15rem 0.5rem 0.15rem 1.6rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    opacity: 0.85;
  }
  .past-label {
    color: var(--fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .past-meta {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .past-note {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .past-purged {
    font-size: var(--text-2xs);
    color: var(--danger);
  }
  .purge-work {
    margin: 0.5rem 0 0;
  }
  .work-pr {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .count {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    padding: 0.05rem 0.4rem;
    border-radius: var(--radius-pill);
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    min-width: 1.4rem;
    text-align: center;
  }

  .empty { color: var(--fg-muted); font-size: var(--text-xs); padding: 0.5rem 0.4rem; }
  .archived-row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0.4rem 4px;
    padding: 4px 2px;
    border-top: 1px dashed var(--border);
    color: var(--fg-muted);
    font-size: var(--control-font);
  }
  .archived-row span { flex: 1; }

  .scope-tabs {
    display: flex;
    gap: 14px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .scope-tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 6px 0;
    font: inherit;
    font-size: var(--text-xs);
    color: var(--fg-muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .scope-tab.on {
    color: var(--fg);
    border-bottom-color: var(--accent);
    font-weight: 500;
  }
  .shared-by {
    font-size: var(--text-xs);
    line-height: 16px;
    color: var(--fg-muted);
    padding: 0 0 4px 28px;
  }
  .group-more {
    font-size: var(--text-xs);
    color: var(--fg-muted);
    margin: 0 0 2px 22px;
  }
  .orphan-section {
    border-top: 1px solid var(--border);
    padding-top: 0.35rem;
    margin-top: 0.35rem;
  }
  /* The unclaimed count (multi-user M1): a label, not a toggle — there is no
     caret because there is nothing to open. */
  .unclaimed-header {
    cursor: default;
  }
  .unclaimed-note {
    margin: 0 0 0.25rem;
    padding: 0 0.4rem;
    font-size: var(--text-2xs);
    line-height: 1.4;
    color: var(--fg-muted);
  }
  .inbox-rest {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.5rem 0.6rem;
    font-size: var(--text-2xs);
  }
  .inbox-rest .muted { color: var(--fg-muted); }
  .inbox-head .muted { color: var(--fg-muted); font-weight: 400; margin-left: 6px; }
  /* G3.1: a state section's header, "Needs you 4 +1 proposed" (Main board). */
  .inbox-sec { display: flex; align-items: center; gap: 6px; color: var(--status-waiting); }
  .inbox-sec.failed { color: var(--status-failed); }
  .inbox-sec .count { font-weight: 500; padding: 0 5px; border-radius: var(--radius-sm); background: var(--bg-hover); color: var(--fg-2); text-transform: none; }
  .inbox-sec .muted { color: var(--fg-muted); font-weight: 400; text-transform: none; letter-spacing: 0; }
  .proposed-line { display: flex; gap: 8px; align-items: center; justify-content: space-between; padding: 0 12px 6px 28px; font-size: var(--text-xs); }
  .section-header {
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    padding: 0 0 0.2rem 0.4rem;
  }
  .section-toggle {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    width: 100%;
    background: transparent;
    border: none;
    cursor: pointer;
    font-family: inherit;
  }
  .section-toggle .caret {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    width: 0.7rem;
    text-align: center;
    transition: transform var(--dur-fast) ease;
    display: inline-block;
  }
  .section-toggle .caret.collapsed { transform: rotate(-90deg); }

  .sidebar-footer {
    flex: 0 0 auto;
    border-top: 1px solid var(--border);
    padding: 0.4rem 0.6rem 0.5rem;
    position: relative;
    background: var(--bg-pane);
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .footer-row {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }
  .new-btn {
    gap: 6px;
    white-space: nowrap;
  }
  .keys,
  .list-count {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .keys kbd { margin-right: 2px; }
  .list-count { font-variant-numeric: tabular-nums; }
  .spacer { flex: 1; }

  .purge-btn {
    opacity: 0;
    transition: opacity var(--dur-base);
    color: var(--danger);
  }
  /* UX-04: `.icon-btn:disabled { opacity: 0.6 }` outranks `opacity: 0` here,
     so before this rule the purge button was INVISIBLE exactly when it
     worked and permanently visible when it did not (hub mode). A blocked
     destructive action must not be the most prominent thing in the row.

     Round-20 F10: the reveal rules are written with an explicit
     `:not(:disabled)` / `:disabled` pair rather than relying on source
     order, because CASCADE ORDER NEVER DECIDES THIS — specificity does. The
     first attempt at the keyboard path (`.proj-row:focus-within .purge-btn`,
     (0,3,0)) silently outranked `.purge-btn:disabled` ((0,2,0)) and lit the
     blocked trash at 0.6 on focus, brighter than the 0.35 chosen for hover:
     the very inversion UX-04/UX-133 removed. Hover and focus now share one
     pair, so the disabled button is never more visible than the enabled one
     in either path — and the enabled one keeps its keyboard affordance. */
  .purge-btn:disabled { opacity: 0; }
  .proj-row:hover .purge-btn:not(:disabled),
  .proj-row:focus-within .purge-btn:not(:disabled) {
    opacity: 0.6;
  }
  .proj-row:hover .purge-btn:disabled,
  .proj-row:focus-within .purge-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
  .purge-btn:hover:not(:disabled) { opacity: 1 !important; }
</style>
