<script lang="ts">
  // Settings → Control API: every token that reaches this fleet's control
  // API in one table (Orbit Fleet 11.4) — each host's own token, with when
  // it was last used and last rotated, beside the paired devices' tokens.
  // A host's token is rotated here (`rotate_host_token`, which re-provisions
  // the host); a device's is revoked (`revoke_device`, as Settings → Devices
  // does). The master token stays in the section above.
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { hostTokens, loadHostTokens, rotateToken } from './host_actions';
  import { rotateTokenMessage } from './hosts_view';
  import { devices, loadDevices, type DeviceSummary } from './devices';
  import { invokeCmd } from './result';
  import { push, pushError } from './toasts';
  import { ago } from './pages/resources';

  let {
    active = true,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    /** Read the tokens once this is true: Settings keeps every panel
     *  mounted, and a hidden one asks nothing. */
    active?: boolean;
    now?: () => number;
  } = $props();

  type Row =
    | { kind: 'host'; key: string; name: string; mode: string; used?: number; created: number; rotated?: number }
    | { kind: 'device'; key: string; name: string; mode: string; used?: number; created: number; self: boolean };

  const rows = $derived<Row[]>([
    ...[...$hostTokens.values()].map(
      (t): Row => ({
        kind: 'host',
        key: `host:${t.host_alias}`,
        name: t.host_alias,
        mode: t.mode,
        used: t.last_used_at ?? undefined,
        created: t.created_at,
        rotated: t.rotated_at ?? undefined,
      }),
    ),
    ...$devices.map(
      (d: DeviceSummary): Row => ({
        kind: 'device',
        key: `device:${d.name}`,
        name: d.name,
        mode: d.mode,
        used: d.last_seen_at,
        created: d.created_at,
        self: d.this_device === true,
      }),
    ),
  ]);

  let asking = $state<Row | null>(null);
  let busy = $state(false);

  let loaded = false;
  $effect(() => {
    if (active && !loaded) {
      loaded = true;
      void loadHostTokens();
      void loadDevices();
    }
  });

  async function go(row: Row) {
    busy = true;
    if (row.kind === 'host') {
      const r = await rotateToken(row.name);
      if (r.ok) {
        push({ kind: 'success', message: `${row.name} has a new control-API token.` });
        if (r.value.warning) push({ kind: 'warning', message: `${row.name}: ${r.value.warning}`, sticky: true });
      } else pushError(r.error, `Rotate token for ${row.name} failed`);
    } else {
      const r = await invokeCmd<unknown>('revoke_device', { args: { device: row.name } });
      if (r.ok) push({ kind: 'success', message: `${row.name} can no longer reach the fleet.` });
      else pushError(r.error, `Revoke ${row.name} failed`);
      await loadDevices();
    }
    busy = false;
    asking = null;
  }
</script>

<section class="block" data-testid="control-api-tokens">
  <div class="section-header"><h4>Control API tokens</h4></div>
  {#if rows.length === 0}
    <p class="hook-desc" data-testid="tokens-empty">No host or device has a token yet. Provision a host or pair a device.</p>
  {:else}
    <table class="tokens">
      <thead>
        <tr><th>Token</th><th>Scope</th><th>Used</th><th>Created</th><th><span class="sr">Action</span></th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.key)}
          <tr data-testid={`token-row-${row.key}`}>
            <td class="name">
              {row.name}
              <span class="kind">· {row.kind === 'host' ? 'per-host' : 'device'}</span>
            </td>
            <td>{row.mode === 'readonly' ? 'read-only' : row.mode}</td>
            <td data-testid={`token-used-${row.key}`}>{ago(row.used, now())}</td>
            <td data-testid={`token-created-${row.key}`}>
              {ago(row.created, now())}
              {#if row.kind === 'host' && row.rotated}
                <span class="rotated" data-testid={`token-rotated-${row.key}`}>rotated {ago(row.rotated, now())}</span>
              {/if}
            </td>
            <td class="act">
              {#if row.kind === 'host'}
                <button type="button" class="hook-btn" disabled={busy} data-testid={`token-rotate-${row.name}`} onclick={() => (asking = row)}
                  >Rotate</button
                >
              {:else if !row.self}
                <button type="button" class="hook-btn" disabled={busy} data-testid={`token-revoke-${row.name}`} onclick={() => (asking = row)}
                  >Revoke</button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <p class="hook-desc">
      A host's token lives in its ~/.claude.json and is replaced by Rotate; a device's was shown once at pairing and is
      ended by Revoke. Used is the last request the token made, to the minute.
    </p>
  {/if}
</section>

{#if asking}
  <ConfirmDialog
    title={asking.kind === 'host' ? `Rotate the token for ${asking.name}` : `Revoke ${asking.name}`}
    message={asking.kind === 'host'
      ? rotateTokenMessage(asking.name)
      : `${asking.name} can no longer reach the fleet. Pair it again to bring it back.`}
    confirmLabel={asking.kind === 'host' ? 'Rotate' : 'Revoke'}
    danger={asking.kind === 'device'}
    {busy}
    confirmTestId="token-confirm"
    onconfirm={() => asking && void go(asking)}
    oncancel={() => (asking = null)} />
{/if}

<style>
  .tokens {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8rem;
  }
  .tokens th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: 0.72rem;
    padding: 0.25rem 0.5rem 0.25rem 0;
    border-bottom: 1px solid var(--border);
  }
  .tokens td {
    padding: 0.35rem 0.5rem 0.35rem 0;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .name {
    font-weight: 500;
  }
  .kind,
  .rotated {
    color: var(--fg-muted);
    font-weight: 400;
  }
  .rotated {
    display: block;
    font-size: 0.72rem;
  }
  .act {
    text-align: right;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
