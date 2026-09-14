<script lang="ts">
  // Bulk "send prompt" for the sidebar's multi-select (FE-4). Same fan-out as
  // PromptComposer's send loop (one `send_prompt` per target, per-target
  // error/success), but the target list is fixed by the selection instead of
  // derived from a source session.
  import { sendPrompt, type SessionRow } from './sessions';
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

  const sendable = $derived(targets.filter((t) => t.kind !== 'shell' && t.status === 'running'));
  const canSend = $derived(prompt.trim().length > 0 && sendable.length > 0 && !sending);

  async function send() {
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
        <li class:skipped data-testid="bulk-target-{t.id}">
          <span class="host-badge">[{t.host_alias}]</span>
          <span class="sess-name">{t.friendly_name ?? t.tmux_name}</span>
          {#if skipped}
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
    <div class="actions">
      <button onclick={onClose}>Cancel</button>
      <button class="primary" disabled={!canSend} onclick={send} data-testid="bulk-prompt-send">
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
  .ok { color: rgb(80, 200, 110); }
  .err { color: #e64a4a; font-size: 0.75rem; }
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
