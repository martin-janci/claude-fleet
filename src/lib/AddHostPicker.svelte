<script lang="ts">
  import { onMount } from 'svelte';
  import { discoverHosts, addHost, type SshHost } from './hosts';

  let { onClose }: { onClose: () => void } = $props();

  let available = $state<SshHost[]>([]);
  let loading = $state(true);
  let error: string | null = $state(null);
  let probing: string | null = $state(null);

  onMount(async () => {
    const r = await discoverHosts();
    loading = false;
    if (r.ok) {
      available = r.value;
    } else {
      error = r.error.message;
    }
  });

  async function pick(host: SshHost) {
    probing = host.alias;
    error = null;
    // alias = ssh_alias for now; future iter will allow a custom local alias.
    const r = await addHost(host.alias, host.alias);
    probing = null;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onClose();
  }

  function describe(h: SshHost): string {
    const parts: string[] = [];
    if (h.hostname) parts.push(h.hostname);
    if (h.user) parts.push(`user=${h.user}`);
    if (h.port) parts.push(`port=${h.port}`);
    return parts.join(' · ');
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Add SSH host">
    <h3>Add SSH host</h3>
    {#if loading}
      <p class="muted">Scanning ~/.ssh/config…</p>
    {:else if available.length === 0}
      <p class="muted">No hosts found in ~/.ssh/config. Add one there first.</p>
    {:else}
      <ul class="hosts-list">
        {#each available as h (h.alias)}
          <li>
            <button
              class="host-row"
              data-testid="picker-row"
              disabled={probing !== null}
              onclick={() => pick(h)}
            >
              <span class="alias">{h.alias}</span>
              {#if describe(h)}
                <span class="desc">{describe(h)}</span>
              {/if}
              {#if probing === h.alias}
                <span class="status">probing…</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if error}
      <p class="err">{error}</p>
    {/if}
    <div class="actions">
      <button onclick={onClose}>Close</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 20;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 480px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .muted { color: var(--fg-muted); font-size: 0.85rem; }

  .hosts-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .host-row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0.45rem 0.6rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .host-row:hover:not(:disabled) { border-color: var(--accent); background: var(--bg-pane); }
  .host-row:disabled { opacity: 0.6; cursor: progress; }
  .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-weight: 600; }
  .desc { color: var(--fg-muted); font-size: 0.8rem; flex: 1; }
  .status { font-size: 0.75rem; color: var(--accent); }

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
</style>
