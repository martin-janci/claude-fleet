<script lang="ts">
  // A task's detail in the center pane (work graph M14.2, read only), from
  // `work { task }`: the tracker's data, where its org and group come from,
  // its repositories, every session with its state and why, and the last
  // outcome. *Open* selects its live session, *Continue* resumes the last
  // conversation (`resume_work`, the Resume button's path), *Start new*
  // opens the new-session dialog for the ticket (`start_work`) — the
  // existing commands, so their confirmations and hub gates are unchanged.
  // Place / Assign org / Make a rule are M14.3. Tracker text is rendered as
  // text, never as markup.
  import { sessions } from './sessions';
  import { projects } from './projects';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { requestNewSession } from './new_session_request';
  import { openExternal } from './open_external';
  import { push, pushError } from './toasts';
  import { timeAgo } from './session_status';
  import { resumeWork, workResumePlan } from './work';
  import ResumeDialog from './ResumeDialog.svelte';
  import {
    countsLabel,
    groupSourceLabel,
    occurrenceKind,
    orgSourceLabel,
    projectForTask,
    ticketOf,
    trackerDown,
    workTreeStore,
    type WorkTreeStore,
  } from './work_tree';
  import { needsNewerHub, workTask, type TaskDetail, type WorkTaskLink } from './work_view';

  let {
    taskId,
    onclose,
    store = workTreeStore,
  }: {
    taskId: string;
    onclose: () => void;
    /** The app's one Work view; injectable for tests. */
    store?: WorkTreeStore;
  } = $props();

  // svelte-ignore state_referenced_locally
  const st = store;
  let detail = $state<TaskDetail | null>(null);
  let error = $state<string | null>(null);
  let needsHub = $state(false);
  let loading = $state(false);
  let seq = 0;

  // The task as the tree last saw it: a placement / org / link change there
  // re-reads the detail (one read per change, not per row event).
  const treeSig = $derived.by(() => {
    for (const o of $st.orgs)
      for (const s of o.sections) {
        const t = s.tasks.find((x) => x.task_id === taskId);
        if (t)
          return JSON.stringify([t.group.id, t.org_id, t.counts, t.placement_version, t.sessions.map((l) => [l.link_id, l.link_version, l.state])]);
      }
    return '';
  });

  $effect(() => {
    const id = taskId;
    void treeSig;
    const mine = ++seq;
    loading = true;
    void workTask(id).then((r) => {
      if (mine !== seq) return;
      loading = false;
      if (r.ok && !r.value?.task) {
        needsHub = true;
      } else if (r.ok) {
        detail = r.value;
        error = null;
        needsHub = false;
      } else if (needsNewerHub(r.error)) {
        needsHub = true;
      } else {
        detail = null;
        error = r.error.code === 'E_NOTFOUND' ? 'This task is gone, or not visible to this window.' : `${r.error.code}: ${r.error.message}`;
      }
    });
  });

  const task = $derived(detail?.task ?? null);
  const selId = $derived($selectedSession?.id ?? null);
  const ORDER: Record<string, number> = { primary: 0, secondary: 1, suggested: 2, past: 3, rejected: 4 };
  const links = $derived(
    (task?.sessions ?? []).slice().sort((a, b) => ORDER[occurrenceKind(a)] - ORDER[occurrenceKind(b)]),
  );
  const live = $derived(links.filter((l) => l.state === 'active' && l.session_id != null));
  const lastEnded = $derived(
    links
      .filter((l) => l.state === 'ended')
      .sort((a, b) => (b.ended_at ?? 0) - (a.ended_at ?? 0))[0] ?? null,
  );

  function rowOf(l: WorkTaskLink) {
    return l.session_id == null ? undefined : $sessions.find((s) => s.id === l.session_id);
  }
  function openLink(l: WorkTaskLink) {
    const row = rowOf(l);
    if (row) selectSessionExplicitly(row);
    else push({ kind: 'info', message: `${l.name} is not in this window's session list yet — refresh the sidebar.` });
  }
  function openLive() {
    const l = live.find((x) => x.primary) ?? live[0];
    if (l) openLink(l);
  }

  let resuming = $state(false);
  let resumeOpen = $state(false);
  async function continueLast() {
    if (!task?.key || resuming) return;
    resuming = true;
    const plan = await workResumePlan(task.key, { linkId: lastEnded?.link_id ?? null });
    const last = plan.ok ? plan.value.modes.find((m) => m.mode === 'last') : undefined;
    if (!plan.ok || !last?.ok || (plan.value.live ?? []).length > 0) {
      resuming = false;
      // The dialog says why, and offers the other modes.
      resumeOpen = true;
      return;
    }
    const r = await resumeWork({ key: task.key, mode: 'last', linkId: plan.value.link_id ?? null });
    resuming = false;
    if (!r.ok) pushError(r.error, `Continue ${task.key} failed`);
    else selectSessionExplicitly(r.value);
  }

  function startNew() {
    if (!task) return;
    const place = projectForTask(task, $sessions, $projects);
    if (!place) {
      push({ kind: 'info', message: 'No projects yet — refresh the sidebar first.' });
      return;
    }
    const key = task.key ?? '';
    requestNewSession({
      project: place.project,
      initialName: task.title ? `${key} ${task.title}`.trim() : key,
      initialHost: place.host,
      ticket: ticketOf(task),
    });
  }

  function when(secs: number | undefined | null): string {
    return secs ? timeAgo(secs, Date.now()) : '';
  }
  const KIND_LABEL: Record<string, string> = {
    primary: '★ primary',
    secondary: 'secondary',
    suggested: 'suggested',
    past: 'past',
    rejected: 'rejected',
  };
</script>

<div class="task-detail" data-testid="work-task-detail">
  <div class="head">
    <button type="button" class="back" onclick={onclose} title="Close the task" aria-label="Close the task">×</button>
    {#if task}
      {#if task.key}<span class="key" class:unavailable={task.unavailable}>{task.key}</span>{/if}
      <h2 class:unavailable={task.unavailable} data-testid="work-task-title">{task.title || task.key || task.task_id}</h2>
    {:else}
      <h2 class="muted">{taskId}</h2>
    {/if}
  </div>

  {#if needsHub}
    <p class="muted" data-testid="work-task-needs-hub">The task view needs a newer hub.</p>
  {:else if error}
    <p class="err" data-testid="work-task-error">{error}</p>
  {:else if !task}
    <p class="muted">{loading ? 'Loading…' : ''}</p>
  {:else}
    <div class="actions">
      <button type="button" class="btn" disabled={live.length === 0} title={live.length === 0 ? 'No live session' : 'Open its live session'} onclick={openLive} data-testid="work-task-open">Open</button>
      <button
        type="button"
        class="btn"
        disabled={!task.key || !lastEnded || resuming}
        title={!task.key ? 'Only a task with a key can be resumed' : !lastEnded ? 'No past session to continue' : 'Continue the last conversation'}
        onclick={() => void continueLast()}
        data-testid="work-task-continue">{resuming ? '…' : 'Continue'}</button
      >
      <button type="button" class="btn" onclick={startNew} data-testid="work-task-start">Start new</button>
      {#if task.url}
        <button type="button" class="btn quiet" onclick={() => void openExternal(task.url ?? '')} data-testid="work-task-link">Open ticket ↗</button>
      {/if}
    </div>

    <dl class="facts">
      <dt>Status</dt>
      <dd>
        {task.status_name ?? task.status_category ?? '—'}{#if task.resolution} · {task.resolution}{/if}
        {#if task.unavailable}<span class="warn"> · unavailable{task.unavailable_reason ? ` (${task.unavailable_reason})` : ''}</span>{/if}
      </dd>
      {#if task.tracker_name}
        <dt>Tracker</dt>
        <dd>
          {task.tracker_name}{#if trackerDown(task.tracker_state)}<span class="warn" data-testid="work-task-tracker-down"> · tracker down ({task.tracker_state}) — as of its last good sync</span>{/if}
        </dd>
      {/if}
      {#if task.assignees && task.assignees.length > 0}
        <dt>Assignees</dt>
        <dd>{task.assignees.join(', ')}{#if task.mine} · you{/if}</dd>
      {/if}
      <dt>Organisation</dt>
      <dd data-testid="work-task-org">
        {$st.orgList.find((o) => o.id === task.org_id)?.name ?? (task.org_id != null ? `org ${task.org_id}` : 'Unassigned')}
        <span class="muted">— {orgSourceLabel(task)}</span>
      </dd>
      <dt>Group</dt>
      <dd data-testid="work-task-group">
        {task.group.label || 'No group'} <span class="muted">— {groupSourceLabel(task.group)}</span>
        {#if detail?.placement?.note}<div class="muted">“{detail.placement.note}”</div>{/if}
      </dd>
      {#if task.repos && task.repos.length > 0}
        <dt>Repositories</dt>
        <dd>{task.repos.join(', ')}</dd>
      {/if}
      <dt>Sessions</dt>
      <dd>{countsLabel(task)}{#if task.counts.suggested > 0} · {task.counts.suggested} suggested{/if}</dd>
      {#if detail?.aliases && detail.aliases.length > 0}
        <dt>Also known as</dt>
        <dd class="muted">{detail.aliases.join(', ')}</dd>
      {/if}
    </dl>

    {#if detail?.description}
      <section>
        <h3>Description</h3>
        <p class="desc" data-testid="work-task-desc">{detail.description}</p>
      </section>
    {/if}

    <section>
      <h3>Sessions ({links.length})</h3>
      {#if links.length === 0}
        <p class="muted" data-testid="work-task-no-sessions">No session has worked on this yet. <em>Start new</em> opens one.</p>
      {:else}
        <ul class="links">
          {#each links as l (l.link_id)}
            {@const kind = occurrenceKind(l)}
            <li class="link {kind}" class:hl={l.session_id != null && l.session_id === selId} data-testid="work-task-session" data-kind={kind}>
              <div class="line">
                <span class="kind">{KIND_LABEL[kind]}</span>
                {#if l.session_id != null && kind !== 'past'}
                  <button type="button" class="name link-btn" onclick={() => openLink(l)}>{l.name}</button>
                {:else}
                  <span class="name">{l.name}</span>
                {/if}
                {#if l.host}<span class="muted">{l.host}</span>{/if}
                {#if l.branch}<span class="muted">· {l.branch}</span>{/if}
                {#if kind === 'past'}
                  <span class="muted">· ended {when(l.ended_at)}{l.end_reason ? ` (${l.end_reason})` : ''}</span>
                {:else if l.needs_you}
                  <span class="warn">· needs you</span>
                {:else if l.claude_status}
                  <span class="muted">· {l.claude_status}</span>
                {/if}
                {#if l.cross_org}<span class="warn">· cross-org</span>{/if}
                {#if l.pr_url}<button type="button" class="link-btn" onclick={() => void openExternal(l.pr_url ?? '')}>PR ↗</button>{/if}
              </div>
              {#if l.why}<div class="why muted">why: {l.why}{l.rule ? ` · ${l.rule}` : ''}</div>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if detail?.last_outcome}
      {@const o = detail.last_outcome}
      <section data-testid="work-task-outcome">
        <h3>Last outcome</h3>
        <p>
          {o.name}{o.host ? ` on ${o.host}` : ''}{o.branch ? ` · ${o.branch}` : ''} · {when(o.at)}{o.end_reason ? ` · ${o.end_reason}` : ''}
          {#if o.pr_url}<button type="button" class="link-btn" onclick={() => void openExternal(o.pr_url ?? '')}>PR ↗</button>{/if}
        </p>
        {#if o.summary}<p class="desc">{o.summary}</p>{/if}
      </section>
    {/if}
  {/if}
</div>

{#if resumeOpen && task?.key}
  <ResumeDialog workKey={task.key} linkId={lastEnded?.link_id ?? null} onclose={() => (resumeOpen = false)} />
{/if}

<style>
  .task-detail { padding: 0.6rem 0.4rem; font-size: 0.85rem; }
  .head { display: flex; align-items: baseline; gap: 0.5rem; }
  .back {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    flex: none;
  }
  .key { font-family: var(--mono); color: var(--accent); font-size: 0.85rem; flex: none; }
  h2 { font-size: 1rem; margin: 0; font-weight: 600; min-width: 0; overflow-wrap: anywhere; }
  h3 {
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0.8rem 0 0.35rem;
  }
  .unavailable { text-decoration: line-through; color: var(--fg-muted); }
  .muted { color: var(--fg-muted); }
  .warn { color: var(--usage-warn); }
  .err { color: var(--usage-crit); }
  .actions { display: flex; flex-wrap: wrap; gap: 0.35rem; margin: 0.6rem 0; }
  .btn {
    height: var(--control-h);
    padding: 0 var(--control-px-lg);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font-size: var(--control-font);
    cursor: pointer;
  }
  .btn:hover:not(:disabled) { background: var(--control-bg-hover); }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn.quiet { border-color: transparent; color: var(--accent); }
  .facts { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 0.8rem; margin: 0; }
  dt { color: var(--fg-muted); font-size: 0.75rem; }
  dd { margin: 0; }
  .desc { white-space: pre-wrap; margin: 0; overflow-wrap: anywhere; }
  .links { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .link { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.3rem 0.5rem; }
  .link.suggested { border-style: dashed; }
  .link.past, .link.rejected { opacity: 0.6; }
  .link.hl { border-color: var(--accent); background: var(--accent-soft); }
  .line { display: flex; flex-wrap: wrap; gap: 0.35rem; align-items: baseline; }
  .kind { font-size: 0.7rem; color: var(--fg-muted); min-width: 4.5rem; }
  .link.primary .kind { color: var(--usage-warn); }
  .name { font-weight: 500; }
  .link-btn { background: transparent; border: none; color: var(--accent); cursor: pointer; padding: 0; font: inherit; }
  .why { font-size: 0.72rem; margin-top: 0.1rem; }
</style>
