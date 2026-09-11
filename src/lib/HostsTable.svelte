<script lang="ts">
  import { hosts, probeHost, deleteHost, hideHost } from './hosts';
  import { accounts, type AccountRow } from './accounts';
  import {
    listHostTokens,
    setHostTokenMode,
    rotateHostToken,
    type HostTokenInfo,
    type TokenMode,
  } from './mcp';
  import { sessions } from './sessions';
  import { hookHealth, hookHealthLabel } from './hook_health';

  let { onAddHost }: { onAddHost: () => void } = $props();

  let busy: string | null = $state(null);
  let error: string | null = $state(null);

  // --- Per-host control-API tokens ---
  // alias -> token info; a host absent here has never been provisioned.
  let hostTokens = $state<Map<string, HostTokenInfo>>(new Map());
  let tokenBusy: string | null = $state(null);
  let tokenError: string | null = $state(null);

  // Coarse clock for the hooks column's "last event Ns ago".
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 15_000);
    return () => clearInterval(t);
  });

  export async function loadHostTokens() {
    const r = await listHostTokens();
    if (r.ok && Array.isArray(r.value)) {
      hostTokens = new Map(r.value.map((t) => [t.host_alias, t]));
    }
  }

  async function onTokenMode(alias: string, mode: TokenMode) {
    tokenBusy = alias;
    tokenError = null;
    const r = await setHostTokenMode(alias, mode);
    tokenBusy = null;
    if (r.ok && r.value) {
      hostTokens = new Map(hostTokens).set(alias, r.value);
    } else if (!r.ok) {
      tokenError = r.error.message;
    }
  }

  async function onRotateToken(alias: string) {
    tokenBusy = alias;
    tokenError = null;
    const r = await rotateHostToken(alias);
    tokenBusy = null;
    if (r.ok && r.value) {
      hostTokens = new Map(hostTokens).set(alias, r.value);
    } else if (!r.ok) {
      tokenError = r.error.message;
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

<section class="block">
  <div class="section-header">
    <h4>Hosts</h4>
    <button class="add" onclick={() => onAddHost()} data-testid="settings-add-host">
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
        <th title="Control-API token: full = every tool, readonly = observe only">Token</th>
        <th title="Fleet hooks: installed with the host's token; last event = newest Stop hook from a session on this host">Hooks</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#each $hosts as h (h.alias)}
        {@const hh = hookHealth(h.alias, hostTokens.has(h.alias), $sessions)}
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
          <td class="token-cell" data-testid="token-cell">
            {#if hostTokens.get(h.alias)}
              <select
                class="mode"
                value={hostTokens.get(h.alias)!.mode}
                disabled={tokenBusy === h.alias}
                onchange={(e) => onTokenMode(h.alias, (e.currentTarget as HTMLSelectElement).value as TokenMode)}
                aria-label="Token mode"
              >
                <option value="full">full</option>
                <option value="readonly">readonly</option>
              </select>
              <button
                disabled={tokenBusy === h.alias}
                onclick={() => onRotateToken(h.alias)}
                title="Mint a new token and re-provision this host"
                aria-label="Rotate token">Rotate</button>
            {:else}
              <span class="muted" title="Provision hosts to mint one">none</span>
            {/if}
          </td>
          <td data-testid="hooks-cell" data-state={hh.state}>
            <span class:muted={hh.state !== 'seen'}>{hookHealthLabel(hh, nowSec)}</span>
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
  {#if tokenError}<p class="err">{tokenError}</p>{/if}
</section>

<style>
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
  .muted { color: var(--fg-muted); }

  .hosts-table td.account {
    font-size: 0.8rem;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }

  .token-cell { display: flex; gap: 0.3rem; align-items: center; white-space: nowrap; }
  .token-cell select.mode {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    font-size: 0.75rem;
    padding: 0.1rem 0.2rem;
  }
  .token-cell button {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0.1rem 0.4rem;
    font-size: 0.75rem;
    border-radius: 4px;
  }
  .token-cell button:hover:not(:disabled) { border-color: var(--border); color: var(--fg); }
  .token-cell button:disabled { opacity: 0.5; cursor: default; }

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
