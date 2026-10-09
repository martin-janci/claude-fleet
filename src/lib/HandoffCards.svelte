<!--
  What Control's agent handed on (Orbit Fleet redesign steps 9.3 and 9.6),
  drawn in its transcript: "Sent to a session" and "Sent
  to a mission" chips that follow their target's live state and open it, a
  live card per task it created, and its proposed tree of subtasks as a card
  where the person unticks what they do not want, then creates the tasks or
  a mission, with Undo for ten minutes. The receipts are the backend's
  (`handoffs.ts`); nothing here is inferred from the transcript's text.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Loader from './Loader.svelte';
  import { durationMs, effectiveMotion } from './motion';
  import { fliesIn } from './handoff_flight';
  import Button from './kit/Button.svelte';
  import QuestionCard from './kit/QuestionCard.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import {
    createFromTree,
    followHandoffs,
    openProposals,
    recentHandoffs,
    sessionChip,
    undoTree,
    undoable,
    workState,
    ACCEPT_UNDO_SECS,
    type ControlHandoff,
  } from './handoffs';
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

  function minutesLeft(h: ControlHandoff): number {
    const at = Math.max(...undoable(h, nowSec).map((i) => i.accepted_at ?? 0));
    return Math.max(1, Math.ceil((at + ACCEPT_UNDO_SECS - nowSec) / 60));
  }
</script>

{#if $recentHandoffs.length > 0}
  <div class="handoffs" data-testid="handoffs">
    {#each $recentHandoffs as h (h.id)}
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
      {:else if h.kind === 'task' && h.item}
        {@const item = h.item}
        <button
          type="button"
          class="handoff task"
          data-testid="handoff-task"
          onclick={() => showTaskInWorkView(`item:${item.id}`)}
        >
          <span class="what">#TASK</span>
          <span class="target">{item.title}</span>
          <StatusChip state={workState(item.status)} />
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
            <ul class="tree">
              {#each open as item (item.id)}
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
                </li>
              {/each}
            </ul>
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
  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-xs);
  }
</style>
