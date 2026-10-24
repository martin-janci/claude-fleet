<!--
  A mission's tasks in its room's Plan tab (Thread room for Work,
  2026-10-15; until then the detail's Plan tab in the sidebar): List keeps
  every write — the task's live sessions, what each waits for, its latest
  attempt, its done_when checks (Met, and the lines edited), the waits it
  starts or drops, Retry, Hold / Release and Take out — and Graph draws
  lanes × waves with the critical path (MissionGraph). Below them the work
  outside the mission it waits on, and a new task under its root.

  The room runs every write (`act`): it re-reads the mission and says what
  went wrong. Task titles are tracker text, rendered as text.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './kit/Icon.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import Loader from './Loader.svelte';
  import MissionGraph from './MissionGraph.svelte';
  import { tablistKeys } from './tablist_keys';
  import { defaultLaneBy, toneOf, type LaneBy } from './mission_graph';
  import { createWorkTask } from './work';
  import { projects } from './projects';
  import { readErrorText } from './work_view';
  import type { Result } from './result';
  import {
    attemptLine,
    checkGlyph,
    checkable,
    doneWhenRows,
    evidenceMissing,
    nodeLabel,
    overrideRunEvidence,
    retryWorkItem,
    laneOfItem,
    sessionsByItem,
    setMissionItem,
    setMissionLane,
    setWorkDep,
    setWorkDoneWhen,
    setWorkHold,
    trailNodes,
    verificationLabel,
    verifyWorkItem,
    wavesOf,
    withoutConfigKeys,
    type FinishSession,
    type GraphNode,
    type MissionDetail,
  } from './missions';

  let {
    detail,
    mayChange,
    busy,
    changeBlocked,
    act,
    tasksView = $bindable('list'),
    onopenitem,
    onopensession,
    onnotice,
  }: {
    detail: MissionDetail;
    mayChange: boolean;
    busy: boolean;
    changeBlocked: boolean;
    /** Run one write; the room re-reads the mission on success. */
    act: <T>(p: Promise<Result<T>>) => Promise<T | null>;
    tasksView?: 'list' | 'graph';
    onopenitem: (itemId: number) => void;
    onopensession: (s: FinishSession) => void;
    /** Something to say in the room's notice line. */
    onnotice: (text: string) => void;
  } = $props();

  const mission = $derived(detail.mission);
  const waves = $derived(wavesOf(detail));
  const itemById = $derived(new Map((detail.items ?? []).map((i) => [i.id, i])));
  const liveByItem = $derived(sessionsByItem(detail));
  // Comet trails beside the mission's current steps (redesign step 9.12):
  // only while it runs, never while it waits on a person.
  const trails = $derived(trailNodes(detail));
  const showGraph = $derived(tasksView === 'graph');

  // The lanes reset to the mission's own default when another one opens.
  let laneOverride = $state<{ mission: number; by: LaneBy } | null>(null);
  const laneBy = $derived(laneOverride && laneOverride.mission === mission.id ? laneOverride.by : defaultLaneBy(detail));

  function repoName(id: number): string | null {
    const own = (mission.repos ?? []).find((r) => r.project_id === id);
    if (own) return own.name;
    const p = $projects.find((x) => x.project.id === id)?.project;
    return p ? `${p.owner}/${p.repo}` : null;
  }

  function titleOf(id: number): string {
    const it = itemById.get(id);
    if (it) return it.title;
    const o = (detail.graph?.outside ?? []).find((x) => x.id === id);
    return o ? (o.key ?? o.title) : `#${id}`;
  }

  /** The members `n` may start waiting for: not itself, not already. */
  function depChoices(n: GraphNode): { id: number; title: string }[] {
    const already = new Set(n.depends_on ?? []);
    return (detail.items ?? [])
      .filter((i) => i.id !== n.item_id && !already.has(i.id) && i.proposal_state !== 'rejected')
      .map((i) => ({ id: i.id, title: i.title }));
  }

  // The task whose condition lines are being edited, and their text.
  let condsFor = $state<number | null>(null);
  let condText = $state('');

  function editConds(itemId: number) {
    condsFor = itemId;
    condText = (itemById.get(itemId)?.done_when ?? []).join('\n');
  }

  async function saveConds() {
    if (condsFor == null) return;
    if (await act(setWorkDoneWhen(condsFor, doneWhenRows(condText)))) condsFor = null;
  }

  // The task whose missing run evidence a person is overriding, and why.
  let overrideFor = $state<number | null>(null);
  let overrideText = $state('');
  /** The task whose new lane is being named (lanes design L2). */
  let laneFor = $state<number | null>(null);
  let laneText = $state('');
  const NEW_LANE = '\u0000new';
  async function saveLane(): Promise<void> {
    if (laneFor == null || !laneText.trim()) return;
    if (await act(setMissionLane(mission.id, laneFor, laneText))) laneFor = null;
  }
  function pickLane(itemId: number, v: string): void {
    if (v === NEW_LANE) {
      laneFor = itemId;
      laneText = '';
    } else {
      void act(setMissionLane(mission.id, itemId, v || null));
    }
  }

  async function saveOverride() {
    if (overrideFor == null || !overrideText.trim()) return;
    const id = overrideFor;
    if (await act(overrideRunEvidence(id, overrideText))) closeOverride(id);
  }

  /** The reason field takes focus as it opens: the Override… button that
   *  opened it is gone (review r8). */
  function focusOnMount(el: HTMLInputElement): void {
    el.focus();
  }

  /** Close the reason form and give focus back to its Override… button,
   *  when the task still has one. */
  async function closeOverride(id: number): Promise<void> {
    overrideFor = null;
    await tick();
    document.querySelector<HTMLButtonElement>(`[data-testid="mission-evidence-override"][data-item="${id}"]`)?.focus();
  }

  async function retry(itemId: number) {
    const out = await act(retryWorkItem(itemId));
    if (out && !out.ok) onnotice(out.detail);
  }

  let newTask = $state('');
  let adding = $state(false);
  async function addTask() {
    if (!mission.root_item_id) return;
    const title = newTask.trim();
    if (!title) return;
    adding = true;
    const t = await createWorkTask({ title, parent: `item:${mission.root_item_id}` });
    adding = false;
    if (!t.ok) {
      onnotice(withoutConfigKeys(readErrorText(t.error)));
      return;
    }
    if (await act(setMissionItem(mission.id, t.value.id, true))) newTask = '';
  }
</script>

<div class="tasks" data-testid="mission-tasks">
  <div class="view-switch" role="tablist" aria-label="Show tasks as" use:tablistKeys>
    <button type="button" role="tab" aria-selected={tasksView === 'list'} data-testid="mission-view-list" onclick={() => (tasksView = 'list')}
      >List</button
    >
    <button type="button" role="tab" aria-selected={tasksView === 'graph'} data-testid="mission-view-graph" onclick={() => (tasksView = 'graph')}
      >Graph</button
    >
  </div>
  {#if showGraph}
    <MissionGraph
      {detail}
      {laneBy}
      {repoName}
      onlanechange={(by) => (laneOverride = { mission: mission.id, by })}
      {onopenitem}
      {onopensession}
    />
  {/if}
  {#each showGraph ? [] : waves as w (w.wave)}
    <div class="wave" data-testid="mission-wave">
      {#if waves.length > 1}<span class="wave-head">W{w.wave}</span>{/if}
      <ul class="items" data-testid="mission-items">
        {#each w.nodes as n (n.item_id)}
          {@const it = itemById.get(n.item_id)}
          {#if it}
            <li data-testid="mission-node" data-state={n.state}>
              <span class="node-dot" title={nodeLabel(n.state)}><StatusDot state={toneOf(n.state)} label={nodeLabel(n.state)} /></span>
              {#if trails.has(n.item_id)}
                <Loader name="comet-trails" size={20} label="Working on it" testid="mission-trails" />
              {/if}
              <span class="main">
                <span class="title-row">
                  <button class="title linkish" type="button" title="Open the task" data-testid="mission-node-open" onclick={() => onopenitem(it.id)}
                    >{it.key ? `${it.key} · ` : ''}{it.title}</button
                  >
                  {#each liveByItem.get(it.id) ?? [] as s (s.session_id)}
                    <button
                      class="btn btn--chip sess-link"
                      type="button"
                      title="Open {s.tmux_name} on {s.host_alias}"
                      data-testid="mission-task-session"
                      data-session-id={s.session_id}
                      onclick={() => onopensession(s)}>Session</button
                    >
                  {/each}
                </span>
                {#if (n.waiting_for ?? []).length > 0}
                  <span class="muted small" data-testid="mission-waits">waits for {(n.waiting_for ?? []).map(titleOf).join(', ')}</span>
                {/if}
                {#if n.attempt}
                  <span class="muted small" data-testid="mission-attempt" title={n.attempt.summary ?? ''}>{attemptLine(n.attempt)}</span>
                {/if}
                {#if n.verification}
                  <span class="vbadge v-{n.verification.state}" data-testid="mission-verified">{verificationLabel(n.verification.state)}</span>
                  <ul class="checks" data-testid="mission-checks">
                    {#each n.verification.checks as c (c.line)}
                      <li class="c-{c.state}">
                        <span class="glyph" aria-label={c.state}>{checkGlyph(c.state)}</span>
                        <span class="line">{c.line}</span>
                        <span class="muted small">{c.detail}</span>
                        {#if detail.may_change && checkable(c)}
                          <button
                            class="btn btn--chip"
                            type="button"
                            disabled={busy}
                            data-testid="mission-check"
                            onclick={() => void act(verifyWorkItem(n.item_id, c.line, true))}>Met</button
                          >
                        {/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
                {#if n.evidence && (evidenceMissing(n.evidence) || n.evidence.state === 'overridden')}
                  <span class="evidence ev-{n.evidence.state}" data-testid="mission-evidence" data-state={n.evidence.state}>
                    <span class="glyph" aria-hidden="true">{checkGlyph(evidenceMissing(n.evidence) ? 'fail' : 'pass')}</span>
                    <span>{evidenceMissing(n.evidence) ? 'No run evidence' : 'Done without evidence'}:</span>
                    <span class="muted">{n.evidence.detail}</span>
                    {#if detail.may_change && evidenceMissing(n.evidence) && overrideFor !== n.item_id}
                      <button
                        class="btn btn--chip"
                        type="button"
                        title="Accept this run without commits or files, and say why"
                        disabled={busy}
                        data-testid="mission-evidence-override"
                        data-item={n.item_id}
                        onclick={() => ((overrideFor = n.item_id), (overrideText = ''))}>Override…</button
                      >
                    {/if}
                  </span>
                  {#if overrideFor === n.item_id}
                    <form class="conds" onsubmit={(e) => (e.preventDefault(), void saveOverride())}>
                      <input
                        type="text"
                        aria-label="Why the task is done without commits or files"
                        placeholder="Why it is done without commits or files"
                        bind:value={overrideText}
                        use:focusOnMount
                        data-testid="mission-evidence-reason"
                      />
                      <span class="row">
                        <button class="btn" type="submit" disabled={busy || !overrideText.trim()} data-testid="mission-evidence-save"
                          >Override</button
                        >
                        <button class="btn btn--quiet" type="button" onclick={() => void closeOverride(n.item_id)}>Cancel</button>
                      </span>
                    </form>
                  {/if}
                {/if}
                {#if laneFor === n.item_id}
                  <form class="conds" onsubmit={(e) => (e.preventDefault(), void saveLane())}>
                    <input
                      type="text"
                      aria-label="New lane's name"
                      placeholder="Lane name, e.g. backend"
                      maxlength="40"
                      bind:value={laneText}
                      data-testid="mission-lane-name"
                    />
                    <span class="row">
                      <button class="btn" type="submit" disabled={busy || !laneText.trim()} data-testid="mission-lane-save">Put in lane</button>
                      <button class="btn btn--quiet" type="button" onclick={() => (laneFor = null)}>Cancel</button>
                    </span>
                  </form>
                {/if}
                {#if condsFor === n.item_id}
                  <form class="conds" onsubmit={(e) => (e.preventDefault(), void saveConds())}>
                    <textarea
                      rows="3"
                      aria-label="Done when, one condition per line"
                      placeholder={'ci\nreview\ntest:cargo test\nperson'}
                      bind:value={condText}
                      data-testid="mission-conds-text"
                    ></textarea>
                    <span class="row">
                      <button class="btn" type="submit" disabled={busy} data-testid="mission-conds-save">Save</button>
                      <button class="btn btn--quiet" type="button" onclick={() => (condsFor = null)}>Cancel</button>
                    </span>
                  </form>
                {/if}
                {#if mayChange && (n.depends_on ?? []).length > 0}
                  <span class="deps">
                    {#each n.depends_on ?? [] as d (d)}
                      <button
                        class="btn btn--chip dep"
                        type="button"
                        title="Stop waiting for {titleOf(d)}"
                        disabled={busy}
                        data-testid="mission-dep-remove"
                        onclick={() => void act(setWorkDep(n.item_id, d, false))}>after {titleOf(d)} ×</button
                      >
                    {/each}
                  </span>
                {/if}
              </span>
              {#if mayChange && condsFor !== n.item_id}
                <button
                  class="btn btn--quiet btn--icon"
                  type="button"
                  aria-label="Done when…"
                  title="Done when: the conditions that verify this task"
                  disabled={busy}
                  data-testid="mission-conds"
                  onclick={() => editConds(n.item_id)}><Icon name="checklist" size={14} /></button
                >
              {/if}
              {#if it.id === mission.root_item_id}
                <span class="muted small">root</span>
              {:else if mayChange}
                {@const lane = laneOfItem(detail, it.id)}
                {@const choices = depChoices(n)}
                <select
                  class="dep-pick"
                  aria-label="Lane"
                  title="Lane: the tasks one session works through on one branch"
                  disabled={busy}
                  value={lane?.name ?? ''}
                  data-testid="mission-lane-pick"
                  onchange={(e) => pickLane(it.id, (e.currentTarget as HTMLSelectElement).value)}
                >
                  <option value="">No lane</option>
                  {#each detail.lanes ?? [] as l (l.id)}<option value={l.name}>{l.name}</option>{/each}
                  <option value={NEW_LANE}>New lane…</option>
                </select>
                {#if choices.length > 0}
                  <select
                    class="dep-pick"
                    aria-label="Waits for"
                    disabled={busy}
                    data-testid="mission-dep-add"
                    onchange={(e) => {
                      const v = Number((e.currentTarget as HTMLSelectElement).value);
                      (e.currentTarget as HTMLSelectElement).value = '';
                      if (v) void act(setWorkDep(n.item_id, v, true));
                    }}
                  >
                    <option value="">after…</option>
                    {#each choices as c (c.id)}<option value={c.id}>{c.title}</option>{/each}
                  </select>
                {/if}
                {#if n.state === 'failed' && mission.state === 'active'}
                  <button
                    class="btn btn--quiet btn--icon"
                    type="button"
                    aria-label="Retry"
                    title="Try again, with the last failure in the prompt"
                    disabled={busy || changeBlocked}
                    data-testid="mission-retry"
                    onclick={() => void retry(it.id)}>↻</button
                  >
                {/if}
                {#if n.state !== 'done'}
                  <button
                    class="btn btn--quiet btn--icon"
                    type="button"
                    aria-label={it.held_at ? 'Release' : 'Hold'}
                    title={it.held_at ? 'Release' : 'Hold: never start this on its own'}
                    disabled={busy}
                    data-testid="mission-hold"
                    onclick={() => void act(setWorkHold(it.id, !it.held_at))}><Icon name={it.held_at ? 'play' : 'pause'} size={14} /></button
                  >
                {/if}
                <button
                  class="btn btn--quiet btn--icon"
                  type="button"
                  aria-label="Take out of the mission"
                  title="Take out of the mission"
                  disabled={busy}
                  data-testid="mission-item-remove"
                  onclick={() => void act(setMissionItem(mission.id, it.id, false))}>×</button
                >
              {/if}
            </li>
          {/if}
        {/each}
      </ul>
    </div>
  {/each}
  {#if (detail.graph?.outside ?? []).length > 0}
    <p class="muted small" data-testid="mission-outside">
      Waits on work outside the mission: {(detail.graph?.outside ?? []).map((o) => (o.key ? `${o.key} · ${o.title}` : o.title)).join(', ')}
    </p>
  {/if}
  {#if mayChange && mission.root_item_id}
    <form class="row" onsubmit={(e) => (e.preventDefault(), void addTask())}>
      <input placeholder="New task" aria-label="New task" bind:value={newTask} data-testid="mission-new-task" />
      <button class="btn" type="submit" disabled={busy || adding || !newTask.trim()}>Add</button>
    </form>
  {/if}
</div>

<style>
  .tasks {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .row input {
    flex: 1 1 8rem;
    min-width: 0;
  }
  input,
  textarea,
  select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .items li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--border);
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }
  .title {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .linkish {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .linkish:hover,
  .linkish:focus-visible {
    color: var(--accent);
    text-decoration: underline;
  }
  .sess-link {
    flex: none;
  }
  .node-dot {
    display: inline-flex;
    align-items: center;
    height: 1lh;
    flex: 0 0 auto;
  }
  .main {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }
  .deps {
    display: flex;
    flex-wrap: wrap;
    gap: 0.2rem;
    margin-top: 0.15rem;
  }
  .dep {
    font-size: var(--text-2xs);
  }
  .dep-pick {
    max-width: 7rem;
    font-size: var(--text-2xs);
  }
  .wave {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }
  .wave-head {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin-top: 0.3rem;
  }
  .vbadge {
    font-size: var(--text-2xs);
    align-self: flex-start;
    padding: 0 0.35rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
  }
  .vbadge.v-verified {
    color: var(--status-done);
    border-color: var(--status-done);
  }
  .vbadge.v-failed {
    color: var(--danger);
    border-color: var(--danger);
  }
  .checks li {
    display: flex;
    gap: 0.3rem;
    align-items: baseline;
    font-size: var(--text-2xs);
  }
  .checks .line {
    font-family: var(--font-mono);
  }
  .glyph {
    width: 1.1rem;
    text-align: center;
    flex: 0 0 auto;
    color: var(--fg-muted);
  }
  .checks .c-pass .glyph {
    color: var(--status-done);
  }
  .checks .c-fail .glyph {
    color: var(--danger);
  }
  .evidence {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    align-items: baseline;
    font-size: var(--text-2xs);
  }
  .evidence.ev-missing .glyph {
    color: var(--danger);
  }
  .evidence.ev-overridden .glyph {
    color: var(--status-done);
  }
  .conds {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    margin-top: 0.2rem;
  }
  /* The design system's Tabs (of-tabs). */
  .view-switch {
    display: flex;
    gap: 20px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 0.4rem;
  }
  .view-switch button {
    font: inherit;
    font-size: var(--text-xs);
    color: var(--fg-muted);
    background: none;
    border: 0;
    cursor: pointer;
    padding: 4px 0;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .view-switch button:hover {
    color: var(--fg);
  }
  .view-switch button[aria-selected='true'] {
    color: var(--fg);
    border-bottom-color: var(--accent);
    font-weight: 500;
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .small {
    font-size: var(--text-2xs);
  }
</style>
