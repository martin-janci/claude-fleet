<script lang="ts">
  // The prompt tuner (`prompt_tune.ts`): a model rewrites the draft into a
  // clearer prompt for the agent. The panel asks, then shows the rewrite
  // beside the draft (editable, or as the changes) with what changed; the
  // person accepts it into the box (`onaccept`), or discards it. Nothing is
  // ever sent from here.
  import { onMount } from 'svelte';
  import DraftedLabel from './DraftedLabel.svelte';
  import { draftedBy } from './ai_proposal';
  import { lineDiff } from './tool_view';
  import { tablistKeys } from './tablist_keys';
  import { TUNE_ASKS, tuneBlocked, type TuneAnswer } from './prompt_tune';
  import type { Result } from './result';

  let {
    model,
    draft,
    tune,
    onaccept,
    onclose,
    acceptLabel = 'Use this',
    testid = 'prompt-tune',
  }: {
    /** Who rewrites, for the heading (`haiku`). */
    model: string;
    /** The draft as it is in the box now. */
    draft: string;
    tune: (ask: string) => Promise<Result<TuneAnswer>>;
    /** Put the (possibly edited) rewrite in the box. */
    onaccept: (prompt: string) => void;
    onclose: () => void;
    acceptLabel?: string;
    testid?: string;
  } = $props();

  let ask = $state('');
  let busy = $state(false);
  let answer = $state<TuneAnswer | null>(null);
  /** The rewrite as the person edits it before accepting. */
  let edited = $state('');
  /** The draft the answer was tuned from: the diff's left side. */
  let tunedFrom = $state('');
  let view = $state<'rewrite' | 'changes'>('rewrite');
  let error = $state<string | null>(null);
  let input: HTMLInputElement | undefined = $state();

  const blocked = $derived(tuneBlocked(draft));
  const diff = $derived(answer ? lineDiff(tunedFrom, edited) : null);
  const meta = $derived(answer ? draftedBy(answer.model, answer.host_alias || null) : '');
  /** The box changed since this tune: accepting would drop what was typed. */
  const stale = $derived(answer !== null && draft !== tunedFrom);

  onMount(() => input?.focus());

  async function run(a = ask) {
    if (busy || blocked) return;
    busy = true;
    error = null;
    const from = draft;
    const r = await tune(a);
    busy = false;
    if (r.ok) {
      answer = r.value;
      edited = r.value.prompt;
      tunedFrom = from;
      view = 'rewrite';
    } else {
      error = r.error.message;
    }
  }

  function quick(a: string) {
    ask = a;
    void run(a);
  }

  function onKey(e: KeyboardEvent) {
    // Nothing typed here may reach the composer under it.
    e.stopPropagation();
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void run();
    }
  }

  function accept() {
    if (!answer || stale || edited.trim() === '') return;
    onaccept(edited);
    onclose();
  }

  // Esc anywhere in the panel (a chip, Use this, ×) closes the tuner and
  // goes no further: under it is a composer or a form whose Esc does more.
  function onPanelKey(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented) return;
    e.preventDefault();
    e.stopPropagation();
    onclose();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="tune" data-testid={testid} aria-label="Tune prompt with {model}" onkeydown={onPanelKey}>
  <div class="head">
    <h3>Tune prompt</h3>
    <span class="muted">{model} rewrites your draft for the agent. Nothing is sent.</span>
    <button
      type="button"
      class="btn btn--icon btn--quiet close"
      data-testid="{testid}-close"
      aria-label="Close the tuner"
      title="Close (Esc)"
      onclick={onclose}>×</button
    >
  </div>
  <div class="ask">
    <input
      bind:this={input}
      bind:value={ask}
      class="q"
      data-testid="{testid}-ask"
      aria-label="What should the rewrite do?"
      placeholder="What should it improve? (optional, Enter tunes)"
      onkeydown={onKey}
    />
    <button
      type="button"
      class="btn btn--primary"
      data-testid="{testid}-run"
      disabled={busy || blocked !== null}
      title={blocked ?? 'Tune (Enter)'}
      onclick={() => void run()}>{busy ? 'Tuning…' : answer ? 'Tune again' : 'Tune'}</button
    >
  </div>
  <div class="quick" role="group" aria-label="Quick asks">
    {#each TUNE_ASKS as a (a)}
      <button
        type="button"
        class="btn btn--chip"
        data-testid="{testid}-quick"
        disabled={busy || blocked !== null}
        title={blocked ?? undefined}
        onclick={() => quick(a)}>{a}</button
      >
    {/each}
  </div>
  {#if error}
    <p class="error" role="alert" data-testid="{testid}-error">{error}</p>
  {/if}
  {#if answer}
    {#if answer.notes}
      <p class="notes" data-testid="{testid}-notes">{answer.notes}</p>
    {/if}
    {#if answer.unchanged}
      <p class="muted" data-testid="{testid}-unchanged">Your draft already reads clearly; nothing was changed.</p>
    {:else}
      <div class="tabs" role="tablist" aria-label="Rewrite view" use:tablistKeys>
        <button
          type="button"
          role="tab"
          class="btn btn--chip"
          class:on={view === 'rewrite'}
          aria-selected={view === 'rewrite'}
          data-testid="{testid}-view-rewrite"
          onclick={() => (view = 'rewrite')}>Rewrite</button
        >
        <button
          type="button"
          role="tab"
          class="btn btn--chip"
          class:on={view === 'changes'}
          aria-selected={view === 'changes'}
          data-testid="{testid}-view-changes"
          onclick={() => (view = 'changes')}>Changes{diff ? ` +${diff.added} −${diff.removed}` : ''}</button
        >
      </div>
      {#if view === 'rewrite'}
        <textarea
          class="rewrite"
          data-testid="{testid}-rewrite"
          aria-label="The rewrite, editable before you use it"
          rows="6"
          bind:value={edited}
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === 'Escape') {
              e.preventDefault();
              onclose();
            }
          }}
        ></textarea>
      {:else if diff}
        <pre class="diff" data-testid="{testid}-diff">{#each diff.rows as r, i (i)}<span
              class="row {r.kind}">{r.kind === 'gap'
                ? `  … ${r.hidden} unchanged`
                : `${r.kind === 'add' ? '+' : r.kind === 'del' ? '−' : ' '} ${r.text}`}
</span>{/each}</pre>
      {/if}
      {#if stale}
        <p class="muted" data-testid="{testid}-stale">Your draft changed since this tune. Tune again to use it.</p>
      {/if}
      <div class="actions">
        <button
          type="button"
          class="btn btn--primary"
          data-testid="{testid}-accept"
          disabled={stale || edited.trim() === ''}
          title={stale
            ? 'Your draft changed since this tune: tune again so nothing you typed is lost.'
            : 'Replace the draft in the box with this. It is not sent: your Send sends it.'}
          onclick={accept}>{acceptLabel}</button
        >
        <button type="button" class="btn" data-testid="{testid}-discard" onclick={onclose}>Discard</button>
      </div>
    {/if}
    <p class="muted drafted">
      <DraftedLabel testid="{testid}-drafted" />
      <span data-testid="{testid}-meta">{meta}</span>
    </p>
  {/if}
</section>

<style>
  .tune {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
    box-shadow: var(--shadow-pop);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .close {
    margin-left: auto;
  }
  h3 {
    margin: 0;
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .ask,
  .quick,
  .tabs,
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .q {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
  }
  .tabs .on {
    background: var(--accent-soft);
    color: var(--fg);
    font-weight: 500;
  }
  .notes {
    margin: 0;
    white-space: pre-wrap;
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .rewrite {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .diff {
    margin: 0;
    max-height: 16rem;
    overflow: auto;
    font-size: var(--text-2xs);
    line-height: 1.5;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .row.add {
    color: var(--diff-add-fg);
  }
  .row.del {
    color: var(--diff-del-fg);
    text-decoration: line-through;
  }
  .row.gap {
    color: var(--fg-muted);
  }
  .muted {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .drafted {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
