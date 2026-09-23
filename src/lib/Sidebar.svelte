<script lang="ts">
  import { tick, untrack } from 'svelte';
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
    sameSession,
    hasNoPane,
    type SessionRow,
  } from './sessions';
  import { describePurge, purgeHostsForProject } from './purge';
  import { sessionMatchesSearch } from './search';
  import { type ProjectRow } from './projects';
  import { selectedSession, selectSession, selectSessionExplicitly, revealSeq } from './selection';
  import { forgetSessionUi } from './session_ui';
  import { applySessionRename, renameKeyHandler } from './session_rename';
  import { readPref, writePref } from './prefs';
  import { theme, cycleTheme } from './theme';
  import NewSessionDialog from './NewSessionDialog.svelte';
  import AddProjectDialog from './AddProjectDialog.svelte';
  import SettingsDialog from './SettingsDialog.svelte';
  import OnboardingCard from './OnboardingCard.svelte';
  import { hostFilter } from './hosts';
  import { onboardingDismissed } from './onboarding';
  import {
    hostsViewOpen,
    newSessionHostRequest,
    requestHostsView,
    settingsOpen,
  } from './app_views';
  import { hintAnchor } from './hints';
  import {
    buildSessionsByProject,
    buildOutsideFleet,
    buildRelatedCountById,
    sessionVisible,
    sortProjectsBySeverity,
    type SessionPredicate,
  } from './sidebar_index';
  import {
    countNeedsYou,
    needsYou,
    worstSeverityByProject,
  } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { push, pushError } from './toasts';
  import { hubStatus, hubBlock } from './hub';
  import { hubConnection, connectionBanner } from './hub_connection';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import BulkPromptDialog from './BulkPromptDialog.svelte';
  import TasksPanel from './TasksPanel.svelte';
  import SidebarFilters from './SidebarFilters.svelte';
  import SessionRowItem from './SessionRowItem.svelte';
  import NewBgSessionDialog from './NewBgSessionDialog.svelte';
  import { isRecency, matchesRecency, type Recency } from './session_status';

  let showTasks = $state(false);

  // Optional collapse handler injected by the parent (App.svelte). When
  // present, a ‹ button appears in the sidebar header so the user can
  // hide the whole sidebar to make room for the terminal.
  let { onCollapse }: { onCollapse?: () => void } = $props();

  let loadError: string | null = $state(null);
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
  const attentionOpts = $derived({ idleSecs: $attentionIdleMinutes * 60, now: nowSec });
  const rowPredicate = $derived.by((): SessionPredicate => {
    if (!needsYouOnly) return null;
    const opts = attentionOpts;
    return (s) => needsYou(s, opts);
  });

  // Multi-select for bulk Kill / Send prompt. Rows are toggled with
  // shift/cmd/ctrl-click, or with the checkboxes once select mode is on.
  let selectMode = $state(false);
  let selectedIds: Set<number> = $state(new Set());
  let bulkKillOpen = $state(false);
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
  // Drop ids whose rows left the store (killed / reaped) so the bulk bar
  // never counts phantoms.
  $effect(() => {
    const live = new Set($sessions.map((s) => s.id));
    if ([...selectedIds].some((id) => !live.has(id))) {
      selectedIds = new Set([...selectedIds].filter((id) => live.has(id)));
    }
  });

  async function confirmBulkKill() {
    bulkKillOpen = false;
    const targets = selectedRows;
    clearSelected();
    const results = await Promise.allSettled(
      targets.map(async (sess) => {
        const r = await killSession(sess.host_alias, sess.tmux_name);
        if (!r.ok) {
          pushError(r.error, `Kill ${sess.tmux_name} failed`);
          return;
        }
        forgetSessionUi(sess.host_alias, sess.tmux_name);
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
      if ($hostFilter !== 'all' && $hostFilter !== sess.host_alias) hostFilter.set('all');
      expandAndScrollTo(sess);
    });
  });

  // Stores are bootstrapped once by App.svelte's onMount; Sidebar just reads
  // them. (A second bootstrap here would double every startup IPC call.)

  async function onRefresh() {
    loading = true;
    loadError = null;
    const pr = await refreshProjects();
    // Explicit user refresh: bypass the backend's freshness window.
    const sr = await loadSessions({ force: true });
    loading = false;
    if (!pr.ok) {
      loadError = pr.error.message;
      pushError(pr.error, 'Refresh projects failed');
    } else if (!sr.ok) {
      loadError = sr.error.message;
      pushError(sr.error, 'Refresh sessions failed');
    }
  }

  function matchesSearch(p: ProjectTreeRow, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (p.project.owner.toLowerCase().includes(needle)) return true;
    if (p.project.repo.toLowerCase().includes(needle)) return true;
    return sessionsForProject(p.project.id).some((s) => sessionMatchesSearch(s, needle));
  }

  // Sessions under the host / bg filters only (no triage predicate): the
  // counters must keep reporting while a triage filter is active, and the
  // project sort must weigh every visible session, not just the filtered ones.
  const hostVisibleSessions = $derived(
    $sessions.filter((s) => sessionVisible(s, $hostFilter, $showBgAgents)),
  );
  // countNeedsYou() classifies each row, and classify() files an external
  // (Outside fleet) row as working/idle, so a read-only row never inflates
  // the pill (spec §5).
  const needsYouTotal = $derived(countNeedsYou(hostVisibleSessions, attentionOpts));
  const severityByProject = $derived(worstSeverityByProject(hostVisibleSessions));

  // Only show projects that either match the filter directly OR have at least
  // one active session. Without sessions the sidebar would be flooded with
  // every cloned repo on disk — most of which the user isn't working on.
  const filtered = $derived(
    sortProjectsBySeverity(
      $projects.filter(
        (p) =>
          matchesRecency(p, recency) &&
          matchesSearch(p, searchQuery) &&
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

  // Map: project_id → sessions filtered by current hostFilter. This derived
  // value is read directly in the template so Svelte tracks it reactively —
  // using a plain function via {@const} doesn't establish the dependency.
  const filteredSessionsByProject = $derived(
    buildSessionsByProject($sessions, $hostFilter, $showBgAgents, rowPredicate),
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
  const orphanSessions = $derived(
    $sessions.filter(
      (s) =>
        s.project_id === null &&
        s.kind !== 'external' &&
        sessionVisible(s, $hostFilter, $showBgAgents, rowPredicate),
    ),
  );

  // Interactive Claude sessions running entirely outside fleet (Claude
  // Desktop, a bare terminal). Read-only; the host filter applies but the
  // bg-agent toggle does not.
  const outsideFleet = $derived(buildOutsideFleet($sessions, $hostFilter));

  // Picker for the footer "+ New session" — shows ALL projects regardless
  // of the recency filter or search query. The filter is for the live-
  // sessions tree; when starting a new session the user shouldn't be
  // restricted to projects with recent activity.
  const allProjectsSorted = $derived(
    // System projects are filtered out: `fleet/operator` is the UX agent's
    // own working directory, not a repository, and starting an ordinary
    // session in it is never what "+ New session" means. The tree above
    // still shows it while the agent is running. See `ProjectRow.system`.
    [...$projects.filter((p) => !p.project.system)].sort((a, b) => {
      const aLabel = (a.project.owner + '/' + a.project.repo).toLowerCase();
      const bLabel = (b.project.owner + '/' + b.project.repo).toLowerCase();
      return aLabel.localeCompare(bLabel);
    }),
  );

  let dialogProject: ProjectTreeRow | null = $state(null);
  let showProjectPicker = $state(false);
  let showAddProject = $state(false);
  /** Host to preselect in NewSessionDialog: where Add project put the project. */
  let dialogHost: string | undefined = $state(undefined);

  // Both act on a checkout using this machine's SSH (and, for Add project,
  // GitHub credentials): neither has a hub tool, so both refuse with
  // E_LOCAL_ONLY in remote mode (`commands/projects.rs`, `commands/sessions.rs`).
  const addProjectBlocked = $derived(hubBlock('add_project', $hubStatus));
  const purgeProjectBlocked = $derived(hubBlock('purge_project', $hubStatus));

  // While the hub's wire contract is outside this build's range, every list
  // load fails with `E_HUB_CONTRACT` and never will heal itself (unlike
  // `reconnecting`/`offline`, where the stores already hold a real last-known
  // list). An empty tree then reads as "you have no projects" instead of
  // "this couldn't load" — so borrow the connection banner's own sentence for
  // this state rather than inventing a second wording.
  const hubSkewEmptyMessage = $derived(
    $hubConnection.state === 'hub_too_old' || $hubConnection.state === 'hub_too_new'
      ? connectionBanner($hubConnection, $hubStatus.url)
      : null,
  );

  /** Host the open project picker preselects (the Hosts view's `n`). */
  let pickerHost: string | undefined = $state(undefined);

  // Onboarding card actions — open the same flows as existing UI. Hosts are
  // managed in the Hosts view, not Settings.
  const openAddHost = () => requestHostsView();
  const openNewSession = () => {
    pickerHost = undefined;
    showProjectPicker = true;
  };

  function toggleProjectPicker() {
    pickerHost = undefined;
    showProjectPicker = !showProjectPicker;
  }

  // "New session on <host>" from the Hosts view: the same project picker,
  // then NewSessionDialog with that host preselected.
  $effect(() => {
    const host = $newSessionHostRequest;
    if (host === null) return;
    newSessionHostRequest.set(null);
    pickerHost = host;
    showProjectPicker = true;
    // Keyboard flow from the Hosts view: land on the first project.
    void tick().then(() => {
      const first =
        sidebarEl?.querySelector<HTMLElement>('.picker .picker-item:not(.add-project)') ??
        sidebarEl?.querySelector<HTMLElement>('.picker .picker-item');
      first?.focus();
    });
  });

  function openNew(p: ProjectTreeRow, e?: Event) {
    e?.stopPropagation();
    // A row's own `+` has no host intent; the picker may carry one.
    dialogHost = e ? undefined : pickerHost;
    pickerHost = undefined;
    dialogProject = p;
    showProjectPicker = false;
  }

  function openAddProject() {
    pickerHost = undefined;
    showProjectPicker = false;
    showAddProject = true;
  }

  // The user added a project in order to start a session in it: go straight
  // to NewSessionDialog on the new row (already merged into `projects`).
  function onProjectAdded(row: ProjectTreeRow, host: string) {
    showAddProject = false;
    dialogHost = host;
    dialogProject = row;
  }

  function onCreated(s: SessionRow) {
    dialogProject = null;
    // Auto-focus the just-created session in the center/terminal panes.
    selectSessionExplicitly(s);
  }

  function onCancel() {
    dialogProject = null;
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
   *  The row is a tabbable `role="button"` that CONTAINS real buttons (the
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
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelectSession(sess);
    }
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
    pendingKill = null;
    const r = await killSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) {
      pushError(r.error, 'Kill failed');
      return;
    }
    // Drop persisted layout for the now-dead session — otherwise localStorage
    // grows unbounded over time. (User can still get a fresh layout if they
    // make a session with the same name later; that's intentional.)
    forgetSessionUi(sess.host_alias, sess.tmux_name);
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

  // --- Purge Project ---
  let pendingPurge: ProjectRow | null = $state(null);

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

<div class="sidebar" data-testid="sidebar-tree" bind:this={sidebarEl}>
  {#snippet sessionRow(sess: SessionRow, readOnly = false)}
    <SessionRowItem
      {sess}
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
    />
  {/snippet}

  <SidebarFilters
    bind:search
    bind:recency
    bind:needsYouOnly
    {loading}
    {loadError}
    {onRefresh}
    {onCollapse}
    {showTasks}
    showSettings={$settingsOpen}
    onOpenTasks={() => (showTasks = true)}
    onOpenSettings={() => settingsOpen.set(true)}
    needsYouCount={needsYouTotal}
    {selectMode}
    {toggleSelectMode}
    selectedCount={selectedIds.size}
    onBulkSend={() => (bulkPromptOpen = true)}
    onBulkKill={() => (bulkKillOpen = true)}
    {clearSelected}
  />

  <div class="scroller">
    {#if !$onboardingDismissed}
      <OnboardingCard onaddhost={openAddHost} onnewsession={openNewSession} />
    {/if}
    {#if filtered.length > 0}
      <ul class="tree">
        {#each filtered as row (row.project.id)}
          {@const projectSessions = filteredSessionsByProject.get(row.project.id) ?? []}
          {@const isCollapsed = collapsed.has(row.project.id)}
          <li class="proj">
            <div
              class="proj-row"
              data-testid="proj-row"
              title={row.project.base_path}
              role="button"
              tabindex="0"
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
              >🗑️</button>
            </div>

            {#if !isCollapsed}
              {#each projectSessions as sess (sess.id)}
                {@render sessionRow(sess)}
              {/each}
            {/if}
          </li>
        {/each}
      </ul>
    {:else if !loadError && orphanSessions.length === 0}
      <p class="empty" data-testid="sidebar-empty">
        {hubSkewEmptyMessage ??
          ($projects.length === 0
            ? 'No projects yet. Set a projects base in Settings → Projects, or click ↻ to scan.'
            : 'No active sessions. Click + below to start one.')}
      </p>
    {/if}

    {#if orphanSessions.length > 0}
      <div class="orphan-section" data-testid="orphan-sessions">
        <div class="section-header">Other sessions ({orphanSessions.length})</div>
        {#each orphanSessions as sess (sess.id)}
          {@render sessionRow(sess)}
        {/each}
      </div>
    {/if}

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
          {#each outsideFleet as sess (sess.id)}
            {@render sessionRow(sess, true)}
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  <footer class="sidebar-footer" data-testid="sidebar-chrome-bottom">
    <div class="footer-row">
      <button
        class="new-btn"
        onclick={toggleProjectPicker}
        data-testid="new-session-footer"
      >
        + New session
      </button>
      <button
        class="icon-btn"
        title="Launch a supervised Claude background session"
        onclick={() => (showBgModal = true)}
        data-testid="new-bg-session-btn"
        use:hintAnchor={{ id: 'bg-session', when: $sessions.some((s) => !hasNoPane(s)) && !$sessions.some((s) => s.kind === 'bg') }}
      >⚡</button>
    </div>
    <button
      class="theme-toggle"
      onclick={cycleTheme}
      title="Theme: {$theme} (click to cycle auto/light/dark)"
      data-testid="theme-toggle"
    >
      theme: {$theme}
    </button>
    {#if showProjectPicker}
      <div class="picker" role="listbox" aria-label="Pick project for new session">
        <button
          class="picker-item add-project"
          disabled={addProjectBlocked !== null}
          title={addProjectBlocked ?? ''}
          onclick={openAddProject}
          data-testid="add-project-row"
        >
          ＋ Add project…
        </button>
        {#each allProjectsSorted as row (row.project.id)}
          <button class="picker-item" onclick={() => openNew(row)}>
            {#if collidingRepos.has(row.project.repo)}<span class="owner"
                >{row.project.owner}/</span
              >{/if}{row.project.repo}
          </button>
        {/each}
        {#if allProjectsSorted.length === 0}
          <p class="empty pad">No projects yet. Add one, or refresh.</p>
        {/if}
      </div>
    {/if}
  </footer>
</div>

<!-- The project picker is a popover, not a modal; the modals below handle
     their own Escape through <dialog>'s cancel event. -->
<svelte:window onkeydown={(e) => {
  if (e.key === 'Escape' && showProjectPicker) showProjectPicker = false;
}} />

{#if showAddProject}
  <AddProjectDialog onCreated={onProjectAdded} onCancel={() => (showAddProject = false)} />
{/if}

{#if dialogProject}
  <NewSessionDialog project={dialogProject} initialHost={dialogHost} onCreate={onCreated} {onCancel} />
{/if}

{#if pendingKill}
  <ConfirmDialog
    title="Kill session?"
    confirmLabel="Kill"
    danger
    onconfirm={confirmKill}
    oncancel={cancelKill}
    confirmTestId="confirm-kill"
  >
    This will kill the tmux session <code>{pendingKill.tmux_name}</code> on
    <code>{pendingKill.host_alias}</code> and lose any running claude state inside it. Continue?
  </ConfirmDialog>
{/if}

{#if bulkKillOpen}
  <ConfirmDialog
    title="Kill {selectedRows.length} session{selectedRows.length === 1 ? '' : 's'}?"
    confirmLabel="Kill all"
    danger
    onconfirm={confirmBulkKill}
    oncancel={() => (bulkKillOpen = false)}
    confirmTestId="confirm-bulk-kill"
  >
    This will kill
    {#each selectedRows as r, i (r.id)}{i > 0 ? ', ' : ''}<code>{r.tmux_name}</code> on <code>{r.host_alias}</code>{/each}
    and lose any running claude state inside them. Continue?
  </ConfirmDialog>
{/if}

{#if bulkPromptOpen}
  <BulkPromptDialog targets={selectedRows} onClose={() => (bulkPromptOpen = false)} />
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

{#if $settingsOpen}
  <SettingsDialog onClose={() => settingsOpen.set(false)} />
{/if}

{#if showTasks}
  <Modal title="Tasks" onclose={() => (showTasks = false)} width="640px" testid="tasks-dialog">
    <TasksPanel />
  </Modal>
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
    border-radius: 5px;
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    min-width: 1.6rem;
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }
  .icon-btn.small {
    padding: 0.1rem 0.35rem;
    font-size: 0.85rem;
    min-width: 1.4rem;
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
    font-size: 0.85rem;
    padding: 0.3rem 0.4rem;
    border-radius: 4px;
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
    font-size: 0.65rem;
    width: 0.7rem;
    text-align: center;
    transition: transform 0.1s ease;
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
  .count {
    font-size: 0.7rem;
    color: var(--fg-muted);
    padding: 0.05rem 0.4rem;
    border-radius: 999px;
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    min-width: 1.4rem;
    text-align: center;
  }

  .empty { color: var(--fg-muted); font-size: 0.85rem; padding: 0.5rem 0.4rem; }
  .pad { padding: 0.5rem 0.6rem; }

  .orphan-section {
    border-top: 1px solid var(--border);
    padding-top: 0.35rem;
    margin-top: 0.35rem;
  }
  .section-header {
    font-size: 0.65rem;
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
    font-size: 0.65rem;
    width: 0.7rem;
    text-align: center;
    transition: transform 0.1s ease;
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
    flex: 1;
    text-align: left;
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
  }
  .new-btn:hover { border-color: var(--accent); background: var(--bg-pane); }

  .purge-btn {
    opacity: 0;
    transition: opacity 0.15s;
    color: var(--color-error, #f44336);
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
  .theme-toggle {
    width: 100%;
    text-align: left;
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }

  .picker {
    position: absolute;
    bottom: 100%;
    left: 0;
    right: 0;
    margin-bottom: 0.3rem;
    border: 1px solid var(--border);
    background: var(--bg);
    border-radius: 5px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.3);
    max-height: 240px;
    overflow: auto;
    z-index: 5;
  }
  .picker-item {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--fg);
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    cursor: pointer;
  }
  .picker-item:hover { background: var(--bg-pane); }
  .picker-item.add-project { color: var(--accent); border-bottom: 1px solid var(--border); }
</style>
