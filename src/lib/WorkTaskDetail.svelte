<script lang="ts">
  // A task in Details (work graph M14), opened from the Work view: what the
  // tracker says (title, status, link, assignees, a description excerpt),
  // where its organisation and its group come from, its repositories, EVERY
  // session it has had — active, suggested, past, rejected — each with its
  // state and why, and the last outcome. Open / Continue / Start new act on
  // it; Place in group…, Assign org… (local tasks, with the impact dialog)
  // and Make a rule… (with a preview) correct it — in the rail's Placement,
  // with where the org and the group come from. The shared-work sections
  // (design 2026-09-29 §4: brief, subtasks, proposals, jobs; agent steps
  // after Sessions) are `TaskWorkSections`.
  //
  // Tracker text and Claude's summaries are third-party text: rendered as
  // Markdown through `MarkdownView` (a data tree, never HTML), so raw markup
  // stays literal text.
  import { onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions } from './sessions';
  import { selectSessionExplicitly, taskLinksLoaded } from './selection';
  import { orgs as orgStore } from './orgs';
  import { openExternal } from './open_external';
  import { shortAge, timeAgo } from './session_status';
  import { assessRow, hasReading, verdictLabel, verdictState } from './evidence';
  import StatusDot from './kit/StatusDot.svelte';
  import { changedAny, describeEvidence, onWorkChangedDebounced } from './work';
  import { providerInfo, unavailableLabel } from './trackers';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import WorkPlaceDialog from './WorkPlaceDialog.svelte';
  import EditTaskDialog from './EditTaskDialog.svelte';
  import WorkOrgDialog from './WorkOrgDialog.svelte';
  import WorkRuleEditor from './WorkRuleEditor.svelte';
  import TaskWorkSections from './TaskWorkSections.svelte';
  import TaskAttachments from './TaskAttachments.svelte';
  import { attachToWork, pastedImages } from './task_attachments';
  import TaskBlockedSpend from './TaskBlockedSpend.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import AiChangeLine from './AiChangeLine.svelte';
  import type { ProposalSource } from './ai_proposal';
  import { proposalFor } from './proposals';
  import WorkButton from './WorkButton.svelte';
  import { tablistKeys } from './tablist_keys';
  import MarkdownView from './MarkdownView.svelte';
  import Icon from './kit/Icon.svelte';
  import { copyText } from './clipboard';
  import { finishesWhen } from './handoffs';
  import { setWorkStatus, type WorkItemStatus } from './work';
  import { listStartRules, ruleProject, type StartRuleView } from './start_rules';
  import { projects } from './projects';
  import {
    activityEvents,
    COMMENT_MAX,
    commentAuthor,
    commentOnWork,
    deleteWorkComment,
    deliveryOf,
    hasDelivery,
    startRuleFor,
    subtaskProgress,
    TASK_TAB_LABELS,
    TASK_TABS,
    type TaskTab,
  } from './task_detail';
  import {
    dependencyName,
    groupSessionLinks,
    groupSourceText,
    occurrenceKind,
    orgSourceText,
    placeWork,
    placementNote,
    readErrorText,
    ruleDraftFor,
    selectedTaskId,
    taskStatus,
    trackerDown,
    trackerDownLabel,
    workRules,
    workTask,
    workTreeMeta,
    type TaskDetail,
    type WorkRule,
    type WorkRuleDraft,
    type WorkTask,
    type WorkTaskLink,
  } from './work_view';
  import type { IpcError } from './result';

  let {
    taskId,
    onclose,
    closeLabel = 'Close',
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
  }: { taskId: string; onclose?: () => void; closeLabel?: string; debounceMs?: number } = $props();

  const editBlocked = $derived(hubActionBlocked('edit_work_item', $hubStatus, $hubConnection));
  const statusBlocked = $derived(hubActionBlocked('set_work_status', $hubStatus, $hubConnection));
  const placeBlocked = $derived(hubActionBlocked('place_work', $hubStatus, $hubConnection));
  const orgBlocked = $derived(hubActionBlocked('assign_work_org', $hubStatus, $hubConnection));
  const ruleBlocked = $derived(hubActionBlocked('save_work_rule', $hubStatus, $hubConnection));

  let detail = $state<TaskDetail | null>(null);
  let error = $state<IpcError | null>(null);
  // A re-read that failed while the task is shown: the detail stays, with a
  // line to retry (the full error is for a first load only).
  let refreshError = $state<IpcError | null>(null);
  let loading = $state(false);
  let rules = $state<WorkRule[]>([]);
  let placing = $state(false);
  let editing = $state(false);
  let assigning = $state(false);
  let ruleDraft = $state<WorkRuleDraft | null>(null);

  let loadSeq = 0;
  async function load(id: string) {
    const mine = ++loadSeq;
    loading = true;
    const r = await workTask(id);
    if (mine !== loadSeq) return;
    loading = false;
    if (!r.ok) {
      // A task that is gone (or no longer visible) is gone whatever was shown.
      if (detail && r.error.code !== 'E_NOTFOUND' && r.error.code !== 'E_FORBIDDEN') {
        refreshError = r.error;
        return;
      }
      error = r.error;
      refreshError = null;
      detail = null;
      return;
    }
    error = null;
    refreshError = null;
    detail = r.value && r.value.task ? r.value : null;
    // A bare key a sync bound to a ticket answers as the ticket: follow it,
    // so the selection survives.
    const now = detail?.task.task_id;
    if (now && now !== id && (detail?.aliases ?? []).includes(id)) selectedTaskId.set(now);
    // Picked without its links (a subtask, a review row): its live session opens now.
    if (now) taskLinksLoaded(now, detail?.task.sessions ?? []);
  }

  // K5 (redesign 6.9): the group Jev proposes for a task no person and no
  // rule placed. A proposal only; a person's click places it.
  const groupProposal = $derived.by(() => {
    const t = detail?.task;
    if (!t) return null;
    if (t.group?.source === 'manual' || t.group?.source === 'rule') return null;
    return proposalFor(t, 'work_placement');
  });
  let placingProposed = $state(false);
  /** Why the proposed placement failed, shown under the proposal. */
  let proposalError = $state<string | null>(null);
  async function placeProposed(label: string) {
    const t = detail?.task;
    if (!t || placingProposed) return;
    placingProposed = true;
    proposalError = null;
    const source = groupProposal?.source ?? 'jev';
    const r = await placeWork(t.task_id, label, t.placement_version ?? 0);
    placingProposed = false;
    if (r.ok) {
      placed(r.value, null);
      // G7.15: a change AI proposed reads as the AI patterns board's line,
      // with Undo (the placement cleared, at the version this one left).
      placedByProposal = { task: t.task_id, label, source, version: r.value.placement_version ?? 0 };
    } else proposalError = readErrorText(r.error);
  }

  /** The placement the person took from Jev's proposal, while it stands. */
  let placedByProposal = $state<{ task: string; label: string; source: ProposalSource; version: number } | null>(null);
  async function undoProposedPlacement() {
    const p = placedByProposal;
    if (!p || placingProposed) return;
    placingProposed = true;
    proposalError = null;
    const r = await placeWork(p.task, '', p.version);
    placingProposed = false;
    if (r.ok) {
      placed(r.value, null);
      placedByProposal = null;
    } else proposalError = readErrorText(r.error);
  }

  // A placement saved: show the task and its placement line as the hub
  // answered at once (not the old "Placed …" until the next read). The
  // write's own bump re-reads the whole detail (debounced): one read, not a
  // second one here.
  function placed(t: WorkTask, note: string | null) {
    if (detail) {
      const version = t.placement_version ?? 0;
      detail = {
        ...detail,
        task: { ...t, sessions: t.sessions ?? detail.task.sessions },
        placement:
          version > 0 ? { group: t.group?.label ?? '', note, version, updated_at: null, updated_by: null } : null,
      };
    }
  }

  let loadedFor: string | null = null;
  $effect(() => {
    const id = taskId;
    if (id === loadedFor) return;
    loadedFor = id;
    detail = null;
    error = null;
    refreshError = null;
    void load(id);
  });

  async function loadRules() {
    const r = await workRules();
    rules = r.ok && Array.isArray(r.value) ? r.value : [];
  }
  void loadRules();

  // The rules are re-read only when a rule may have moved; the task on
  // anything but a saved view's change.
  const off = onWorkChangedDebounced((kinds) => {
    if ([...kinds].some((k) => k !== 'view')) void load(taskId);
    if (changedAny(kinds, 'rule', 'resync', 'local')) void loadRules();
  }, () => debounceMs);
  onDestroy(off);

  const task: WorkTask | null = $derived(detail?.task ?? null);

  // Redesign 6.3: what a blocked task waits for, named by key. The tasks
  // are read once each (at most a few), so the line and its links say
  // "TASK-212" rather than an item id.
  let deps = $state<Map<string, WorkTask>>(new Map());
  const depIds = $derived(task?.blocked ? (task.blocked_by ?? []).slice(0, 5) : []);
  $effect(() => {
    const ids = depIds;
    if (ids.length === 0) return;
    let live = true;
    void Promise.all(ids.map((id) => workTask(id))).then((rs) => {
      if (!live) return;
      const m = new Map<string, WorkTask>();
      rs.forEach((r, i) => {
        if (r.ok && r.value?.task) m.set(ids[i], r.value.task);
      });
      deps = m;
    });
    return () => {
      live = false;
    };
  });
  const depById = (id: string) => deps.get(id);
  const grouped = $derived(groupSessionLinks(task?.sessions ?? []));
  // A coarse clock for the Result chips' staleness (minute-level is plenty).
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 60_000);
    return () => clearInterval(t);
  });
  // The placement rule that put the task in its group: its "its sessions
  // start here" host and account apply when no start rule decides (G7.1).
  const placingRule = $derived(
    task?.group?.source === 'rule' && task.group.rule_id != null ? (rules.find((r) => r.id === task.group.rule_id && r.enabled) ?? null) : null,
  );
  const ruleName = $derived(task?.group?.rule_id != null ? (rules.find((r) => r.id === task.group.rule_id)?.name ?? null) : null);
  const matchingRules = $derived((detail?.rules ?? []).map((id) => rules.find((r) => r.id === id)?.name ?? `rule ${id}`));

  function orgName(id: number | null | undefined): string {
    if (id == null) return 'Unassigned';
    return (
      $workTreeMeta.orgs.find((o) => o.id === id)?.name ?? $orgStore.find((o) => o.id === id)?.name ?? `Organisation ${id}`
    );
  }

  function stateLabel(l: WorkTaskLink): string {
    switch (occurrenceKind(l)) {
      case 'primary':
        return 'active · primary';
      case 'secondary':
        return 'active · secondary';
      case 'suggested':
        return 'suggested';
      case 'rejected':
        return 'rejected';
      default:
        return l.ended_at ? `ended ${timeAgo(l.ended_at)}` : 'ended';
    }
  }

  function openLink(l: WorkTaskLink) {
    const row = l.session_id != null ? get(sessions).find((r) => r.id === l.session_id) : undefined;
    if (row) selectSessionExplicitly(row);
  }

  // G3.4: the page is tabbed (Overview, Sessions, Activity, Comments); the
  // tab stays as the selection moves from task to task.
  let tab = $state<TaskTab>('overview');

  // Activity's dated lines: sessions that started, were suggested, turned
  // down or stopped, and comments.
  const events = $derived(detail ? activityEvents(detail) : []);

  // Comments: kept in fleet, never sent to a tracker; deleting is the
  // author's own.
  const commentBlocked = $derived(hubActionBlocked('comment_on_work', $hubStatus, $hubConnection));
  const uncommentBlocked = $derived(hubActionBlocked('delete_work_comment', $hubStatus, $hubConnection));
  let draft = $state('');
  let commenting = $state(false);
  let commentError = $state<string | null>(null);
  let confirmDelete = $state<number | null>(null);
  async function postComment() {
    const t = detail?.task;
    if (!t || t.item_id == null || commenting || !draft.trim()) return;
    commenting = true;
    commentError = null;
    const r = await commentOnWork(t.item_id, draft);
    commenting = false;
    if (!r.ok) {
      commentError = readErrorText(r.error);
      return;
    }
    draft = '';
    attachedNote = null;
    // Shown at once; the write's bump re-reads the whole detail.
    if (detail) detail = { ...detail, comments: [...(detail.comments ?? []), { ...r.value, mine: true }] };
  }
  // An image pasted into the composer goes to the task's Attachments; the
  // comment itself stays text.
  let pageEl = $state<HTMLElement | null>(null);
  const attachBlocked = $derived(hubActionBlocked('attach_to_work', $hubStatus, $hubConnection));
  let attachedNote = $state<string | null>(null);
  async function pasteIntoComposer(e: ClipboardEvent) {
    const itemId = detail?.task.item_id;
    const files = pastedImages(e);
    if (itemId == null || files.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    if (attachBlocked) {
      commentError = attachBlocked;
      return;
    }
    commentError = null;
    for (const f of files) {
      const r = await attachToWork(itemId, f);
      if (!r.ok) {
        commentError = readErrorText(r.error);
        continue;
      }
      attachedNote = `Attached ${r.value.name}`;
      if (detail) detail = { ...detail, attachments: [{ ...r.value, mine: true }, ...(detail.attachments ?? [])] };
    }
  }
  async function removeComment(id: number) {
    confirmDelete = null;
    commentError = null;
    const r = await deleteWorkComment(id);
    if (!r.ok) {
      commentError = readErrorText(r.error);
      return;
    }
    if (detail) detail = { ...detail, comments: (detail.comments ?? []).filter((c) => c.id !== id) };
  }

  // Inline status (a native task's own): set here, without the edit dialog.
  let statusError = $state<string | null>(null);
  let statusBusy = $state(false);
  async function pickStatus(next: WorkItemStatus) {
    const t = task;
    if (!t || t.item_id == null || statusBusy || next === t.status_category) return;
    statusBusy = true;
    statusError = null;
    const r = await setWorkStatus(t.item_id, next);
    statusBusy = false;
    if (!r.ok) statusError = readErrorText(r.error);
    else void load(taskId);
  }

  // The Delivery block and the start rule that would place a start.
  const rowsById = $derived(new Map($sessions.map((r) => [r.id, r])));
  const delivery = $derived(task ? deliveryOf(task, detail?.last_outcome, rowsById, nowSec) : null);
  let startRules = $state.raw<StartRuleView[]>([]);
  void listStartRules().then((r) => {
    if (r.ok && Array.isArray(r.value)) startRules = r.value;
  });
  const startRule = $derived(startRuleFor(task?.key, startRules));

  // Prefilled for the group it is in now (none: the editor asks).
  const makeRuleDraft = (t: WorkTask): WorkRuleDraft =>
    ruleDraftFor(t, t.group && t.group.source !== 'none' ? t.group.label : '');
  // The header's key Copy: a check for a moment.
  let keyCopied = $state(false);
  async function copyKey() {
    const k = task?.key;
    if (!k || !(await copyText(k))) return;
    keyCopied = true;
    setTimeout(() => (keyCopied = false), 1500);
  }
  /** Where the task comes from: local work, a bare key, or its tracker. */
  const sourceLabel = $derived(
    !task ? '' : task.kind === 'local' ? 'local work' : task.kind === 'ref' ? 'bare key' : (providerInfo(task.provider)?.label ?? task.provider ?? 'tracker'),
  );
  /** A comment author's mark: the first letter of the name. */
  const initialOf = (name: string): string => ([...name.trim()][0] ?? '?').toUpperCase();
  /** "2 / 4" in the header chips, with any subtasks. */
  const subtaskCount = $derived(subtaskProgress(detail?.subtasks));
  /** The task's own "finishes when" (G7.6), as one line. */
  const finishes = $derived.by(() => {
    const s = finishesWhen(task?.done_when);
    return s ? s.charAt(0).toUpperCase() + s.slice(1) : '';
  });
</script>

<!-- The page (design 2026-10-10, task detail redesign): a header (where the
     task lives, its key with Copy, the title, its state chips and the one
     Start button), the tabs, then a reading column (what to do) beside a
     properties rail (where it stands: details, delivery, where it starts,
     placement). The rail drops under the column in a narrow pane. -->
<section class="task-detail" data-testid="work-task-detail" aria-label="Task" bind:this={pageEl}>
  <header class="head">
    {#if task}
      <nav class="crumbs" aria-label="Where this task lives" data-testid="work-task-crumbs">
        <span>{orgName(task.org_id)}</span>
        {#if task.group && task.group.source !== 'none' && task.group.label}<span class="sep" aria-hidden="true">/</span><span
            >{task.group.label}</span
          >{/if}
        {#if task.mission}<span class="sep" aria-hidden="true">/</span><span data-testid="work-task-mission"
            >{task.mission.name}{#if task.mission.wave != null}&nbsp;· W{task.mission.wave}{/if}</span
          >{/if}
        {#if task.parent_task_id}<span class="sep" aria-hidden="true">/</span><button
            class="crumb-link"
            type="button"
            data-testid="work-task-parent"
            onclick={() => task.parent_task_id && selectedTaskId.set(task.parent_task_id)}>Parent task</button
          >{/if}
      </nav>
    {/if}
    <div class="head-row">
      <div class="titlebox">
        {#if task}
          <div class="keyline">
            {#if task.key}
              <span class="key" data-testid="work-task-key">{task.key}</span>
              <button class="icon-btn" type="button" data-testid="work-task-copy-key" title="Copy {task.key}" onclick={() => void copyKey()}
                >{#if keyCopied}<Icon name="check" size={12} label="Copied" />{:else}<Icon name="copy" size={12} label="Copy key" />{/if}</button
              >
            {/if}
            <span class="badge" title={task.provider ?? task.kind}>{sourceLabel}</span>
            {#if task.tracker_name}<span class="muted">{task.tracker_name}</span>{/if}
          </div>
        {/if}
        <h2>
          <span class="title" class:unavailable={task?.unavailable}>{task ? task.title || (task.key ? '' : task.task_id) : 'Task'}</span>
        </h2>
      </div>
      <div class="head-actions">
        {#if task}<WorkButton {task} variant="bar" />{/if}
        {#if task?.kind === 'local' && task.item_id != null}
          <button
            class="btn"
            type="button"
            data-testid="work-task-edit"
            disabled={editBlocked !== null}
            title={editBlocked ?? 'Edit the title, description, status and assignees'}
            onclick={() => (editing = true)}><Icon name="edit" size={12} />Edit…</button
          >
        {/if}
        {#if task?.url}
          <button
            class="btn btn--quiet"
            type="button"
            data-testid="work-task-open-url"
            title="Open in {providerInfo(task.provider)?.label ?? 'its tracker'}"
            onclick={() => void openExternal(task.url ?? '')}><Icon name="link" size={12} />Open in tracker</button
          >
        {/if}
        <button
          class="btn btn--quiet icon-only"
          type="button"
          disabled={loading}
          data-testid="work-task-refresh"
          title="Refresh"
          aria-label="Refresh"
          onclick={() => void load(taskId)}><Icon name="retry" size={14} /></button
        >
        {#if onclose}
          <button class="btn btn--quiet" type="button" data-testid="work-task-close" onclick={onclose}>{closeLabel}</button>
        {/if}
      </div>
    </div>
    {#if task}
      <div class="chips">
        {#if task.kind === 'local' && task.item_id != null}
          <select
            class="status-pick status-pick--{task.status_category ?? 'todo'}"
            aria-label="Status"
            data-testid="work-task-status-pick"
            value={task.status_category ?? 'todo'}
            disabled={statusBlocked !== null || statusBusy}
            title={statusBlocked ?? 'Set the status'}
            onchange={(e) => void pickStatus((e.currentTarget as HTMLSelectElement).value as WorkItemStatus)}
          >
            <option value="todo">To do</option>
            <option value="in_progress">In progress</option>
            <option value="done">Done</option>
          </select>
        {:else if taskStatus(task)}
          <span class="chip chip--status chip--{task.status_category ?? 'todo'}" data-testid="work-task-status">{taskStatus(task)}</span>
        {/if}
        {#if task.resolution}<span class="chip">{task.resolution}</span>{/if}
        {#if subtaskCount}<span class="chip" data-testid="work-task-subtask-chip" title="Subtasks done">✓ {subtaskCount}</span>{/if}
        <TaskBlockedSpend {task} lookup={depById} testid="work-task" />
        {#if trackerDown(task)}<span class="chip chip--warn" data-testid="work-task-tracker-down"
            >{trackerDownLabel(task)} — what is shown is the last sync</span
          >{/if}
      </div>
    {/if}
  </header>

  {#if error}
    <div class="error" role="alert" data-testid="work-task-error">
      <p>{error.code === 'E_NOTFOUND' ? 'This task no longer exists, or is not visible from here.' : readErrorText(error)}</p>
      <button class="btn" type="button" onclick={() => void load(taskId)}>Retry</button>
    </div>
  {:else if !task}
    <p class="muted pad" data-testid="work-task-loading">Loading…</p>
  {:else}
    {#if refreshError}
      <p class="warn pad" role="status" data-testid="work-task-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="work-task-refresh-retry" onclick={() => void load(taskId)}>Retry</button>
      </p>
    {/if}
    {#if statusError}<p class="warn pad" role="alert" data-testid="work-task-status-error">{statusError}</p>{/if}
    <div class="tabs" role="tablist" aria-label="Task" use:tablistKeys>
      {#each TASK_TABS as t (t)}
        <button
          type="button"
          role="tab"
          class="tab"
          aria-selected={tab === t}
          data-testid="work-task-tab-{t}"
          onclick={() => (tab = t)}
          >{TASK_TAB_LABELS[t]}{#if t === 'sessions' && (task.sessions ?? []).length > 0}<span class="tab-count"
              >{(task.sessions ?? []).length + (task.sessions_more ?? 0)}</span
            >{:else if t === 'comments' && (detail?.comments ?? []).length > 0}<span class="tab-count">{(detail?.comments ?? []).length}</span
            >{/if}</button
        >
      {/each}
    </div>

    {#if tab === 'overview'}
      <div class="body">
        <div class="main">
          {#if task.unavailable}
            <p class="callout callout--warn" data-testid="work-task-unavailable">Unavailable: {unavailableLabel(task.unavailable_reason)}</p>
          {/if}
          {#if task.blocked && depIds.length > 0}
            <p class="callout" data-testid="work-task-waits-for">
              <strong>Waits for</strong>
              {#each depIds as id (id)}
                <button class="btn btn--quiet" type="button" data-testid="work-task-dependency" onclick={() => selectedTaskId.set(id)}
                  >{dependencyName(id, deps.get(id))}</button
                >
              {/each}
            </p>
          {/if}
          {#if detail?.description}
            <section aria-label="Description">
              <h3>Description{#if task.tracker_name}<span class="hint">from {task.tracker_name}</span>{/if}</h3>
              <div class="prose" data-testid="work-task-description"><MarkdownView source={detail.description} /></div>
              {#if detail.description_truncated && detail.description_chars}
                <p class="muted small" data-testid="work-task-description-cut">
                  Shown {[...detail.description].length} of {detail.description_chars} characters{#if task.url}&nbsp;—
                    <button class="link-btn" type="button" data-testid="work-task-description-open" onclick={() => void openExternal(task.url ?? '')}
                      >open the ticket</button
                    >{:else}&nbsp;— open the ticket in its tracker for the rest{/if}
                </p>
              {/if}
            </section>
          {/if}
          {#if finishes}
            <section aria-label="Finishes when">
              <h3>Finishes when</h3>
              <p class="line" data-testid="work-task-finishes">{finishes}</p>
            </section>
          {/if}
          {#if detail}<TaskWorkSections {detail} part="work" />{/if}
          {#if detail && task.item_id != null}
            <TaskAttachments
              itemId={task.item_id}
              attachments={detail.attachments ?? []}
              pasteTarget={pageEl}
            />
          {/if}
        </div>

        <aside class="rail" aria-label="Properties">
          <section aria-label="Details">
            <h3>Details</h3>
            <dl class="props">
              {#if (task.assignees ?? []).length > 0}
                <dt>Assignees</dt>
                <dd data-testid="work-task-assignees">{(task.assignees ?? []).join(', ')}{#if task.mine}&nbsp;(you){/if}</dd>
              {/if}
              <dt>Source</dt>
              <dd>{sourceLabel}{#if task.tracker_name}&nbsp;· {task.tracker_name}{/if}</dd>
              {#if (task.repos ?? []).length > 0}
                <dt>Repositories</dt>
                <dd><span data-testid="work-task-repos">{(task.repos ?? []).join(', ')}</span></dd>
              {:else if task.project_label}
                <dt>Project</dt>
                <dd>{task.project_label}</dd>
              {/if}
            </dl>
          </section>

          {#if delivery && hasDelivery(delivery)}
            <section class="delivery" data-testid="work-task-delivery" aria-label="Delivery">
              <h3>Delivery</h3>
              <dl class="props">
                {#if delivery.pr}
                  {@const pr = delivery.pr}
                  <dt>Pull request</dt>
                  <dd data-testid="work-task-delivery-pr">
                    <button class="link-btn" type="button" onclick={() => void openExternal(pr.url)}>{pr.label}</button>
                    {#if pr.checks === 'passing'}<span class="ok">✓ checks pass</span>{:else if pr.checks === 'failing'}<span class="bad"
                        >✕ {pr.failing} failing</span
                      >{:else if pr.checks === 'running'}<span class="muted">checks running</span>{/if}
                  </dd>
                {/if}
                {#if delivery.column}<dt>Tracker column</dt><dd data-testid="work-task-delivery-column">{delivery.column}</dd>{/if}
                {#if delivery.spend || delivery.duration}
                  <dt>Spent</dt>
                  <dd data-testid="work-task-delivery-spend">
                    {delivery.spend ?? '$0'}{#if delivery.duration}&nbsp;over {delivery.duration}{/if}
                  </dd>
                {/if}
                {#if delivery.owner}
                  <dt>Owner</dt>
                  <dd data-testid="work-task-delivery-owner" class:bad={delivery.owner.overdue}>{delivery.owner.text}</dd>
                {/if}
              </dl>
            </section>
          {/if}

          {#if startRule}
            <section aria-label="Starts in">
              <h3>Starts in</h3>
              <p class="line" data-testid="work-task-start-rule">
                <strong>{ruleProject(startRule, $projects.map((t) => t.project))}</strong>{#if startRule.host_alias}&nbsp;on {startRule.host_alias}{:else}&nbsp;on its last host{/if}{#if startRule.fallback_host}, else {startRule.fallback_host}{/if}
                <span class="muted small">· rule {startRule.pattern}</span>
              </p>
              {#if startRule.profile || startRule.model || startRule.effort || startRule.agent === 'codex'}
                <dl class="props" data-testid="work-task-start-launch">
                  {#if startRule.agent === 'codex'}<dt>Agent</dt><dd>Codex</dd>{/if}
                  {#if startRule.profile}<dt>Account</dt><dd data-testid="work-task-start-account">{startRule.profile}</dd>{/if}
                  {#if startRule.model || startRule.effort}
                    <dt>Model</dt>
                    <dd data-testid="work-task-start-model">{startRule.model ?? "host's default"}{#if startRule.effort}&nbsp;· effort {startRule.effort}{/if}</dd>
                  {/if}
                </dl>
              {/if}
            </section>
          {:else if placingRule && (placingRule.host_alias || placingRule.profile)}
            <section aria-label="Starts in">
              <h3>Starts in</h3>
              <p class="line" data-testid="work-task-placement-start">
                Sessions start on <strong>{placingRule.host_alias ?? 'its usual host'}</strong>{#if placingRule.profile}&nbsp;· account {placingRule.profile}{/if}
                <span class="muted small">· rule “{placingRule.name}”</span>
              </p>
            </section>
          {/if}

          <section aria-label="Placement">
            <h3>Placement</h3>
            <dl class="props">
              <dt>Organisation</dt>
              <dd data-testid="work-task-org">
                <strong>{orgName(task.org_id)}</strong> — {orgSourceText(task)}
              </dd>
              <dt>Group</dt>
              <dd data-testid="work-task-group">
                <strong>{task.group?.source === 'none' ? 'No group' : task.group?.label}</strong> — {groupSourceText(task.group, task, ruleName)}
                {#if placedByProposal && placedByProposal.task === task.task_id}
                  <AiChangeLine
                    what="Placed in {placedByProposal.label}"
                    source={placedByProposal.source}
                    onundo={() => void undoProposedPlacement()}
                    undoing={placingProposed}
                    undoBlocked={placeBlocked}
                    testid="work-task-group-ai-change"
                  />
                {/if}
                {#if groupProposal}
                  <div class="group-proposal" data-testid="work-task-group-proposal">
                    <span>Jev proposes “{groupProposal.value}”</span>
                    <ProposedBy proposal={groupProposal} field="work_placement" testid="work-task-group-proposed-by" />
                    <button
                      class="btn btn--quiet"
                      type="button"
                      data-testid="work-task-group-proposal-place"
                      disabled={placeBlocked !== null || placingProposed}
                      title={placeBlocked ?? `Put this task in “${groupProposal.value}”`}
                      onclick={() => groupProposal && void placeProposed(groupProposal.value)}>Place in {groupProposal.value}</button
                    >
                  </div>
                  {#if proposalError}
                    <p class="err" role="alert" data-testid="work-task-group-proposal-error">{proposalError}</p>
                  {/if}
                {/if}
              </dd>
            </dl>
            <details class="more" data-testid="work-task-more">
              <summary>Why here</summary>
              <div class="muted small" data-testid="work-task-group-note">{placementNote(task.group, task)}</div>
              {#if detail?.placement}
                <div class="muted small" data-testid="work-task-placement">
                  Placed{#if detail.placement.updated_by}&nbsp;by {detail.placement.updated_by}{/if}{#if detail.placement.updated_at}&nbsp;{timeAgo(detail.placement.updated_at)}{/if}{#if detail.placement.note}: “{detail.placement.note}”{/if}
                </div>
              {/if}
              {#if matchingRules.length > 0}
                <div class="muted small" data-testid="work-task-rules">Matching rules: {matchingRules.join(', ')}</div>
              {/if}
            </details>
            <div class="rail-actions">
              <button
                class="btn btn--quiet"
                type="button"
                data-testid="work-task-place"
                disabled={placeBlocked !== null}
                title={placeBlocked ?? 'Put this task under a group of the Work view (local to fleet)'}
                onclick={() => (placing = true)}>Place in group…</button
              >
              {#if task.kind === 'local'}
                <button
                  class="btn btn--quiet"
                  type="button"
                  data-testid="work-task-assign-org"
                  disabled={orgBlocked !== null}
                  title={orgBlocked ?? 'Move this task to another organisation (the impact is shown first)'}
                  onclick={() => (assigning = true)}>Assign org…</button
                >
              {/if}
              <button
                class="btn btn--quiet"
                type="button"
                data-testid="work-task-make-rule"
                disabled={ruleBlocked !== null}
                title={ruleBlocked ?? 'A rule for tasks like this one (previewed before it is saved)'}
                onclick={() => (ruleDraft = makeRuleDraft(task))}>Make a rule…</button
              >
            </div>
          </section>
        </aside>
      </div>
    {/if}

    {#if tab === 'sessions'}
      {#snippet linkList(list: WorkTaskLink[], label: string)}
        {#if list.length > 0}
          <h4>{label} <span class="tab-count">{list.length}</span></h4>
          <ul class="links">
            {#each list as l (l.link_id)}
              {@const kind = occurrenceKind(l)}
              <li class="link link--{kind}" data-testid="work-task-link" data-kind={kind}>
                <div class="lhead">
                  <span class="mark" aria-hidden="true">{kind === 'primary' ? '★' : kind === 'suggested' ? '?' : kind === 'past' ? '·' : '○'}</span>
                  {#if l.session_id != null && $sessions.some((r) => r.id === l.session_id) && kind !== 'past'}
                    <button class="name link-btn" type="button" data-testid="work-task-link-open" onclick={() => openLink(l)}
                      >{l.name ?? `session ${l.session_id}`}</button
                    >
                  {:else}
                    <span class="name">{l.name ?? `session ${l.session_id ?? l.link_id}`}</span>
                  {/if}
                  {#if l.host}<span class="muted">{l.host}</span>{/if}
                  <span class="state" data-testid="work-task-link-state">{stateLabel(l)}</span>
                  {#if l.cross_org}<span class="warn">cross-org</span>{/if}
                  {#if l.needs_you}<span class="warn">needs you</span>{/if}
                  {#if (l.other_tasks ?? 0) > 0}<span class="muted">+{l.other_tasks} other task{l.other_tasks === 1 ? '' : 's'}</span>{/if}
                </div>
                {#if l.why}<p class="why" data-testid="work-task-link-why">{l.why}</p>{/if}
                {#each l.evidence ?? [] as ev, i (i)}
                  <p class="evidence" data-testid="work-task-evidence">{describeEvidence(ev)}</p>
                {/each}
                <p class="small muted sub">
                  {#if l.branch}branch {l.branch} · {/if}{#if l.created_at}linked {timeAgo(l.created_at)}{/if}{#if l.end_reason} · ended: {l.end_reason}{/if}{#if l.resumable === false} · not resumable{/if}
                  {#if l.pr_url}
                    · <button class="link-btn" type="button" onclick={() => void openExternal(l.pr_url ?? '')}>PR</button>
                    {@const liveRow = kind !== 'past' && l.session_id != null ? $sessions.find((r) => r.id === l.session_id) : undefined}
                    {@const result = liveRow ? assessRow(liveRow, nowSec) : null}
                    {#if hasReading(result)}
                      · <span
                        class="result"
                        data-testid="work-task-link-result"
                        data-verdict={result.verdict}
                        title="Result of the session's PR; open the session for the reasons"
                        ><StatusDot state={verdictState(result.verdict)} label={null} size={6} /> {verdictLabel(result.verdict)}</span
                      >
                    {/if}
                  {/if}
                </p>
              </li>
            {/each}
          </ul>
        {/if}
      {/snippet}
      <div class="pane-body">
        {#if (task.sessions ?? []).length === 0}
          <p class="muted" data-testid="work-task-no-sessions">No session has worked on this task yet.</p>
        {/if}
        {@render linkList(grouped.active, 'Active')}
        {@render linkList(grouped.suggested, 'Suggested')}
        {@render linkList(grouped.past, 'Past')}
        {@render linkList(grouped.rejected, 'Rejected')}
        {#if (task.sessions_more ?? 0) > 0}
          <p class="muted">…and {task.sessions_more} more.</p>
        {/if}
      </div>
    {/if}

    {#if tab === 'activity'}
      <div class="pane-body">
        {#if !detail?.last_outcome && !detail?.placement && (detail?.steps ?? []).length === 0 && events.length === 0}
          <p class="muted" data-testid="work-task-no-activity">Nothing has happened on this task yet.</p>
        {/if}
        {#if events.length > 0}
          <ol class="events" data-testid="work-task-events">
            {#each events as e, i (i)}
              <li data-kind={e.kind}><span class="when">{timeAgo(e.at)}</span><span class="what">{e.text}</span></li>
            {/each}
          </ol>
        {/if}
        {#if detail?.placement}
          <p class="muted small" data-testid="work-task-activity-placed">
            Placed in {detail.placement.group}{#if detail.placement.updated_by}&nbsp;by {detail.placement.updated_by}{/if}{#if detail.placement.updated_at}&nbsp;{timeAgo(detail.placement.updated_at)}{/if}
          </p>
        {/if}

        {#if detail?.last_outcome}
          {@const o = detail.last_outcome}
          <section aria-label="Last outcome">
            <h3>Last outcome</h3>
            <div class="outcome" data-testid="work-task-outcome">
              <p class="small muted">
                {shortAge(o.at)}{#if o.name}&nbsp;· {o.name}{/if}{#if o.host}&nbsp;· {o.host}{/if}{#if o.branch}&nbsp;· branch {o.branch}{/if}
                {#if o.pr_url}
                  · <button class="link-btn" type="button" onclick={() => void openExternal(o.pr_url ?? '')}>PR</button>
                {/if}
              </p>
              {#if o.summary}<div class="prose"><MarkdownView source={o.summary} /></div>{/if}
            </div>
          </section>
        {/if}

        {#if detail}<TaskWorkSections {detail} part="steps" />{/if}
      </div>
    {/if}

    {#if tab === 'comments'}
      <div class="pane-body comments-pane">
        {#if (detail?.comments ?? []).length === 0}
          <p class="muted" data-testid="work-task-comments-empty">No comments yet. They stay in fleet; nothing is written to a tracker.</p>
        {:else}
          <ul class="comments">
            {#each detail?.comments ?? [] as c (c.id)}
              {@const who = commentAuthor(c)}
              <li class="comment" data-testid="work-task-comment">
                <span class="avatar" aria-hidden="true">{initialOf(who)}</span>
                <div class="cmain">
                  <div class="chead">
                    <strong>{who}</strong>
                    <span class="muted small">{timeAgo(c.created_at)}</span>
                    {#if c.mine}
                      <span class="cacts">
                        {#if confirmDelete === c.id}
                          <button class="btn btn--crit" type="button" data-testid="work-task-comment-delete-confirm" onclick={() => void removeComment(c.id)}
                            >Delete</button
                          >
                          <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = null)}>Keep</button>
                        {:else}
                          <button
                            class="btn btn--quiet"
                            type="button"
                            data-testid="work-task-comment-delete"
                            disabled={uncommentBlocked !== null}
                            title={uncommentBlocked ?? 'Delete your comment'}
                            onclick={() => (confirmDelete = c.id)}>Delete…</button
                          >
                        {/if}
                      </span>
                    {/if}
                  </div>
                  <div class="cbody prose"><MarkdownView source={c.body} /></div>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
        {#if task.item_id != null}
          <form
            class="composer"
            onsubmit={(e) => {
              e.preventDefault();
              void postComment();
            }}
          >
            <textarea
              rows="3"
              bind:value={draft}
              maxlength={COMMENT_MAX}
              placeholder="Add a comment: kept in fleet, never sent to the tracker"
              aria-label="Comment"
              data-testid="work-task-comment-input"
              onpaste={(e) => void pasteIntoComposer(e)}
              onkeydown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  void postComment();
                }
              }}
            ></textarea>
            <div class="composer-row">
              {#if commentError}<span class="err" role="alert" data-testid="work-task-comment-error">{commentError}</span>{/if}
              {#if attachedNote}<span class="muted small" role="status" data-testid="work-task-comment-attached">{attachedNote} · in Overview → Attachments</span>{/if}
              <span class="muted small hint-keys">Markdown · ⌘↵ to post</span>
              <button
                class="btn"
                type="submit"
                data-testid="work-task-comment-post"
                disabled={commentBlocked !== null || commenting || !draft.trim()}
                title={commentBlocked ?? 'Post (Ctrl/⌘+Enter)'}>{commenting ? 'Posting…' : 'Comment'}</button
              >
            </div>
          </form>
        {:else}
          <p class="muted small" data-testid="work-task-comments-bare-key">A bare key has no task in fleet to comment on yet.</p>
        {/if}
      </div>
    {/if}
  {/if}
</section>

{#if editing && task}
  <EditTaskDialog taskId={task.task_id} onclose={() => (editing = false)} ondone={() => void load(taskId)} />
{/if}

{#if placing && task}
  <WorkPlaceDialog
    {task}
    currentNote={detail?.placement?.note ?? null}
    onclose={() => (placing = false)}
    ondone={placed}
    onreload={() => void load(taskId)}
    onmakerule={(d) => (ruleDraft = d)}
  />
{/if}
{#if assigning && task}
  <WorkOrgDialog {task} onclose={() => (assigning = false)} ondone={() => void load(taskId)} />
{/if}
{#if ruleDraft}
  <WorkRuleEditor
    initial={ruleDraft}
    onclose={() => (ruleDraft = null)}
    onsaved={() => {
      void loadRules();
      void load(taskId);
    }}
    onundone={() => {
      void loadRules();
      void load(taskId);
    }}
  />
{/if}

<style>
  .events,
  .comments {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: var(--text-xs);
  }
  .events li {
    padding: 2px 0;
  }
  .events .when {
    display: inline-block;
    min-width: 5.5em;
  }
  .comment {
    padding: 4px 0;
    border-bottom: 1px solid var(--border);
  }
  .chead {
    display: flex;
    gap: 6px;
    align-items: baseline;
  }
  .cbody {
    margin: 2px 0 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .composer {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 6px;
  }
  .composer textarea {
    font: inherit;
    font-size: var(--text-xs);
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
    resize: vertical;
  }
  .composer-row {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
  }
  .task-detail {
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
    container-type: inline-size;
  }
  .pad {
    margin: var(--space-2) 0 0;
  }

  /* ── header ── */
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding-bottom: var(--space-3);
  }
  .crumbs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .crumbs .sep {
    opacity: 0.5;
  }
  .crumb-link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .crumb-link:hover {
    color: var(--fg);
    text-decoration: underline;
  }
  .head-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .titlebox {
    min-width: 0;
    flex: 1 1 320px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .keyline {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: var(--text-xs);
  }
  .key {
    font-family: var(--mono);
    color: var(--fg-muted);
  }
  .icon-btn {
    display: inline-grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  h2 {
    margin: 0;
    font-size: var(--text-xl);
    line-height: var(--text-xl-lh);
    font-weight: var(--text-xl-weight);
    text-wrap: balance;
  }
  .title {
    overflow-wrap: anywhere;
  }
  .title.unavailable {
    text-decoration: line-through;
  }
  .head-actions {
    display: flex;
    gap: var(--space-1);
    align-items: flex-start;
    flex-wrap: wrap;
  }
  .head-actions .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .icon-only {
    padding-inline: 5px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: var(--space-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    height: var(--control-h-sm);
    padding: 0 8px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    color: var(--fg-2);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .chip--status {
    font-weight: 600;
  }
  .chip--in_progress,
  .status-pick--in_progress {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .chip--done,
  .status-pick--done {
    background: var(--done-soft);
    color: var(--status-done);
  }
  .chip--warn {
    background: var(--waiting-soft);
    color: var(--usage-warn);
    white-space: normal;
  }
  .status-pick {
    font: inherit;
    font-size: var(--text-xs);
    font-weight: 600;
    height: var(--control-h-sm);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background-color: var(--chip-bg);
    color: var(--fg);
    padding: 0 4px;
  }
  .badge {
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
    padding: 0 0.3rem;
    color: var(--fg-muted);
  }

  /* ── tabs ── */
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
  }
  .tab {
    padding: 6px 10px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    cursor: pointer;
  }
  .tab[aria-selected='true'] {
    color: var(--fg);
    font-weight: 600;
    border-bottom-color: var(--accent);
  }
  .tab-count {
    margin-left: 4px;
    padding: 0 5px;
    border-radius: var(--radius-pill);
    background: var(--count-bg);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }

  /* ── body: reading column + properties rail ── */
  .body {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(220px, 280px);
    gap: var(--space-6);
    padding-top: var(--space-4);
  }
  @container (max-width: 720px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-4);
    }
  }
  .main,
  .rail,
  .pane-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }
  .pane-body {
    padding-top: var(--space-3);
    gap: var(--space-2);
  }
  .rail {
    gap: var(--space-3);
    align-self: start;
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-raise);
  }
  .rail > section + section {
    border-top: 1px solid var(--border);
    padding-top: var(--space-3);
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 6px;
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .hint {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
  }
  h4 {
    margin: 0.4rem 0 0.1rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .prose {
    max-width: var(--prose-max);
    line-height: 20px;
    overflow-wrap: anywhere;
  }
  .prose :global(p) {
    margin: 0 0 8px;
  }
  .prose :global(p:last-child) {
    margin-bottom: 0;
  }
  .callout {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 0;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
  }
  .callout--warn {
    background: var(--waiting-soft);
    color: var(--usage-warn);
  }
  .props {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 6px 10px;
    margin: 0;
    font-size: var(--text-xs);
  }
  .props dt {
    color: var(--fg-muted);
  }
  .props dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .rail .line {
    font-size: var(--text-xs);
    margin: 0 0 6px;
  }
  .rail-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: var(--space-2);
  }
  .ok {
    color: var(--status-done);
  }
  .bad {
    color: var(--status-failed);
  }
  .line {
    margin: 0;
  }
  .group-proposal {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
  }
  .more {
    margin-top: var(--space-2);
  }
  .more > summary {
    cursor: pointer;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .more[open] > summary {
    margin-bottom: 0.2rem;
  }
  .more > div + div {
    margin-top: 4px;
  }

  /* ── sessions ── */
  .links {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .link {
    padding: 0.4rem 0;
    border-bottom: 1px solid var(--border);
  }
  .link--suggested {
    border-bottom-style: dashed;
  }
  .link--past,
  .link--rejected {
    opacity: 0.7;
  }
  .lhead {
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .link--primary .mark {
    color: var(--accent);
  }
  .name {
    font-weight: 600;
  }
  .state {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .why,
  .evidence,
  .sub {
    margin: 0 0 0 1.1rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .link-btn {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
  }
  .small {
    font-size: var(--text-2xs);
    margin: 0;
  }
  .muted {
    color: var(--fg-muted);
  }
  .warn {
    color: var(--usage-warn);
  }
  .err,
  .error {
    color: var(--usage-crit);
  }
  .error {
    padding-top: var(--space-2);
  }
  .error p {
    margin: 0 0 0.4rem;
  }
  /* ── activity and comments ── */
  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    border-left: 2px solid var(--border);
  }
  .events li {
    display: flex;
    gap: var(--space-2);
    padding: 3px 0 3px var(--space-3);
    font-size: var(--text-xs);
  }
  .events .when {
    flex: none;
    min-width: 4.5em;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .comments-pane {
    max-width: var(--prose-max);
  }
  .comments {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .comment {
    display: grid;
    grid-template-columns: 24px minmax(0, 1fr);
    gap: var(--space-2);
    padding: var(--space-2) 0;
    border-bottom: 1px solid var(--border);
  }
  .avatar {
    width: 24px;
    height: 24px;
    border-radius: var(--radius-pill);
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: var(--text-2xs);
    font-weight: 600;
  }
  .chead {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .cacts {
    margin-left: auto;
    display: inline-flex;
    gap: 4px;
  }
  .cbody {
    margin-top: 2px;
  }
  .composer {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .composer:focus-within {
    border-color: var(--accent);
  }
  .composer textarea {
    font: inherit;
    resize: vertical;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--fg);
    min-height: 3.5em;
  }
  .composer-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .hint-keys {
    margin-left: auto;
  }
</style>
