<!--
  Redesign step 3.11: an editable LLM draft, "Drafted · by haiku on mercury
  · from 3 changed files · Regenerate · Clear". The draft is always the
  person's to edit; it keeps the ai-pre ring (controls.css) until they
  touch it. While the
  LLM writes, the field says so beside a small Atom (the Loader kit's
  agent-thinking loader), which waits the manual's 400 ms so a quick
  draft never flashes it.
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

  function clear() {
    value = '';
    touched = false;
    onclear?.();
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
    oninput={() => (touched = true)}
  ></textarea>
</label>
{#if busy || value.trim() !== ''}
  <div class="why" data-testid="{testid}-meta">
    <DraftedLabel testid="{testid}-drafted" />
    {#if busy}
      <Loader name="atom" size={20} stage={false} testid="{testid}-atom" />
      <span data-testid="{testid}-busy">Drafting…</span>
    {:else if source}
      <span>{source}</span>
    {/if}
    {#if onregenerate}
      <span aria-hidden="true">·</span>
      <button
        type="button"
        class="link"
        data-testid="{testid}-regenerate"
        disabled={busy}
        onclick={() => {
          touched = false;
          onregenerate?.();
        }}>Regenerate</button
      >
    {/if}
    <span aria-hidden="true">·</span>
    <button type="button" class="link" data-testid="{testid}-clear" disabled={busy} onclick={clear}>Clear</button>
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
