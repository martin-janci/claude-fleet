<script lang="ts">
  // A `progress` block (docs/chat-blocks.md). Blocks with the same id in one
  // conversation are one job: the first card shows the newest state, the
  // later ones are a line saying it moved on (progress_board.ts).
  // No loader here: a running job shows its count and steps, and a job
  // waiting on a person never animates.
  import { onDestroy } from 'svelte';
  import Markdown from '../MarkdownView.svelte';
  import { elapsedWords, peers, track, type ProgressBlock } from './progress_board';

  let { block }: { block: ProgressBlock } = $props();

  let el = $state<HTMLElement>();
  $effect(() => {
    if (el) return track(el, block);
  });

  const board = $derived(el ? peers(el, block.id) : { blocks: [block], home: true });
  const shown = $derived(board.home ? (board.blocks.at(-1) ?? block) : block);
  const updates = $derived(board.blocks.length - 1);

  const STATE_LABEL = { running: 'Working', waiting: 'Needs you', done: 'Done', failed: 'Failed' } as const;
  const STEP_MARK = { pending: '○', running: '◐', done: '✓', failed: '✕', skipped: '–' } as const;

  const pct = $derived(
    shown.total !== undefined ? Math.round((Math.min(shown.done ?? 0, shown.total) / shown.total) * 100) : null,
  );
  // The time since the job started, while it runs (G7.4): a second-hand
  // clock, stopped once the job is done or failed.
  let now = $state(Math.floor(Date.now() / 1000));
  const clock = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000);
  onDestroy(() => clearInterval(clock));
  const elapsed = $derived(
    shown.started_at !== undefined && (shown.state === 'running' || shown.state === 'waiting')
      ? elapsedWords(now - shown.started_at)
      : null,
  );

  const count = $derived.by(() => {
    const unit = shown.unit ? ` ${shown.unit}` : '';
    if (shown.total !== undefined) return `${shown.done ?? 0} of ${shown.total}${unit}`;
    if (shown.done !== undefined) return `${shown.done}${unit} so far`;
    return null;
  });
</script>

<div bind:this={el} class="progress-slot">
  {#if board.home}
    <section class="card {shown.state}" data-testid="rich-progress" data-state={shown.state} aria-label={shown.title}>
      <header>
        <strong>{shown.title}</strong>
        <span class="head-right">
          {#if elapsed}<span class="muted elapsed" data-testid="rich-progress-elapsed">{elapsed}</span>{/if}
          <span class="chip" data-testid="rich-progress-state">{STATE_LABEL[shown.state]}</span>
        </span>
      </header>
      {#if count !== null}
        <div class="count">
          {#if pct !== null}
            <div
              class="meter"
              role="progressbar"
              aria-valuemin="0"
              aria-valuemax={shown.total}
              aria-valuenow={shown.done ?? 0}
              aria-label={shown.title}>
              <div class="fill" style:width="{pct}%"></div>
            </div>
          {/if}
          <span class="muted" data-testid="rich-progress-count">{count}</span>
        </div>
      {/if}
      {#if shown.steps}
        <ol class="steps">
          {#each shown.steps as s, k (k)}
            <li class={s.state} data-testid="rich-progress-step" data-state={s.state}>
              <span class="mark" aria-hidden="true">{STEP_MARK[s.state]}</span>{s.title}
              {#if s.detail}<span class="step-detail muted" data-testid="rich-progress-step-detail">{s.detail}</span>{/if}
            </li>
          {/each}
        </ol>
      {/if}
      {#if shown.note}<Markdown source={shown.note} />{/if}
      {#if updates > 0}
        <p class="muted" data-testid="rich-progress-updates">{updates === 1 ? '1 update' : `${updates} updates`} below</p>
      {/if}
    </section>
  {:else}
    <p class="moved" data-testid="rich-progress-moved">↑ {block.title}: {STATE_LABEL[block.state].toLowerCase()}, shown above</p>
  {/if}
</div>

<style>
  .card {
    --tone: var(--accent);
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--tone);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .card.waiting { --tone: var(--usage-warn); }
  .card.done { --tone: var(--usage-ok); }
  .card.failed { --tone: var(--usage-crit); }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
  }
  .head-right { display: inline-flex; align-items: baseline; gap: 0.5rem; }
  .elapsed { font-size: var(--text-2xs); font-variant-numeric: tabular-nums; }
  .step-detail { display: block; margin-left: 1.2rem; font-size: var(--text-2xs); }
  .chip {
    padding: 0 0.45rem;
    border-radius: var(--radius-pill);
    border: 1px solid var(--tone);
    color: var(--tone);
    font-size: var(--text-2xs);
    white-space: nowrap;
  }
  .count {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .meter {
    flex: 1;
    height: 6px;
    border-radius: var(--radius-xs);
    background: var(--border);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--tone);
  }
  .muted,
  .moved {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    margin: 0;
  }
  .moved { margin: 0.2em 0 0.5em; }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.88em;
  }
  .mark {
    display: inline-block;
    width: 1.2rem;
    color: var(--fg-muted);
  }
  .steps .done .mark { color: var(--usage-ok); }
  .steps .failed .mark { color: var(--usage-crit); }
  .steps .running { font-weight: 600; }
  .steps .skipped { color: var(--fg-muted); text-decoration: line-through; }
</style>
