<script lang="ts">
  import { onMount } from 'svelte';
  import { hosts, probeHost, deleteHost, hideHost } from './hosts';
  import { accounts, type AccountRow } from './accounts';
  import { mcpStatus, mcpConfigure, mcpClientConfig, type McpStatus } from './mcp';
  import AddHostPicker from './AddHostPicker.svelte';

  let { onClose }: { onClose: () => void } = $props();

  let showAddPicker = $state(false);
  let busy: string | null = $state(null);
  let error: string | null = $state(null);

  // --- Control API (MCP) ---
  let mcp: McpStatus | null = $state(null);
  let mcpBusy = $state(false);
  let mcpError: string | null = $state(null);
  let tokenShown = $state(false);
  let portInput = $state(4180);

  const configBlock = $derived(mcp ? mcpClientConfig(mcp) : '');

  onMount(async () => {
    const r = await mcpStatus();
    if (r.ok && r.value) {
      mcp = r.value;
      portInput = r.value.port;
    } else if (!r.ok) {
      mcpError = r.error.message;
    }
  });

  async function applyMcp(opts: {
    enabled: boolean;
    port?: number;
    regenerateToken?: boolean;
  }) {
    mcpBusy = true;
    mcpError = null;
    const r = await mcpConfigure(opts);
    mcpBusy = false;
    if (r.ok && r.value) {
      mcp = r.value;
      portInput = r.value.port;
    } else if (!r.ok) {
      mcpError = r.error.message;
    }
  }

  function maskToken(t: string): string {
    return t.length > 4 ? '••••••••••••' + t.slice(-4) : '••••';
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* clipboard unavailable — no-op */
    }
  }

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

    <section class="block" data-testid="mcp-section">
      <div class="section-header">
        <h4>Control API (MCP)</h4>
      </div>
      <p class="mcp-blurb">
        Lets an AI assistant drive claude-fleet over a localhost-only MCP
        server. Off by default. Every request needs the bearer token.
      </p>
      {#if mcp}
        <div class="mcp-row">
          <label class="toggle">
            <input
              type="checkbox"
              checked={mcp.enabled}
              disabled={mcpBusy}
              onchange={() => applyMcp({ enabled: !mcp!.enabled, port: portInput })}
              data-testid="mcp-enable" />
            Enable control API
          </label>
          <span class="status status-{mcp.running ? 'on' : 'off'}">
            {mcp.running ? 'running' : 'stopped'}
          </span>
        </div>
        {#if mcp.bind_error}
          <p class="err">Server could not start: {mcp.bind_error}</p>
        {/if}

        <div class="mcp-field">
          <span class="lbl">Port</span>
          <input
            class="port"
            type="number"
            min="1"
            max="65535"
            bind:value={portInput}
            disabled={mcpBusy} />
          <button
            disabled={mcpBusy || portInput === mcp.port}
            onclick={() => applyMcp({ enabled: mcp!.enabled, port: portInput })}>
            Apply
          </button>
        </div>

        <div class="mcp-field">
          <span class="lbl">URL</span>
          <code class="mono">{mcp.url}</code>
          <button onclick={() => copyText(mcp!.url)}>Copy</button>
        </div>

        <div class="mcp-field">
          <span class="lbl">Token</span>
          <code class="mono token">{tokenShown ? mcp.token : maskToken(mcp.token)}</code>
          <button onclick={() => (tokenShown = !tokenShown)}>
            {tokenShown ? 'Hide' : 'Show'}
          </button>
          <button onclick={() => copyText(mcp!.token)}>Copy</button>
          <button
            class="danger"
            disabled={mcpBusy}
            onclick={() =>
              applyMcp({ enabled: mcp!.enabled, port: portInput, regenerateToken: true })}
            title="Mint a new token — invalidates existing clients">
            Regenerate
          </button>
        </div>

        <details class="mcp-config">
          <summary>MCP client config</summary>
          <pre>{configBlock}</pre>
          <button onclick={() => copyText(configBlock)}>Copy config</button>
        </details>
      {/if}
      {#if mcpError}<p class="err">{mcpError}</p>{/if}
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

  .mcp-blurb {
    font-size: 0.78rem;
    color: var(--fg-muted);
    margin: 0 0 0.6rem;
    line-height: 1.4;
  }
  .mcp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .mcp-field {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.4rem;
    font-size: 0.82rem;
  }
  .mcp-field .lbl {
    width: 3.2rem;
    color: var(--fg-muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .mcp-field .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg-alt, rgba(127, 127, 127, 0.12));
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-field .token {
    flex: 1;
    min-width: 0;
  }
  .mcp-field .port {
    width: 6rem;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
  }
  .mcp-field button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .mcp-field button:hover:not(:disabled) { border-color: var(--accent); }
  .mcp-field button:disabled { opacity: 0.5; cursor: default; }
  .mcp-field button.danger:hover:not(:disabled) {
    color: #e64a4a;
    border-color: #e64a4a;
  }
  .mcp-config { font-size: 0.8rem; margin-top: 0.3rem; }
  .mcp-config summary { cursor: pointer; color: var(--fg-muted); }
  .mcp-config pre {
    background: var(--bg-alt, rgba(127, 127, 127, 0.12));
    padding: 0.5rem;
    border-radius: 4px;
    overflow: auto;
    font-size: 0.75rem;
    margin: 0.4rem 0;
  }
  .mcp-config button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .mcp-config button:hover { border-color: var(--accent); }
</style>
