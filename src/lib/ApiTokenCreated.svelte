<script lang="ts">
  // "Token created" (M15 step G2.8): the one place a named token is ever
  // shown. Copy token, Copy as env line, Done; closing forgets it, and the
  // table shows only its name and last use from then on.
  import Modal from './Modal.svelte';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import type { ApiTokenCreated } from './api_tokens';

  let { created, onclose }: { created: ApiTokenCreated; onclose: () => void } = $props();

  /** The token as the sheet prints it: its head and tail, never the whole
   *  value on screen (Copy carries the whole). */
  const shown = $derived(
    created.token.length > 24 ? `${created.token.slice(0, 16)}…${created.token.slice(-3)}` : created.token,
  );

  async function copy(text: string, what: string) {
    if (await copyText(text)) push({ kind: 'success', message: `${what} copied.` });
  }
</script>

<Modal label="Token created" {onclose} width="440px" testid="api-token-created">
  <div class="sheet">
    <h3>Token created</h3>
    <p class="lead">Copy it now. It is not shown again.</p>
    <code class="token" data-testid="api-token-value">{shown}</code>
    <div class="row">
      <button type="button" class="btn" data-testid="api-token-copy" onclick={() => void copy(created.token, 'Token')}
        >Copy token</button
      >
      <button type="button" class="btn" data-testid="api-token-copy-env" onclick={() => void copy(created.env_line, 'Env line')}
        >Copy as env line</button
      >
      <span class="grow"></span>
      <button type="button" class="btn primary" data-testid="api-token-done" onclick={onclose}>Done</button>
    </div>
  </div>
</Modal>

<style>
  .sheet {
    padding: 1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h3 {
    margin: 0;
    font-size: var(--text-md);
  }
  .lead {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .token {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    word-break: break-all;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .grow {
    flex: 1;
  }
</style>
