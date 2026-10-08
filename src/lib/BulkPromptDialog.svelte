<script lang="ts">
  // Bulk "send prompt" for the sidebar's multi-select (FE-4). Same fan-out as
  // PromptComposer's send loop (one `send_prompt` per target, per-target
  // error/success), but the target list is fixed by the selection instead of
  // derived from a source session.
  import { sendPrompt, type SessionRow } from './sessions';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import Modal from './Modal.svelte';

  let {
    targets,
    onClose,
  }: {
    targets: SessionRow[];
    onClose: () => void;
  } = $props();

  let prompt = $state('');
  let sending = $state(false);
  let errors = $state<Record<number, string>>({});
  let succeeded = $state<Record<number, boolean>>({});

  /**
   * Narrowed per target, then again at the send (multi-user M1, F2b). Sidebar
   * hands this dialog a selection it has already narrowed with `bulkTargets` —
   * but this dialog re-derives its own fan-out from `targets` and the dialog
   * stays open, so a grant narrowed while it is open would otherwise send a
   * prompt into a session this client may no longer drive. `send_prompt` is
   * `drive` in `share.ts::SESSION_TIER`; the narrowing is asked here, over the
   * rows this dialog is actually going to write to.
   */
  const writable = $derived(bulkTargets(targets, 'send_prompt', $sessionBlocked));
  const sendable = $derived(
    writable.filter((t) => t.kind !== 'shell' && t.status === 'running'),
  );
  /** Rows the selection held that this client may not drive — counted so the
   *  dialog says so rather than quietly sending to fewer sessions. */
  const notMine = $derived(targets.length - writable.length);
  const hubBlocked = $derived(hubActionBlocked('send_prompt', $hubStatus, $hubConnection));
  const canSend = $derived(
    prompt.trim().length > 0 && sendable.length > 0 && !sending && hubBlocked === null,
  );

  async function send() {
    // Re-asked at the call: `canSend` reads the same two halves, and the
    // fan-out below walks `sendable`, which is the narrowed list.
    if (!canSend) return;
    sending = true;
    errors = {};
    succeeded = {};
    await Promise.allSettled(
      sendable.map(async (t) => {
        const r = await sendPrompt(t.host_alias, t.tmux_name, prompt);
        if (r.ok) succeeded[t.id] = true;
        else errors[t.id] = r.error.message;
      }),
    );
    sending = false;
    if (Object.keys(errors).length === 0) setTimeout(() => onClose(), 600);
  }
</script>

<Modal label="Send prompt to selected sessions" onclose={onClose} width="520px" testid="bulk-prompt-dialog">
  <div class="dialog">
    <h3>Send prompt to {sendable.length} session{sendable.length === 1 ? '' : 's'}</h3>
    <ul class="targets">
      {#each targets as t (t.id)}
        {@const skipped = !sendable.includes(t)}
        {@const notYours = !writable.includes(t)}
        <li class:skipped data-testid="bulk-target-{t.id}">
          <span class="host-badge">[{t.host_alias}]</span>
          <span class="sess-name">{t.friendly_name ?? t.tmux_name}</span>
          {#if notYours}
            <span class="muted" data-testid="bulk-not-mine-{t.id}" title={$sessionBlocked(t, 'send_prompt') ?? ''}
              >not yours</span
            >
          {:else if skipped}
            <span class="muted" title="shell or non-running sessions are skipped">skipped</span>
          {:else if succeeded[t.id]}
            <span class="ok">✓</span>
          {:else if errors[t.id]}
            <span class="err" data-testid="bulk-err-{t.id}">✗ {errors[t.id]}</span>
          {/if}
        </li>
      {/each}
    </ul>
    <textarea
      bind:value={prompt}
      rows="6"
      placeholder="Prompt to send to every selected session…"
      data-testid="bulk-prompt-textarea"
    ></textarea>
    {#if hubBlocked}
      <p class="muted" data-testid="bulk-prompt-blocked">{hubBlocked}</p>
    {:else if notMine > 0}
      <p class="muted" data-testid="bulk-prompt-not-mine">
        {notMine} of the selected sessions {notMine === 1 ? 'is' : 'are'} not yours to drive and
        {notMine === 1 ? 'is' : 'are'} left out.
      </p>
    {/if}
    <div class="actions">
      <button onclick={onClose}>Cancel</button>
      <button
        class="primary"
        disabled={!canSend}
        title={hubBlocked ?? ''}
        onclick={send}
        data-testid="bulk-prompt-send"
      >
        {sending ? 'Sending…' : 'Send →'}
      </button>
    </div>
  </div>
</Modal>

<style>
  .dialog { display: flex; flex-direction: column; gap: 0.7rem; }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .targets {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    max-height: 12rem;
    overflow: auto;
  }
  .targets li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
    padding: 0.2rem 0.3rem;
  }
  .targets li.skipped { opacity: 0.55; }
  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
  }
  .sess-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .muted { color: var(--fg-muted); font-size: 0.75rem; }
  .ok { color: var(--status-done); }
  .err { color: var(--danger); font-size: 0.75rem; }
  textarea {
    width: 100%;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    resize: vertical;
    min-height: 5rem;
  }
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
