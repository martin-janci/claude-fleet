<script lang="ts">
  import { hosts, probeHost, deleteHost, hideHost } from './hosts';
  import { accounts, type AccountRow } from './accounts';
  import AddHostPicker from './AddHostPicker.svelte';

  let { onClose }: { onClose: () => void } = $props();

  let showAddPicker = $state(false);
  let busy: string | null = $state(null);
  let error: string | null = $state(null);

  const accountByUuid = $derived(
    new Map<string, AccountRow>($accounts.map((a) => [a.uuid, a])),
  );

  function accountCell(h: { account_uuid: string | null }): string {
    if (!h.account_uuid) return '—';
    const acc = accountByUuid.get(h.account_uuid);
    if (!acc) return h.account_uuid;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `${email} (${acc.seat_tier})` : email;
  }

  async function onProbe(alias: string) {
    busy = alias;
    error = null;
    const r = await probeHost(alias);
    busy = null;
    if (!r.ok) error = r.error.message;
  }

  async function onRemove(alias: string) {
    if (alias === 'local') return;
    busy = alias;
    error = null;
    const r = await deleteHost(alias);
    busy = null;
    if (!r.ok) error = r.error.message;
  }

  async function onToggleHide(alias: string, hidden: boolean) {
    busy = alias;
    error = null;
    const r = await hideHost(alias, hidden);
    busy = null;
    if (!r.ok) error = r.error.message;
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Settings">
    <header>
      <h3>Settings</h3>
      <button class="close" onclick={onClose} aria-label="Close">×</button>
    </header>

    <section class="block">
      <div class="section-header">
        <h4>Hosts</h4>
        <button class="add" onclick={() => (showAddPicker = true)} data-testid="settings-add-host">
          + Add host
        </button>
      </div>
      <table class="hosts-table" data-testid="hosts-table">
        <thead>
          <tr>
            <th>Alias</th>
            <th>tmux</th>
            <th>claude</th>
            <th>Account</th>
            <th>Status</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each $hosts as h (h.alias)}
            <tr class:hidden-row={h.hidden}>
              <td class="alias">{h.alias}{#if h.ssh_alias && h.ssh_alias !== h.alias}<span class="muted"> ({h.ssh_alias})</span>{/if}</td>
              <td>{h.tmux_version ?? '—'}</td>
              <td>{h.claude_version ?? '—'}</td>
              <td class="account" data-testid="account-cell">{accountCell(h)}</td>
              <td>
                <span class="status status-{h.reachable ? 'on' : 'off'}">
                  {h.reachable ? 'online' : 'offline'}
                </span>
              </td>
              <td class="row-actions">
                <button
                  disabled={busy === h.alias}
                  onclick={() => onProbe(h.alias)}
                  title="Re-probe"
                  aria-label="Re-probe">↻</button>
                {#if h.alias !== 'local'}
                  <button
                    disabled={busy === h.alias}
                    onclick={() => onToggleHide(h.alias, !h.hidden)}
                    title={h.hidden ? 'Show' : 'Hide'}
                    aria-label="Toggle hide">{h.hidden ? '👁' : '🚫'}</button>
                  <button
                    class="danger"
                    disabled={busy === h.alias}
                    onclick={() => onRemove(h.alias)}
                    title="Remove host"
                    aria-label="Remove">×</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if error}<p class="err">{error}</p>{/if}
    </section>
  </div>
</div>

{#if showAddPicker}
  <AddHostPicker onClose={() => (showAddPicker = false)} />
{/if}

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 15;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 560px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  header { display: flex; align-items: center; justify-content: space-between; }
  header h3 { margin: 0; font-size: 1rem; }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.2rem;
    cursor: pointer;
    padding: 0 0.4rem;
  }
  .close:hover { color: var(--fg); }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .section-header h4 {
    margin: 0;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .add {
    font-size: 0.8rem;
    padding: 0.25rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .add:hover { border-color: var(--accent); }

  .hosts-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }
  .hosts-table th {
    text-align: left;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .hosts-table td { padding: 0.4rem; border-bottom: 1px solid var(--border); }
  .hosts-table tr.hidden-row td { opacity: 0.55; }
  .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .muted { color: var(--fg-muted); }

  .hosts-table td.account {
    font-size: 0.8rem;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }

  .status {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
  }
  .status-on { background: rgba(60,180,90,0.18); color: rgb(80,200,110); }
  .status-off { background: rgba(180,100,100,0.18); color: rgb(220,130,130); }

  .row-actions { display: flex; gap: 0.2rem; }
  .row-actions button {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0.15rem 0.45rem;
    font-size: 0.85rem;
    border-radius: 4px;
  }
  .row-actions button:hover { border-color: var(--border); color: var(--fg); }
  .row-actions button.danger:hover { color: #e64a4a; border-color: #e64a4a; }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
</style>
