<script lang="ts">
  import { newBgSession } from './sessions';
  import { hosts } from './hosts';
  import Modal from './Modal.svelte';

  // The draft lives in the Sidebar and is bound here, so it survives closing
  // and reopening the dialog as before.
  let {
    bgModalHost = $bindable(),
    bgModalName = $bindable(),
    bgModalPrompt = $bindable(),
    bgModalError = $bindable(),
    bgModalLoading = $bindable(),
    onClose,
  }: {
    bgModalHost: string;
    bgModalName: string;
    bgModalPrompt: string;
    bgModalError: string | null;
    bgModalLoading: boolean;
    onClose: () => void;
  } = $props();

  async function doNewBgSession() {
    bgModalError = null;
    bgModalLoading = true;
    try {
      const result = await newBgSession(bgModalHost, bgModalName, bgModalPrompt);
      if (result.ok) {
        onClose();
        bgModalName = '';
        bgModalPrompt = '';
      } else {
        bgModalError = result.error.message;
      }
    } catch (e: unknown) {
      bgModalError = e instanceof Error ? e.message : String(e);
    } finally {
      bgModalLoading = false;
    }
  }
</script>

<Modal title="New Background Session" onclose={() => onClose()} width="420px" testid="bg-session-modal">
  <div class="modal">
    <label class="modal-field">
      <span>Host</span>
      <select bind:value={bgModalHost}>
        {#each $hosts as host (host.alias)}
          <option value={host.alias}>{host.alias}</option>
        {/each}
      </select>
    </label>
    <label class="modal-field">
      <span>Session name</span>
      <input
        type="text"
        bind:value={bgModalName}
        placeholder="e.g. fix-auth-bug"
        data-testid="bg-session-name"
      />
    </label>
    <label class="modal-field">
      <span>Initial prompt</span>
      <textarea
        bind:value={bgModalPrompt}
        rows="4"
        placeholder="What should Claude work on?"
        data-testid="bg-session-prompt"
      ></textarea>
    </label>
    {#if bgModalError}
      <p class="err">{bgModalError}</p>
    {/if}
    <div class="modal-actions">
      <button onclick={() => onClose()}>Cancel</button>
      <button
        class="btn-primary"
        onclick={doNewBgSession}
        disabled={bgModalLoading || !bgModalName.trim() || !bgModalPrompt.trim()}
        data-testid="bg-session-submit"
      >
        {bgModalLoading ? 'Launching…' : 'Launch'}
      </button>
    </div>
  </div>
</Modal>

<style>
  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }

  .modal {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .modal-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }
  .modal-field input,
  .modal-field select,
  .modal-field textarea {
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
    color: var(--fg);
    font-family: inherit;
    font-size: 12px;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .modal-actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .btn-primary {
    color: var(--accent) !important;
    border-color: var(--accent) !important;
  }
  .btn-primary:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 14%, transparent) !important; }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
