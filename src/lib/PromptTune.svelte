<script lang="ts" module>
  import { errorText } from './error_copy';
  import type { Result as R } from './result';
  import type { TuneAnswer as T } from './prompt_tune';

  /** A run the panel stopped waiting for (closed, or Stop waiting) while it
   *  was still going: the host still runs it and books it, so reopening the
   *  tuner on the same draft picks it up instead of paying for another. */
  interface Left {
    promise: Promise<R<T>>;
    /** The draft in the box it was asked from. */
    from: string;
    ask: string;
    at: number;
  }
  const left = new Map<string, Left>();
  /** How long a left run is kept for a reopen. */
  export const LEFT_TUNE_TTL_MS = 10 * 60_000;
  function leftKey(testid: string, from: string): string {
    return `${testid}\u0000${from}`;
  }
  /** Tests: forget every left run. */
  export function _forgetLeftTunes(): void {
    left.clear();
  }
</script>

<script lang="ts">
  // The prompt tuner (`prompt_tune.ts`): a model rewrites the draft into a
  // clearer prompt for the agent. The panel asks, then shows the rewrite
  // beside the draft (editable, or as the changes) with what changed; the
  // person accepts it into the box (`onaccept`), or discards it. Nothing is
  // ever sent from here.
  import { onMount, onDestroy } from 'svelte';
  import DraftedLabel from './DraftedLabel.svelte';
  import CopyButton from './CopyButton.svelte';
  import { push as pushToast } from './toasts';
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
    /** Tune `from`: the box's draft, or the rewrite on screen once there is
     *  one (Tune again and the quick asks build on it). */
    tune: (ask: string, from: string) => Promise<Result<TuneAnswer>>;
    /** Put the (possibly edited) rewrite in the box; called again with the
     *  draft as it was when the person picks the toast's Undo. */
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
  /** The run being waited for, and its number: a stale number's answer is
   *  dropped (Stop waiting, or a newer run). */
  let pending: Left | null = null;
  let seq = 0;
  const diff = $derived(answer ? lineDiff(tunedFrom, edited) : null);
  const meta = $derived(answer ? draftedBy(answer.model, answer.host_alias || null) : '');
  /** The box changed since this tune: accepting would drop what was typed. */
  const stale = $derived(answer !== null && draft !== tunedFrom);

  onMount(() => {
    input?.focus();
    // A run this panel was closed on, for the same draft: show it (still
    // going, or done) rather than start another paid one.
    const key = leftKey(testid, draft);
    const was = left.get(key);
    left.delete(key);
    if (was && Date.now() - was.at < LEFT_TUNE_TTL_MS) {
      ask = was.ask;
      void wait(was);
    }
  });

  onDestroy(() => leave());

  /** Stop waiting for the run in flight; keep it for a reopen. */
  function leave() {
    if (!pending) return;
    left.set(leftKey(testid, pending.from), { ...pending, at: Date.now() });
    pending = null;
    seq++;
    busy = false;
  }

  async function wait(p: Left) {
    const mine = ++seq;
    pending = p;
    busy = true;
    error = null;
    const r = await p.promise;
    if (mine !== seq) return;
    pending = null;
    busy = false;
    if (r.ok) {
      answer = r.value;
      edited = r.value.prompt;
      tunedFrom = p.from;
      view = 'rewrite';
    } else {
      error = errorText(r.error);
    }
  }

  async function run(a = ask) {
    if (busy || blocked) return;
    // Once a rewrite is on screen (and the box has not changed since),
    // tune that, with the person's edits; the diff and the stale check
    // still compare against the draft it all started from.
    const onRewrite = answer !== null && !answer.unchanged && !stale && edited.trim() !== '';
    const from = onRewrite ? tunedFrom : draft;
    await wait({ promise: tune(a, onRewrite ? edited : draft), from, ask: a, at: 0 });
  }

  /** A chip's words after what the person typed, so a tap never throws
   *  their words away; the field keeps only what they typed. */
  function quick(a: string) {
    const typed = ask.trim();
    if (typed === '' || (TUNE_ASKS as readonly string[]).includes(typed)) {
      ask = a;
      void run(a);
    } else {
      void run(`${typed}; ${a}`);
    }
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
    const before = draft;
    onaccept(edited);
    onclose();
    // Set from code, so the box's own Ctrl+Z cannot bring the draft back.
    pushToast({ kind: 'info', message: 'Draft replaced', action: { label: 'Undo', run: () => onaccept(before) } });
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
<section class="tune" data-testid={testid} aria-label="Tune prompt with {model}" aria-busy={busy} onkeydown={onPanelKey}>
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
      title={blocked ?? (answer && !answer.unchanged && !stale ? 'Tune the rewrite again (Enter)' : 'Tune (Enter)')}
      onclick={() => void run()}>{busy ? 'Tuning…' : answer ? 'Tune again' : 'Tune'}</button
    >
    {#if busy}
      <button
        type="button"
        class="btn"
        data-testid="{testid}-stop"
        title="Stop waiting. The run still finishes on the host and is billed; reopen the tuner on this draft to see it."
        onclick={leave}>Stop waiting</button
      >
    {/if}
  </div>
  {#if blocked && !busy}
    <p class="muted" data-testid="{testid}-blocked">{blocked}.</p>
  {/if}
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
  <div class="said" aria-live="polite">
    {#if answer}
      {#if answer.notes}
        <p class="notes" data-testid="{testid}-notes">{answer.notes}</p>
      {/if}
      {#if answer.unchanged}
        <p class="muted" data-testid="{testid}-unchanged">Your draft already reads clearly; nothing was changed.</p>
      {/if}
    {/if}
  </div>
  {#if answer}
    {#if !answer.unchanged}
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
            } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              accept();
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
      {#if answer.cut}
        <p class="warn" data-testid="{testid}-cut">
          The rewrite ran past the limit and was cut at its end (…). Read its end before you use it.
        </p>
      {/if}
      <div class="actions">
        <button
          type="button"
          class="btn btn--primary"
          data-testid="{testid}-accept"
          disabled={stale || edited.trim() === ''}
          title={stale
            ? 'Your draft changed since this tune: tune again so nothing you typed is lost.'
            : 'Replace the draft in the box with this (Mod+Enter in the rewrite). It is not sent: your Send sends it.'}
          onclick={accept}>{answer.cut ? `${acceptLabel} (cut)` : acceptLabel}</button
        >
        <CopyButton text={edited} label="Copy rewrite" />
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
  .warn {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--status-waiting);
  }
  .said:empty {
    display: none;
  }
  .error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
