<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import Modal from './Modal.svelte';

  // Single-value text prompt on top of Modal — the in-app replacement for
  // window.prompt(), which WKWebView answers with null (so "New branch" never
  // worked on macOS). `validate` returns an error string to block submit.
  let {
    title,
    label,
    initialValue = '',
    placeholder = '',
    confirmLabel = 'OK',
    validate,
    onsubmit,
    oncancel,
    children,
  }: {
    title: string;
    label: string;
    initialValue?: string;
    placeholder?: string;
    confirmLabel?: string;
    validate?: (value: string) => string | null;
    onsubmit: (value: string) => void;
    oncancel: () => void;
    /** Extra controls rendered between the input and the buttons. */
    children?: Snippet;
  } = $props();

  // Seed once; the prompt owns the value from here on.
  let value = $state(untrack(() => initialValue));
  const trimmed = $derived(value.trim());
  const error = $derived(trimmed === '' ? null : (validate?.(trimmed) ?? null));
  const canSubmit = $derived(trimmed !== '' && error === null);

  function submit(e?: Event) {
    e?.preventDefault();
    if (!canSubmit) return;
    onsubmit(trimmed);
  }
</script>

<Modal {title} onclose={oncancel} width="400px" testid="prompt-dialog">
  <form class="form" onsubmit={submit}>
    <label class="field">
      <span>{label}</span>
      <input
        type="text"
        bind:value
        {placeholder}
        data-autofocus=""
        autocomplete="off"
        spellcheck="false"
        data-testid="prompt-input"
        aria-invalid={error !== null}
      />
    </label>
    {#if error}
      <p class="err" data-testid="prompt-error">{error}</p>
    {/if}
    {#if children}
      {@render children()}
    {/if}
    <div class="actions">
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="submit" class="primary" disabled={!canSubmit} data-testid="prompt-submit">{confirmLabel}</button>
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input {
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
  }
  .field input[aria-invalid='true'] { border-color: #e64a4a; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
