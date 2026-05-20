<script lang="ts">
  import { onMount } from 'svelte';
  import { discoverHosts, addHost, type SshHost } from './hosts';
  import { probeSshAlias, type ProbePreview } from './accounts';

  let { onClose }: { onClose: () => void } = $props();

  let available = $state<SshHost[]>([]);
  let loading = $state(true);
  let error: string | null = $state(null);
  // Preview state: the host the user clicked + the probe result so far.
  // null = no row clicked yet; { host, preview: null } = probing; { host, preview: ProbePreview } = ready to confirm.
  let previewing = $state<{ host: SshHost; preview: ProbePreview | null } | null>(null);
  let adding = $state(false);

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
    previewing = { host, preview: null };
    error = null;
    const r = await probeSshAlias(host.alias);
    if (!previewing || previewing.host.alias !== host.alias) {
      // User clicked Cancel during the probe — discard
      return;
    }
    if (!r.ok) {
      error = r.error.message;
      previewing = null;
      return;
    }
    previewing = { host, preview: r.value };
  }

  async function confirmAdd() {
    if (!previewing?.preview) return;
    adding = true;
    error = null;
    const r = await addHost(previewing.host.alias, previewing.host.alias);
    adding = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onClose();
  }

  function cancelPreview() {
    previewing = null;
  }

  function describe(h: SshHost): string {
    const parts: string[] = [];
    if (h.hostname) parts.push(h.hostname);
    if (h.user) parts.push(`user=${h.user}`);
    if (h.port) parts.push(`port=${h.port}`);
    return parts.join(' · ');
  }

  function accountLine(p: ProbePreview): string {
    if (!p.account) return '— (claude not logged in)';
    const email = p.account.email ?? p.account.uuid ?? 'unknown';
    return p.account.seat_tier ? `${email} (${p.account.seat_tier})` : email;
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Add SSH host">
    {#if !previewing}
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
                onclick={() => pick(h)}
              >
                <span class="alias">{h.alias}</span>
                {#if describe(h)}
                  <span class="desc">{describe(h)}</span>
                {/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {#if error}<p class="err">{error}</p>{/if}
      <div class="actions">
        <button onclick={onClose}>Close</button>
      </div>
    {:else}
      <h3>Add host: {previewing.host.alias}</h3>
      {#if !previewing.preview}
        <p class="muted" data-testid="preview-probing">Probing…</p>
      {:else}
        <dl class="preview" data-testid="preview-result">
          <dt>Hostname</dt><dd>{previewing.host.hostname ?? '—'}</dd>
          <dt>tmux</dt><dd>{previewing.preview.tmux_version ?? '— (not installed)'}</dd>
          <dt>claude</dt><dd>{previewing.preview.claude_version ?? '— (not installed)'}</dd>
          <dt>Account</dt><dd data-testid="preview-account">{accountLine(previewing.preview)}</dd>
        </dl>
      {/if}
      {#if error}<p class="err">{error}</p>{/if}
      <div class="actions">
        <button onclick={cancelPreview} disabled={adding}>Cancel</button>
        <button
          class="primary"
          disabled={!previewing.preview || adding}
          onclick={confirmAdd}
          data-testid="preview-confirm"
        >{adding ? 'Adding…' : 'Add'}</button>
      </div>
    {/if}
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
  .preview {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0;
  }
  .preview dt {
    color: var(--fg-muted);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .preview dd { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.9rem; }
  .actions button.primary {
    border-color: var(--accent);
    color: var(--fg);
  }
</style>
