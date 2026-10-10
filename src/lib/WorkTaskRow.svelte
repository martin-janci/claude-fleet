<!-- One task in the Work list (redesign board "Work · tasks with filters
     open", manual: SessionRow): the status dot, key and title, where it
     stands and why, then chips for its live session, its pull request and
     what it cost. The whole row selects the task; a session chip opens that
     session. Shared by the grouped tree and the List layout. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { STATE_WORD } from './kit/status';
  import { sessions } from './sessions';
  import { unavailableLabel } from './trackers';
  import { costChip, linkTone, liveLinks, occurrenceTitle, prChip, taskLine, taskTone } from './work_row';
  import { isOccurrenceOf, taskLabel, trackerDown, trackerDownLabel, type WorkTask, type WorkTaskLink } from './work_view';

  let {
    task: t,
    selected = false,
    currentSessionId = null,
    onselect,
    onopen,
    title,
    trailing,
    lookup,
  }: {
    task: WorkTask;
    selected?: boolean;
    /** The session in focus: its chip, and the row, are lit. */
    currentSessionId?: number | null;
    onselect: () => void;
    /** Open one of its sessions (a session chip). */
    onopen?: (l: WorkTaskLink) => void;
    /** The title as the caller words it (a derived title, a bare id). */
    title?: string;
    /** Actions after the body (the List layout's edit and start). */
    trailing?: Snippet;
    /** The task a dependency id names, when the view has it loaded: a
     *  blocked task's line names it by key (step 6.3). */
    lookup?: (id: string) => Pick<WorkTask, 'key' | 'title'> | null | undefined;
  } = $props();

  const rows = $derived(new Map($sessions.map((s) => [s.id, s])));
  const tone = $derived(taskTone(t));
  const line = $derived(taskLine(t, lookup));
  const live = $derived(liveLinks(t));
  const pr = $derived(prChip(t, rows));
  const cost = $derived(costChip(t));
  /** An epic's, or any parent's, roll-up (sprints design §3). */
  const rollup = $derived((t.children_total ?? 0) > 0 ? `${t.children_done ?? 0}/${t.children_total} done` : null);
  const lit = $derived(live.some((l) => isOccurrenceOf(l, currentSessionId)));
  const shown = $derived(title ?? (t.title || (t.key ? '' : t.task_id)));
</script>

<div class="of-row work-row" class:lit aria-selected={selected ? 'true' : undefined} data-testid="work-task-body">
  <StatusDot state={tone} label={STATE_WORD[tone]} />
  <div class="body">
    <button
      class="main"
      type="button"
      data-testid="work-task-row"
      aria-current={selected ? 'true' : undefined}
      title={t.unavailable ? unavailableLabel(t.unavailable_reason) : taskLabel(t)}
      onclick={onselect}
    >
      <span class="l1">
        {#if t.key}<span class="key">{t.key}</span>{/if}
        <span class="title" class:unavailable={t.unavailable} class:derived={t.title_derived}>{shown}</span>
        {#if t.needs_you}<span class="sr" data-testid="work-task-needs-you">needs you</span>{/if}
        {#if t.review}<span class="review" data-testid="work-task-review" title="Something to review">?</span>{/if}
      </span>
      <span class="line" data-testid="work-task-line"
        >{#if line.failed}<span class="f">{line.lead}</span>{:else}{line.lead}{/if}{#if line.why}{' · '}{line.why}{/if}</span
      >
    </button>
    {#if live.length > 0 || pr || cost || trackerDown(t) || (t.open_proposals ?? 0) > 0 || t.epic || rollup}
      <div class="chips">
        {#if t.epic}<span class="of-chip accent" data-testid="work-task-epic">Epic</span>{/if}
        {#if rollup}<span class="of-chip tnum" data-testid="work-task-rollup" title="Tasks filed under it that are done">{rollup}</span>{/if}
        {#if live.length === 1}
          {@const l = live[0]}
          <button
            class="of-chip chip-btn"
            class:current={isOccurrenceOf(l, currentSessionId)}
            type="button"
            data-testid="work-occurrence"
            data-kind={l.primary ? 'primary' : 'secondary'}
            data-session-id={l.session_id ?? ''}
            title={occurrenceTitle(l)}
            onclick={() => onopen?.(l)}
            ><StatusDot state={linkTone(l)} size={6} label={null} />{l.name ?? `session ${l.session_id}`}</button
          >
        {:else if live.length > 1}
          <button
            class="of-chip chip-btn"
            type="button"
            data-testid="work-task-sessions"
            title={live.map((l) => l.name ?? `session ${l.session_id}`).join(', ')}
            onclick={onselect}
            ><StatusDot state={live.some((l) => l.needs_you) ? 'waiting' : 'working'} size={6} label={null} />{live.length} sessions</button
          >
        {/if}
        {#if pr}
          <span class="of-chip" data-testid="work-task-pr" title={pr.url}
            >{pr.label}{#if pr.checks === 'passing'}<span class="ok" aria-label="checks passing">✓</span>{:else if pr.checks === 'failing'}<span
                class="bad"
                aria-label="{pr.failing} failing">✕ {pr.failing}</span
              >{:else if pr.checks === 'running'}<span class="run" aria-label="checks running">…</span>{/if}</span
          >
        {/if}
        {#if trackerDown(t)}<span class="of-chip failed" data-testid="work-task-tracker-down" title={`tracker state: ${t.tracker_state}`}
            >{trackerDownLabel(t)}</span
          >{/if}
        {#if cost}<span class="of-chip tnum" data-testid="work-task-cost" title="What its sessions have spent">{cost}</span>{/if}
        {#if (t.open_proposals ?? 0) > 0}
          <span class="of-chip accent" data-testid="task-proposals-badge" title="Agent proposals — decide them on the task page"
            >{t.open_proposals} to review</span
          >
        {/if}
      </div>
    {/if}
  </div>
  {#if trailing}<span class="trailing">{@render trailing()}</span>{/if}
</div>

<style>
  .work-row {
    margin: 0 6px;
    position: relative;
  }
  .work-row.lit:not([aria-selected='true']) {
    box-shadow: inset 2px 0 0 var(--status-working);
  }
  .main {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .main::after {
    /* The whole row picks the task; the chips sit above this. */
    content: '';
    position: absolute;
    inset: 0;
  }
  .main:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 2px;
    border-radius: var(--radius-sm);
  }
  .l1 {
    min-width: 0;
  }
  .key {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 16px;
    color: var(--fg-muted);
  }
  .title.unavailable {
    text-decoration: line-through;
    color: var(--fg-muted);
  }
  .title.derived {
    font-style: italic;
  }
  .review {
    flex: none;
    font-size: var(--text-2xs);
    color: var(--status-waiting);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .chips {
    position: relative;
  }
  .chip-btn {
    border: 0;
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chip-btn:hover {
    color: var(--fg);
  }
  .chip-btn.current {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .ok {
    color: var(--status-done);
  }
  .bad {
    color: var(--status-failed);
  }
  .run {
    color: var(--status-working);
  }
  /* Row actions float over the row's right end while it is pointed at or
     focused, so a quiet row keeps its full width for the title. */
  .trailing {
    position: absolute;
    top: 4px;
    right: 6px;
    display: none;
    align-items: center;
    gap: 2px;
    padding: 0 2px;
    border-radius: var(--radius-sm);
    background: var(--bg-hover);
  }
  .work-row:hover .trailing,
  .work-row:focus-within .trailing,
  .trailing:has(:global(.pop-anchor), :global([role='status']), :global([data-testid='start-progress'])) {
    display: flex;
  }
</style>
