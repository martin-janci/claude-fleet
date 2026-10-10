<!--
  Redesign step 3.11: an editable LLM draft, "Drafted · by haiku on mercury
  · from 3 changed files · Regenerate · Clear". The draft is always the
  person's to edit; it keeps the ai-pre ring (controls.css) until they
  touch it. While the
  LLM writes, the field says so beside a small Atom (the Loader kit's
  agent-thinking loader), which waits the manual's 400 ms so a quick
  draft never flashes it.
  G4.9 (AI patterns board): once edited the field says "Edited · your text
  now", and Regenerate asks before it replaces that text. Regenerate and
  Clear are AI-caused changes, so each offers Undo back to the text before.
-->
<script lang="ts">
  import { draftedBy } from './ai_proposal';
  import DraftedLabel from './DraftedLabel.svelte';
  import Loader from './Loader.svelte';

  let {
    value = $bindable(''),
    label,
    model,
    host,
    from,
    busy = false,
    rows = 3,
    placeholder = '',
    onregenerate,
    onclear,
    testid = 'draft-field',
  }: {
    value?: string;
    label: string;
    /** The model that wrote it (`haiku`). */
    model?: string | null;
    /** The host it ran on (`mercury`). */
    host?: string | null;
    /** What it read: "from 3 changed files". */
    from?: string | null;
    /** The LLM is writing. */
    busy?: boolean;
    rows?: number;
    placeholder?: string;
    onregenerate?: () => void;
    /** After the field was emptied. */
    onclear?: () => void;
    testid?: string;
  } = $props();

  /** The person has edited the draft: it is theirs now, no ring. */
  let touched = $state(false);
  const drafted = $derived(value.trim() !== '' && !touched);
  const source = $derived(draftedBy(model, host, from));
  /** Regenerate was pressed on edited text: ask before replacing it. */
  let asking = $state(false);
  /** The text a Regenerate or Clear replaced, for Undo. */
  let before = $state<{ text: string; touched: boolean } | null>(null);

  function remember() {
    before = value.trim() !== '' ? { text: value, touched } : null;
  }

  function regenerate() {
    if (touched && value.trim() !== '') {
      asking = true;
      return;
    }
    replace();
  }

  function replace() {
    asking = false;
    remember();
    touched = false;
    onregenerate?.();
  }

  function clear() {
    asking = false;
    remember();
    value = '';
    touched = false;
    onclear?.();
  }

  function undo() {
    if (!before) return;
    value = before.text;
    touched = before.touched;
    before = null;
  }
</script>

<label class="field" data-testid={testid}>
  <span class="label">{label}</span>
  <textarea
    class:ai-pre={drafted}
    data-testid="{testid}-input"
    {rows}
    {placeholder}
    aria-busy={busy}
    bind:value
    oninput={() => {
      touched = true;
      before = null;
      asking = false;
    }}
  ></textarea>
</label>
{#if asking}
  <div class="why" role="group" aria-label="Replace your text?" data-testid="{testid}-ask">
    <span>Replace your text with a new draft?</span>
    <button type="button" class="link" data-testid="{testid}-replace" onclick={replace}>Replace</button>
    <span aria-hidden="true">·</span>
    <button type="button" class="link" data-testid="{testid}-keep" onclick={() => (asking = false)}>Keep mine</button>
  </div>
{:else if busy || value.trim() !== '' || before}
  <div class="why" data-testid="{testid}-meta">
    {#if touched && !busy && value.trim() !== ''}
      <span class="pill" data-testid="{testid}-edited">Edited</span>
      <span>your text now</span>
    {:else if busy || value.trim() !== ''}
      <DraftedLabel testid="{testid}-drafted" />
      {#if busy}
        <Loader name="atom" size={20} stage={false} testid="{testid}-atom" />
        <span data-testid="{testid}-busy">Drafting…</span>
      {:else if source}
        <span>{source}</span>
      {/if}
    {/if}
    {#if onregenerate}
      <span aria-hidden="true">·</span>
      <button type="button" class="link" data-testid="{testid}-regenerate" disabled={busy} onclick={regenerate}
        >Regenerate</button
      >
    {/if}
    {#if value.trim() !== ''}
      <span aria-hidden="true">·</span>
      <button type="button" class="link" data-testid="{testid}-clear" disabled={busy} onclick={clear}>Clear</button>
    {/if}
    {#if before && !busy}
      <span aria-hidden="true">·</span>
      <button type="button" class="link" data-testid="{testid}-undo" onclick={undo}>Undo</button>
    {/if}
  </div>
{/if}

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .label {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  textarea {
    font: inherit;
    font-size: var(--text-xs);
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-raise);
    color: var(--fg);
    resize: vertical;
  }
  .why {
    font-size: var(--text-xs);
    line-height: 16px;
    color: var(--fg-muted);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .pill {
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    color: var(--fg-2);
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
  }
  .link {
    font: inherit;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    cursor: pointer;
  }
  .link:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .link:hover:not(:disabled) {
    text-decoration: underline;
  }
  .link:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
</style>
