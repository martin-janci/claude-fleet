<script lang="ts">
  // Desktop confirmation for destructive control-API calls. When the
  // `mcp.confirm_destructive` toggle is on, the backend answers
  // broadcast_prompt / kill_session / delete_worktree / set_clipboard with
  // E_CONFIRM_REQUIRED and emits `mcp:confirm-required`; this dialog shows
  // the queue and answers via `mcp_confirm`. Always mounted (App.svelte) so
  // a request is never missed while Settings is closed.
  import { onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import {
    mcpConfirm,
    mcpPendingConfirms,
    MCP_CONFIRM_EVENT,
    type ConfirmRequest,
  } from './mcp';

  let queue = $state<ConfirmRequest[]>([]);
  let busy = $state(false);
  const current = $derived(queue[0] ?? null);

  function enqueue(req: ConfirmRequest) {
    if (queue.some((q) => q.nonce === req.nonce)) return;
    queue = [...queue, req];
  }

  async function answer(approved: boolean) {
    if (!current || busy) return;
    busy = true;
    const nonce = current.nonce;
    await mcpConfirm(nonce, approved);
    busy = false;
    queue = queue.filter((q) => q.nonce !== nonce);
  }

  onMount(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    // Requests raised before this view mounted (e.g. after a reload).
    void mcpPendingConfirms().then((r) => {
      if (r.ok && Array.isArray(r.value)) {
        for (const p of r.value) enqueue({ ...p, summary: '', caller: '' });
      }
    });
    void listen<ConfirmRequest>(MCP_CONFIRM_EVENT, (e) => enqueue(e.payload)).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  });
</script>

{#if current}
  <div class="modal-backdrop" role="presentation">
    <div class="dialog" role="dialog" aria-label="Confirm control-API call" data-testid="mcp-confirm">
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
        <button class="deny" disabled={busy} onclick={() => answer(false)} data-testid="mcp-confirm-deny">
          Deny
        </button>
        <button class="approve" disabled={busy} onclick={() => answer(true)} data-testid="mcp-confirm-approve">
          Approve
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.45);
    display: flex; align-items: center; justify-content: center;
    z-index: 30;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 440px;
    max-width: calc(100vw - 32px);
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h3 { margin: 0; font-size: 1rem; }
  .who { margin: 0; font-size: 0.85rem; }
  .muted { margin: 0; font-size: 0.75rem; color: var(--fg-muted); }
  code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .summary {
    margin: 0;
    font-size: 0.75rem;
    background: var(--bg-alt, rgba(127, 127, 127, 0.12));
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
  .deny:hover:not(:disabled) { color: #e64a4a; border-color: #e64a4a; }
</style>
