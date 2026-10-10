<!--
  What Control's agent handed on (Orbit Fleet redesign steps 9.3 and 9.6),
  drawn in its transcript: "Sent to a session" and "Sent
  to a mission" chips that follow their target's live state and open it, a
  live card per task it created, and its proposed tree of subtasks as a card
  where the person unticks what they do not want, then creates the tasks or
  a mission, with Undo for ten minutes. The receipts are the backend's
  (`handoffs.ts`); nothing here is inferred from the transcript's text.

  Gap plan G3.11 (board MCTasks): the plan card lists its tasks by wave
  with what each finishes when, says "May duplicate …" with Merge or Keep
  both, and opens the parent with Edit in Work; a created task's card
  takes an owner and a due date while it is new, with Undo while the
  backend can take it back, and every task card is a status card (its
  live session, Move to Done, Mark verified, Open in Work). Cut: an agent
  per task and drag to reorder (a work item has no agent field and no
  order), the Group picker, and Start review run (no review-run action).
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Loader from './Loader.svelte';
  import { durationMs, effectiveMotion } from './motion';
  import { fliesIn } from './handoff_flight';
  import Button from './kit/Button.svelte';
  import QuestionCard from './kit/QuestionCard.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { STATE_WORD } from './kit/status';
  import {
    createFromTree,
    finishesWhen,
    liveSessionsOf,
    refreshHandoffs,
    taskUndoable,
    treeWaves,
    followHandoffs,
    openProposals,
    recentHandoffs,
    sessionChip,
    undoTree,
    undoable,
    workState,
    ACCEPT_UNDO_SECS,
    taskSummary,
    splitTaskReceipts,
    TASKS_OPEN_UP_TO,
    type ControlHandoff,
    type HandoffItem,
  } from './handoffs';
  import { displayName } from './attention';
  import { leave } from './destination';
  import { editWorkItem, mergeWorkProposal, parseAssignees, setWorkStatus } from './work';
  import { undoWorkAccept, verifyWorkItem } from './missions';
  import { workTask, type DuplicateHint } from './work_view';
  import { sessions } from './sessions';
  import { focusSession } from './session_focus';
  import { openMission } from './missions';
  import { showTaskInWorkView, sidebarView } from './work_view';

  let { mac = false }: { mac?: boolean } = $props();

  onMount(() => followHandoffs());

  // Undo's countdown; a minute is the finest it says.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 15_000);
    return () => clearInterval(t);
  });

  // Step 9.13: a session receipt that arrives while the chat is open flies
  // in as a comet, then its chip settles. `animationend` lands it; the timer
  // is the fallback for a hidden window, where no animation runs.
  const openedAt = Math.floor(Date.now() / 1000) - 1;
  const seen = new Set<number>();
  let flying = $state<number[]>([]);
  let settled = $state<number[]>([]);

  $effect(() => {
    const list = $recentHandoffs;
    const motion = $effectiveMotion;
    untrack(() => {
      for (const h of list) {
        if (seen.has(h.id)) continue;
        seen.add(h.id);
        if (!fliesIn(h, openedAt, motion)) continue;
        flying = [...flying, h.id];
        setTimeout(() => land(h.id), durationMs('slow') + 300);
      }
    });
  });

  function land(id: number) {
    if (!flying.includes(id)) return;
    flying = flying.filter((x) => x !== id);
    settled = [...settled, id];
  }

  /** Unticked proposals, per receipt: everything starts ticked. */
  let unticked = $state<Record<number, number[]>>({});
  let busy = $state<number | null>(null);
  let errors = $state<Record<number, string>>({});

  function toggle(h: ControlHandoff, id: number) {
    const off = unticked[h.id] ?? [];
    unticked[h.id] = off.includes(id) ? off.filter((x) => x !== id) : [...off, id];
  }

  function ticked(h: ControlHandoff): number[] {
    const off = unticked[h.id] ?? [];
    return openProposals(h)
      .map((i) => i.id)
      .filter((id) => !off.includes(id));
  }

  async function create(h: ControlHandoff, asMission: boolean) {
    busy = h.id;
    const err = await createFromTree(h, ticked(h), asMission);
    busy = null;
    if (err) errors[h.id] = err;
    else delete errors[h.id];
  }

  async function undo(h: ControlHandoff) {
    busy = h.id;
    const err = await undoTree(h, nowSec);
    busy = null;
    if (err) errors[h.id] = err;
    else delete errors[h.id];
  }

  function openSession(h: ControlHandoff) {
    const c = sessionChip(h, $sessions);
    if (h.session_id != null) focusSession(h.session_id, c.name);
  }

  function openMissionChip(h: ControlHandoff) {
    if (h.mission_id == null) return;
    sidebarView.set('work');
    openMission(h.mission_id);
  }

  // ── G3.11: duplicates on the plan card ──

  /** Jev's "may duplicate" per proposed item, read from the parent's task
   *  page (`ProposalView.duplicate`), once per parent. */
  let dups = $state<Record<number, DuplicateHint>>({});
  /** Items a person said to keep beside the task they may repeat. */
  let kept = $state<number[]>([]);
  const dupRead = new Set<number>();
  $effect(() => {
    for (const h of $recentHandoffs) {
      if (h.kind !== 'tree' || !h.item || openProposals(h).length === 0 || dupRead.has(h.item.id)) continue;
      dupRead.add(h.item.id);
      void workTask(`item:${h.item.id}`).then((r) => {
        if (!r.ok) return;
        const next = { ...dups };
        for (const p of r.value?.proposals ?? []) if (p.duplicate) next[p.item_id] = p.duplicate;
        dups = next;
      });
    }
  });

  async function merge(h: ControlHandoff, item: HandoffItem, into: DuplicateHint) {
    busy = h.id;
    const r = await mergeWorkProposal(item.id, into.item_id);
    busy = null;
    if (!r.ok) errors[h.id] = r.error.message;
    else await refreshHandoffs();
  }

  function editInWork(h: ControlHandoff) {
    if (!h.item) return;
    showTaskInWorkView(`item:${h.item.id}`);
    leave('control');
  }

  // ── G3.11: the created-task and status card ──

  /** A task receipt is new for this long: its card asks owner and due. */
  const FRESH_SECS = 600;
  let owner = $state<Record<number, string>>({});
  let due = $state<Record<number, string>>({});
  let saved = $state<Record<number, string>>({});

  async function saveDraft(h: ControlHandoff, item: HandoffItem) {
    const who = parseAssignees(owner[h.id] ?? '');
    const when = due[h.id] ?? '';
    busy = h.id;
    const r = await editWorkItem(item.id, { ...(who.length ? { assignees: who } : {}), ...(when ? { due_at: when } : {}) });
    busy = null;
    if (!r.ok) errors[h.id] = r.error.message;
    else {
      delete errors[h.id];
      saved[h.id] = [who.join(', '), when].filter(Boolean).join(' · ');
    }
  }

  async function taskAction(h: ControlHandoff, run: () => Promise<{ ok: boolean; error?: { message: string } }>) {
    busy = h.id;
    const r = await run();
    busy = null;
    if (!r.ok) errors[h.id] = r.error?.message ?? 'Failed';
    else {
      delete errors[h.id];
      await refreshHandoffs();
    }
  }

  // ── Control chat UX (2026-10-10): the task group ──

  const split = $derived(splitTaskReceipts($recentHandoffs));
  const others = $derived(split.others);
  const tasks = $derived(split.tasks);
  const listId = `handoff-tasks-${Math.random().toString(36).slice(2, 8)}`;
  /** The person's own fold, once they pressed the header. */
  let tasksChoice = $state<boolean | null>(null);
  /** A draft or an error waiting on a row is never folded away. */
  const tasksForced = $derived(
    tasks.some((h) => (nowSec - h.at < FRESH_SECS && h.item!.status === 'todo') || errors[h.id] !== undefined),
  );
  const tasksOpen = $derived(tasksForced || (tasksChoice ?? tasks.length <= TASKS_OPEN_UP_TO));

  function minutesLeft(h: ControlHandoff): number {
    const at = Math.max(...undoable(h, nowSec).map((i) => i.accepted_at ?? 0));
    return Math.max(1, Math.ceil((at + ACCEPT_UNDO_SECS - nowSec) / 60));
  }
</script>

{#if $recentHandoffs.length > 0}
  <div class="handoffs" data-testid="handoffs">
    {#each others as h (h.id)}
      {#if h.kind === 'session'}
        {@const c = sessionChip(h, $sessions)}
        {@const inFlight = flying.includes(h.id)}
        <div class="slot">
        {#if inFlight}
          <span class="flight" data-testid="handoff-flight" aria-hidden="true" onanimationend={() => land(h.id)}>
            <Loader name="comet" size={12} delay={0} testid="handoff-comet" />
          </span>
        {/if}
        <button
          type="button"
          class="handoff"
          class:in-flight={inFlight}
          class:settled={settled.includes(h.id)}
          data-flight={inFlight ? 'flying' : settled.includes(h.id) ? 'landed' : undefined}
          data-testid="handoff-session"
          title={h.preview ?? undefined}
          onclick={() => openSession(h)}
        >
          <span class="what">Sent to a session</span>
          <span class="target">{c.name}</span>
          {#if c.state}<StatusChip state={c.state} />{:else}<span class="what">ended</span>{/if}
        </button>
        </div>
      {:else if h.kind === 'mission'}
        <button
          type="button"
          class="handoff"
          data-testid="handoff-mission"
          title={h.preview ?? undefined}
          disabled={!h.mission_name}
          onclick={() => openMissionChip(h)}
        >
          <span class="what">Sent to a mission</span>
          <span class="target">{h.mission_name ?? 'a deleted mission'}</span>
          {#if h.mission_state}<StatusChip state={workState(h.mission_state)} />{/if}
        </button>
      {:else if h.kind === 'tree'}
        {@const open = openProposals(h)}
        {@const back = undoable(h, nowSec)}
        {#if open.length > 0}
          <QuestionCard
            question={h.item ? `Create tasks under ${h.item.title}?` : 'Create these tasks?'}
            label="The agent proposed tasks"
            testid="handoff-tree"
            {mac}
            keys={false}
            answers={[
              {
                label: 'Create tasks',
                primary: true,
                disabled: busy === h.id || ticked(h).length === 0,
                onselect: () => void create(h, false),
                testid: 'handoff-tree-create',
              },
              {
                label: 'Create as a mission',
                disabled: busy === h.id || ticked(h).length === 0 || !h.item,
                onselect: () => void create(h, true),
                testid: 'handoff-tree-mission',
              },
            ]}
          >
            {@const waves = treeWaves(open)}
            {#each waves as wave, wi (wi)}
              {#if waves.length > 1}<p class="wave" data-testid="handoff-tree-wave">Wave {wi + 1}</p>{/if}
              <ul class="tree">
                {#each wave as item (item.id)}
                  {@const dup = kept.includes(item.id) ? undefined : dups[item.id]}
                  {@const when = finishesWhen(item.done_when)}
                  <li>
                    <label>
                      <input
                        type="checkbox"
                        checked={!(unticked[h.id] ?? []).includes(item.id)}
                        onchange={() => toggle(h, item.id)}
                        data-testid="handoff-tree-item"
                      />
                      {item.title}
                    </label>
                    {#if when}<p class="sub" data-testid="handoff-tree-when">{when}</p>{/if}
                    {#if dup}
                      <p class="dup" data-testid="handoff-tree-dup">
                        May duplicate {dup.key ?? dup.title}
                        <Button variant="quiet" size="sm" disabled={busy === h.id} onclick={() => void merge(h, item, dup)} testid="handoff-tree-merge"
                          >Merge</Button
                        >
                        <Button variant="quiet" size="sm" onclick={() => (kept = [...kept, item.id])} testid="handoff-tree-keep">Keep both</Button>
                      </p>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/each}
            {#if h.item}
              <Button variant="quiet" size="sm" onclick={() => editInWork(h)} testid="handoff-tree-edit">Edit in Work</Button>
            {/if}
            {#if errors[h.id]}<p class="error" role="alert">{errors[h.id]}</p>{/if}
          </QuestionCard>
        {:else if back.length > 0}
          <div class="handoff done" data-testid="handoff-tree-created">
            <span class="what">Created {back.length} {back.length === 1 ? 'task' : 'tasks'}</span>
            <Button
              variant="quiet"
              size="sm"
              disabled={busy === h.id}
              onclick={() => void undo(h)}
              testid="handoff-tree-undo">Undo · {minutesLeft(h)} min</Button
            >
            {#if errors[h.id]}<p class="error" role="alert">{errors[h.id]}</p>{/if}
          </div>
        {/if}
      {/if}
    {/each}
    {#if tasks.length > 0}
      <!-- Control chat UX (2026-10-10): the created tasks as one group of
           one-line rows under a summary, folded once there are more than a
           few, so they never take the chat's height. -->
      <section class="task-group" data-testid="handoff-task-group">
        <button
          type="button"
          class="group-head"
          aria-expanded={tasksOpen}
          aria-controls={listId}
          data-testid="handoff-task-toggle"
          disabled={tasksForced}
          onclick={() => (tasksChoice = !tasksOpen)}
        >
          <span class="chev" aria-hidden="true">{tasksOpen ? '▾' : '▸'}</span>
          <span>{taskSummary(tasks.map((h) => h.item!.status))}</span>
        </button>
        <ul class="task-rows" id={listId} hidden={!tasksOpen}>
          {#each tasks as h (h.id)}
            {@const item = h.item!}
            {@const live = liveSessionsOf(item.id, $sessions)}
            {@const fresh = nowSec - h.at < FRESH_SECS && item.status === 'todo'}
            {@const when = finishesWhen(item.done_when)}
            {@const state = workState(item.status)}
            {@const liveLong = live.length > 0 ? `Running in ${live.map((x) => `${displayName(x, true)} on ${x.host_alias}`).join(', ')}` : 'No session on it'}
            <li class="task-card" class:is-done={item.status === 'done'} data-testid="handoff-task-card">
              <StatusDot {state} size={8} />
              <button
                type="button"
                class="handoff task"
                title={item.title}
                data-testid="handoff-task"
                onclick={() => showTaskInWorkView(`item:${item.id}`)}
              >
                <span class="what">#TASK</span>
                <span class="target">{item.title}</span>
              </button>
              <span class="live" title={liveLong} data-testid="handoff-task-live"
                ><span class="sr-only">{liveLong}</span><span aria-hidden="true"
                  >{live.length > 0 ? live.map((x) => `${displayName(x, true)} · ${x.host_alias}`).join(', ') : STATE_WORD[state]}</span
                ></span
              >
              <div class="actions">
                {#if taskUndoable(item, nowSec)}
                  <Button variant="quiet" size="sm" disabled={busy === h.id} onclick={() => void taskAction(h, () => undoWorkAccept([item.id]))} testid="handoff-task-undo"
                    >Undo</Button
                  >
                {/if}
                {#if item.status !== 'done'}
                  <Button variant="quiet" size="sm" disabled={busy === h.id} onclick={() => void taskAction(h, () => setWorkStatus(item.id, 'done'))} testid="handoff-task-done"
                    >Move to Done</Button
                  >
                {/if}
                {#if (item.done_when ?? []).includes('person')}
                  <Button
                    variant="quiet"
                    size="sm"
                    disabled={busy === h.id}
                    onclick={() => void taskAction(h, () => verifyWorkItem(item.id, 'person', true))}
                    testid="handoff-task-verify">Mark verified</Button
                  >
                {/if}
                <Button variant="quiet" size="sm" onclick={() => showTaskInWorkView(`item:${item.id}`)} testid="handoff-task-open">Open in Work</Button>
              </div>
              {#if fresh || when || errors[h.id]}
                <div class="more">
                  {#if fresh}<p class="sub" data-testid="handoff-task-drafted">Drafted from your message</p>{/if}
                  {#if when}<p class="sub" data-testid="handoff-task-when">{when}</p>{/if}
                  {#if fresh}
                    <form
                      class="draft"
                      onsubmit={(e) => {
                        e.preventDefault();
                        void saveDraft(h, item);
                      }}
                    >
                      <label>Owner <input bind:value={owner[h.id]} placeholder="You, or a name" data-testid="handoff-task-owner" /></label>
                      <label>Due <input type="date" bind:value={due[h.id]} data-testid="handoff-task-due" /></label>
                      <Button
                        variant="quiet"
                        size="sm"
                        type="submit"
                        disabled={busy === h.id || (!parseAssignees(owner[h.id] ?? '').length && !due[h.id])}
                        testid="handoff-task-save">Save</Button
                      >
                      {#if saved[h.id]}<span class="sub" role="status">Saved {saved[h.id]}</span>{/if}
                    </form>
                  {/if}
                  {#if errors[h.id]}<p class="error" role="alert">{errors[h.id]}</p>{/if}
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>
{/if}

<style>
  .handoffs {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .handoff {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-raise);
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    text-align: left;
    cursor: pointer;
  }
  /* Step 9.13: the comet crosses from the transcript onto the chip, which
     is hidden until it lands and then settles. */
  .slot {
    position: relative;
    display: flex;
  }
  .slot > .handoff {
    flex: 1;
  }
  .flight {
    position: absolute;
    top: 50%;
    left: 0;
    display: inline-flex;
    transform: translate(-100%, -50%);
    pointer-events: none;
    animation: handoff-fly var(--dur-slow) ease-out forwards;
  }
  @keyframes handoff-fly {
    from {
      left: 0;
      opacity: 0;
    }
    30% {
      opacity: 1;
    }
    to {
      left: 100%;
      opacity: 1;
    }
  }
  .handoff.in-flight {
    opacity: 0;
  }
  .handoff.settled {
    animation: handoff-settle var(--dur-base) ease-out;
  }
  @keyframes handoff-settle {
    from {
      opacity: 0.4;
      transform: scale(0.96);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .handoff:disabled,
  .handoff.done {
    cursor: default;
  }
  .what {
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .target {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--text-sm);
  }
  .tree label {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
  }
  /* Control chat UX (2026-10-10): one line per task under a header. */
  .task-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .group-head {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    padding: 2px var(--space-1);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .group-head:hover:not(:disabled) {
    color: var(--fg);
  }
  .group-head:disabled {
    cursor: default;
  }
  .group-head:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .chev {
    display: inline-block;
    width: 1ch;
  }
  .task-rows {
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
    overflow: hidden;
  }
  .task-rows[hidden] {
    display: none;
  }
  .task-card {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto;
    align-items: center;
    column-gap: var(--space-2);
    min-height: 28px;
    padding: 2px var(--space-2);
    font-size: var(--text-xs);
  }
  .task-card + .task-card {
    border-top: 1px solid var(--border);
  }
  .task-card:hover,
  .task-card:focus-within {
    background: color-mix(in srgb, var(--fg) 4%, transparent);
  }
  .task-card > .handoff {
    border: 0;
    padding: 0;
    background: transparent;
  }
  .task-card > .handoff:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    border-radius: var(--radius-sm);
  }
  .task-card.is-done .target {
    color: var(--fg-muted);
  }
  .task-card.is-done {
    opacity: 0.7;
  }
  .live {
    max-width: 22ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-muted);
  }
  /* The row's actions show on hover or focus; they stay in the tab order. */
  .task-card .actions {
    flex-wrap: nowrap;
    gap: 0;
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur-fast) ease-out;
  }
  .task-card:hover .actions,
  .task-card:focus-within .actions {
    opacity: 1;
    pointer-events: auto;
  }
  @media (hover: none) {
    .task-card .actions {
      opacity: 1;
      pointer-events: auto;
    }
  }
  .more {
    grid-column: 2 / -1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-bottom: 2px;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .sub,
  .wave,
  .dup {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .wave {
    font-weight: 500;
    margin-top: var(--space-1);
  }
  .dup {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-wrap: wrap;
  }
  .draft,
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    font-size: var(--text-xs);
  }
  .draft input {
    font: inherit;
    font-size: var(--text-xs);
  }
  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-xs);
  }
</style>
