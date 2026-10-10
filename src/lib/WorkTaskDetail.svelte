<script lang="ts">
  // A task in Details (work graph M14), opened from the Work view: what the
  // tracker says (title, status, link, assignees, a description excerpt),
  // where its organisation and its group come from, its repositories, EVERY
  // session it has had — active, suggested, past, rejected — each with its
  // state and why, and the last outcome. Open / Continue / Start new act on
  // it; Place in group…, Assign org… (local tasks, with the impact dialog)
  // and Make a rule… (with a preview) correct it — under the *Placement &
  // rules* disclosure, with where the org and the group come from. The
  // shared-work sections (design 2026-09-29 §4: brief, subtasks, proposals,
  // jobs; agent steps after Sessions) are `TaskWorkSections`.
  //
  // Tracker text and Claude's summaries are third-party text: rendered as
  // plain text, never as markup.
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
  import TaskBlockedSpend from './TaskBlockedSpend.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import AiChangeLine from './AiChangeLine.svelte';
  import type { ProposalSource } from './ai_proposal';
  import { proposalFor } from './proposals';
  import WorkButton from './WorkButton.svelte';
  import { tablistKeys } from './tablist_keys';
  import { setWorkStatus, type WorkItemStatus } from './work';
  import { listStartRules, ruleProject, type StartRuleView } from './start_rules';
  import { projects } from './projects';
  import { deliveryOf, hasDelivery, startRuleFor, TASK_TAB_LABELS, TASK_TABS, type TaskTab } from './task_detail';
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

  // G3.4: the page is tabbed (Overview, Sessions, Activity); the tab stays
  // as the selection moves from task to task.
  let tab = $state<TaskTab>('overview');

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
</script>

<section class="task-detail" data-testid="work-task-detail" aria-label="Task">
  <header>
    <h2>
      {#if task?.key}<span class="key">{task.key}</span>{/if}
      <span class="title" class:unavailable={task?.unavailable}>{task ? task.title || (task.key ? '' : task.task_id) : 'Task'}</span>
    </h2>
    <div class="head-actions">
      {#if task?.kind === 'local' && task.item_id != null}
        <select
          class="status-pick"
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
      {/if}
      {#if task?.kind === 'local' && task.item_id != null}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="work-task-edit"
          disabled={editBlocked !== null}
          title={editBlocked ?? 'Edit the title, description, status and assignees'}
          onclick={() => (editing = true)}>Edit…</button
        >
      {/if}
      <button class="btn btn--quiet" type="button" disabled={loading} data-testid="work-task-refresh" onclick={() => void load(taskId)}
        >Refresh</button
      >
      {#if onclose}
        <button class="btn btn--quiet" type="button" data-testid="work-task-close" onclick={onclose}>{closeLabel}</button>
      {/if}
    </div>
  </header>

  {#if error}
    <div class="error" role="alert" data-testid="work-task-error">
      <p>{error.code === 'E_NOTFOUND' ? 'This task no longer exists, or is not visible from here.' : readErrorText(error)}</p>
      <button class="btn" type="button" onclick={() => void load(taskId)}>Retry</button>
    </div>
  {:else if !task}
    <p class="muted" data-testid="work-task-loading">Loading…</p>
  {:else}
    {#if refreshError}
      <p class="warn" role="status" data-testid="work-task-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="work-task-refresh-retry" onclick={() => void load(taskId)}>Retry</button>
      </p>
    {/if}
    {#if statusError}<p class="warn" role="alert" data-testid="work-task-status-error">{statusError}</p>{/if}
    <div class="tabs" role="tablist" aria-label="Task" use:tablistKeys>
      {#each TASK_TABS as t (t)}
        <button
          type="button"
          role="tab"
          class="tab"
          aria-selected={tab === t}
          data-testid="work-task-tab-{t}"
          onclick={() => (tab = t)}
          >{TASK_TAB_LABELS[t]}{#if t === 'sessions' && (task.sessions ?? []).length > 0}<span class="tab-count">{(task.sessions ?? []).length + (task.sessions_more ?? 0)}</span>{/if}</button
        >
      {/each}
    </div>
    {#if tab === 'overview'}
    <div class="meta">
      <span class="badge" title={task.provider ?? task.kind}
        >{task.kind === 'local' ? 'local work' : task.kind === 'ref' ? 'bare key' : `${providerInfo(task.provider)?.label ?? task.provider ?? 'tracker'}`}</span
      >
      {#if task.tracker_name}<span>{task.tracker_name}</span>{/if}
      {#if taskStatus(task)}<span class="status" data-testid="work-task-status">{taskStatus(task)}</span>{/if}
      {#if task.resolution}<span class="muted">({task.resolution})</span>{/if}
      <TaskBlockedSpend {task} lookup={depById} testid="work-task" />
      {#if trackerDown(task)}<span class="warn" data-testid="work-task-tracker-down">{trackerDownLabel(task)} — what is shown is the last sync</span>{/if}
      {#if task.url}
        <button class="btn btn--quiet" type="button" data-testid="work-task-open-url" onclick={() => void openExternal(task.url ?? '')}
          >Open in tracker</button
        >
      {/if}
    </div>
    {#if task.blocked && depIds.length > 0}
      <p class="line" data-testid="work-task-waits-for">
        Waits for:
        {#each depIds as id (id)}
          <button class="btn btn--quiet" type="button" data-testid="work-task-dependency" onclick={() => selectedTaskId.set(id)}
            >{dependencyName(id, deps.get(id))}</button
          >
        {/each}
      </p>
    {/if}
    {#if task.unavailable}
      <p class="warn" data-testid="work-task-unavailable">Unavailable: {unavailableLabel(task.unavailable_reason)}</p>
    {/if}
    {#if (task.assignees ?? []).length > 0}
      <p class="line" data-testid="work-task-assignees">Assignees: {(task.assignees ?? []).join(', ')}{#if task.mine}&nbsp;(you){/if}</p>
    {/if}
    {#if detail?.description}
      <p class="excerpt" data-testid="work-task-description">{detail.description}</p>
      {#if detail.description_truncated && detail.description_chars}
        <p class="muted small" data-testid="work-task-description-cut">
          Shown {[...detail.description].length} of {detail.description_chars} characters{#if task.url}&nbsp;—
            <button class="btn btn--quiet" type="button" data-testid="work-task-description-open" onclick={() => void openExternal(task.url ?? '')}
              >open the ticket</button
            >{:else}&nbsp;— open the ticket in its tracker for the rest{/if}
        </p>
      {/if}
    {/if}

    {#if (task.repos ?? []).length > 0}
      <p class="line">Repositories: <span data-testid="work-task-repos">{(task.repos ?? []).join(', ')}</span></p>
    {/if}

    {#if delivery && hasDelivery(delivery)}
      <section class="delivery" data-testid="work-task-delivery" aria-label="Delivery">
        <h3>Delivery</h3>
        <dl>
          {#if delivery.pr}
            {@const pr = delivery.pr}
            <dt>Pull request</dt>
            <dd data-testid="work-task-delivery-pr">
              <button class="link-btn" type="button" onclick={() => void openExternal(pr.url)}>{pr.label}</button>
              {#if pr.checks === 'passing'}<span class="ok">✓ checks pass</span>{:else if pr.checks === 'failing'}<span class="bad">✕ {pr.failing} failing</span>{:else if pr.checks === 'running'}<span class="muted">checks running</span>{/if}
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
      <p class="line" data-testid="work-task-start-rule">
        Starts in <strong>{ruleProject(startRule, $projects.map((t) => t.project))}</strong>{#if startRule.host_alias}&nbsp;on {startRule.host_alias}{:else}&nbsp;on its last host{/if}{#if startRule.fallback_host}, else {startRule.fallback_host}{/if}
        <span class="muted small">· rule {startRule.pattern}</span>
      </p>
      {#if startRule.profile || startRule.model || startRule.effort || startRule.agent === 'codex'}
        <dl class="prov" data-testid="work-task-start-launch">
          {#if startRule.agent === 'codex'}<dt>Agent</dt><dd>Codex</dd>{/if}
          {#if startRule.profile}<dt>Account</dt><dd data-testid="work-task-start-account">{startRule.profile}</dd>{/if}
          {#if startRule.model || startRule.effort}
            <dt>Model</dt>
            <dd data-testid="work-task-start-model">{startRule.model ?? "host's default"}{#if startRule.effort}&nbsp;· effort {startRule.effort}{/if}</dd>
          {/if}
        </dl>
      {/if}
    {:else if placingRule && (placingRule.host_alias || placingRule.profile)}
      <p class="line" data-testid="work-task-placement-start">
        Sessions start on <strong>{placingRule.host_alias ?? 'its usual host'}</strong>{#if placingRule.profile}&nbsp;· account {placingRule.profile}{/if}
        <span class="muted small">· rule “{placingRule.name}”</span>
      </p>
    {/if}

    <details class="more" data-testid="work-task-more">
      <summary>Placement &amp; rules</summary>
      <dl class="prov">
        <dt>Organisation</dt>
        <dd data-testid="work-task-org">
          <strong>{orgName(task.org_id)}</strong> — {orgSourceText(task)}
        </dd>
        <dt>Group</dt>
        <dd data-testid="work-task-group">
          <strong>{task.group?.source === 'none' ? 'No group' : task.group?.label}</strong> — {groupSourceText(task.group, task, ruleName)}
          <div class="muted small" data-testid="work-task-group-note">{placementNote(task.group, task)}</div>
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
          {#if detail?.placement}
            <div class="muted small" data-testid="work-task-placement">
              Placed{#if detail.placement.updated_by}&nbsp;by {detail.placement.updated_by}{/if}{#if detail.placement.updated_at}&nbsp;{timeAgo(detail.placement.updated_at)}{/if}{#if detail.placement.note}: “{detail.placement.note}”{/if}
            </div>
          {/if}
          {#if matchingRules.length > 0}
            <div class="muted small" data-testid="work-task-rules">Matching rules: {matchingRules.join(', ')}</div>
          {/if}
        </dd>
      </dl>
      <div class="actions edits">
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
    </details>

    <!-- Redesign 6.6: the one split button every task start uses, with
         its progress ("Checkout", "Setting up…") under it. -->
    <div class="actions"><WorkButton {task} variant="bar" /></div>
    {#if detail}<TaskWorkSections {detail} part="work" />{/if}
    {/if}

    {#if tab === 'sessions'}
    {#snippet linkList(list: WorkTaskLink[], label: string)}
      {#if list.length > 0}
        <h4>{label}</h4>
        <ul class="links">
          {#each list as l (l.link_id)}
            {@const kind = occurrenceKind(l)}
            <li class="link link--{kind}" data-testid="work-task-link" data-kind={kind}>
              <div class="lhead">
                <span class="mark" aria-hidden="true">{kind === 'primary' ? '★' : kind === 'suggested' ? '?' : kind === 'past' ? '·' : '○'}</span>
                {#if l.session_id != null && $sessions.some((r) => r.id === l.session_id) && kind !== 'past'}
                  <button class="name link-btn" type="button" data-testid="work-task-link-open" onclick={() => openLink(l)}>{l.name ?? `session ${l.session_id}`}</button>
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
              <p class="small muted">
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
                    ><StatusDot state={verdictState(result.verdict)} label={null} size={6} /> {verdictLabel(result.verdict)}</span>
                  {/if}
                {/if}
              </p>
            </li>
          {/each}
        </ul>
      {/if}
    {/snippet}
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
    {/if}

    {#if tab === 'activity'}
    {#if !detail?.last_outcome && !detail?.placement && (detail?.steps ?? []).length === 0}
      <p class="muted" data-testid="work-task-no-activity">Nothing has happened on this task yet.</p>
    {/if}
    {#if detail?.placement}
      <p class="muted small" data-testid="work-task-activity-placed">
        Placed in {detail.placement.group}{#if detail.placement.updated_by}&nbsp;by {detail.placement.updated_by}{/if}{#if detail.placement.updated_at}&nbsp;{timeAgo(detail.placement.updated_at)}{/if}
      </p>
    {/if}

    {#if detail?.last_outcome}
      {@const o = detail.last_outcome}
      <h3>Last outcome</h3>
      <div class="outcome" data-testid="work-task-outcome">
        <p class="small muted">
          {shortAge(o.at)}{#if o.name}&nbsp;· {o.name}{/if}{#if o.host}&nbsp;· {o.host}{/if}{#if o.branch}&nbsp;· branch {o.branch}{/if}
          {#if o.pr_url}
            · <button class="link-btn" type="button" onclick={() => void openExternal(o.pr_url ?? '')}>PR</button>
          {/if}
        </p>
        {#if o.summary}<p class="excerpt">{o.summary}</p>{/if}
      </div>
    {/if}

    {#if detail}<TaskWorkSections {detail} part="steps" />{/if}
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
  />
{/if}

<style>
  .task-detail {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    font-size: var(--text-sm);
  }
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
  }
  .tab {
    padding: 4px 10px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    cursor: pointer;
  }
  .tab[aria-selected='true'] {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .tab-count {
    margin-left: 4px;
    font-variant-numeric: tabular-nums;
  }
  .delivery dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 10px;
    margin: 0;
  }
  .delivery dt {
    color: var(--fg-muted);
  }
  .delivery dd {
    margin: 0;
  }
  .ok {
    color: var(--status-done);
  }
  .bad {
    color: var(--status-failed);
  }
  .status-pick {
    font: inherit;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.5rem;
  }
  h2 {
    margin: 0;
    font-size: var(--text-md);
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    align-items: baseline;
  }
  h3 {
    margin: 0.6rem 0 0.2rem;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  h4 {
    margin: 0.3rem 0 0.1rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .head-actions,
  .actions,
  .meta {
    display: flex;
    gap: 0.35rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .key {
    font-family: var(--mono);
  }
  .title {
    font-weight: normal;
    overflow-wrap: anywhere;
  }
  .title.unavailable {
    text-decoration: line-through;
  }
  .badge {
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
    padding: 0 0.3rem;
    color: var(--fg-muted);
  }
  .status {
    font-weight: 600;
  }
  .line,
  .excerpt {
    margin: 0;
  }
  .excerpt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border-left: 2px solid var(--border);
    padding-left: 0.5rem;
    color: var(--fg-muted);
  }
  .group-proposal {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    font-size: var(--text-xs);
  }
  .prov {
    margin: 0.2rem 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.2rem 0.6rem;
  }
  .prov dt {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .prov dd {
    margin: 0;
  }
  .links {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .link {
    padding: 0.25rem 0;
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
  .evidence {
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
  .error p {
    margin: 0 0 0.4rem;
  }
  .edits {
    margin-top: 0.1rem;
  }
  .more > summary {
    cursor: pointer;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .more[open] > summary {
    margin-bottom: 0.2rem;
  }
</style>
