<script lang="ts">
  // Desktop confirmation for destructive control-API calls. When the
  // `mcp.confirm_destructive` toggle is on, the backend answers
  // broadcast_prompt / kill_session / delete_worktree / set_clipboard with
  // E_CONFIRM_REQUIRED and emits `mcp:confirm-required`; this dialog shows
  // the queue and answers via `mcp_confirm`. Always mounted (App.svelte) so
  // a request is never missed while Settings is closed.
  //
  // TRUST BOUNDARY: `mcp_confirm` is a plain Tauri command, so any script
  // running in this webview could auto-approve a nonce. The toggle protects
  // against agents on the MCP side, not against code running in the desktop
  // itself — the desktop is the trusted party here, by design. Do not expose
  // `mcp_confirm` to anything less trusted than this window.
  //
  // The queue is shared with Control's confirm cards (redesign step 9.2,
  // `confirms.ts`): in the New layout the operator's own requests are
  // answered as cards in its transcript while one is on screen, and this
  // dialog shows everything else.
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import { answerConfirm, answering, dialogConfirms, startConfirmQueue } from './confirms';

  const queue = $derived($dialogConfirms);
  const current = $derived(queue[0] ?? null);
  const busy = $derived(current !== null && $answering.has(current.nonce));

  async function answer(approved: boolean) {
    if (!current || busy) return;
    await answerConfirm(current.nonce, approved);
  }

  onMount(() => startConfirmQueue());
</script>

{#if current}
  <!-- Escape / backdrop = Deny: dismissing must never count as approval. -->
  <Modal label="Confirm control-API call" onclose={() => answer(false)} width="440px" testid="mcp-confirm">
    <div class="body">
      <h3>Approve <code>{current.tool}</code>?</h3>
      <p class="who">
        An agent{current.caller ? ` (${current.caller})` : ''} asked the control API to run
        <code>{current.tool}</code>.
      </p>
      {#if current.summary}
        <pre class="summary">{current.summary}</pre>
      {/if}
      {#if queue.length > 1}
        <p class="muted">{queue.length - 1} more waiting</p>
      {/if}
      <div class="actions">
        <button class="deny" disabled={busy} onclick={() => answer(false)} data-testid="mcp-confirm-deny" data-autofocus>
          Deny
        </button>
        <button class="approve" disabled={busy} onclick={() => answer(true)} data-testid="mcp-confirm-approve">
          Approve
        </button>
      </div>
    </div>
  </Modal>
{/if}

<style>
  .body { display: flex; flex-direction: column; gap: 0.6rem; }
  h3 { margin: 0; font-size: 1rem; }
  .who { margin: 0; font-size: 0.85rem; }
  .muted { margin: 0; font-size: 11px; color: var(--fg-muted); }
  code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .summary {
    margin: 0;
    font-size: 11px;
    background: var(--bg-sunk);
    padding: 0.4rem 0.5rem;
    border-radius: 4px;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
  .actions button {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    padding: 0.3rem 0.8rem;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .actions button:disabled { opacity: 0.5; cursor: default; }
  .approve:hover:not(:disabled) { border-color: var(--accent); }
  .deny:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
</style>
