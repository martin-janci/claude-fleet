<script lang="ts">
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import { listSecrets, setSecret, deleteSecret, type SecretRow } from './assets';
  import { hosts } from './hosts';

  // `names` is the union of names collected from the last computed sync
  // plan's actions (`secrets` + `missing_secrets`) — stored names come from
  // `listSecrets` below, and the free-text input adds a name not yet known
  // to either. Values are write-only: this panel never displays one, only
  // whether a name is "set" globally / per host.
  let { names, onclose }: { names: string[]; onclose: () => void } = $props();

  let rows = $state<SecretRow[]>([]);
  let error = $state<string | null>(null);
  let extraNames = $state<string[]>([]);
  let newName = $state('');
  let valueDrafts = $state<Record<string, string>>({});
  let hostDrafts = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);

  async function reload() {
    const r = await listSecrets();
    if (r.ok) rows = r.value;
    else error = r.error.message;
  }

  onMount(reload);

  const visibleNames = $derived(
    Array.from(new Set([...names, ...rows.map((r) => r.name), ...extraNames])).sort(),
  );
  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));

  function draftValue(name: string): string {
    return valueDrafts[name] ?? '';
  }
  function draftHost(name: string): string {
    return hostDrafts[name] ?? '';
  }
  function onValueInput(name: string, e: Event) {
    valueDrafts = { ...valueDrafts, [name]: (e.currentTarget as HTMLInputElement).value };
  }
  function onHostChange(name: string, e: Event) {
    hostDrafts = { ...hostDrafts, [name]: (e.currentTarget as HTMLSelectElement).value };
  }

  function globalStatus(name: string): 'set' | 'not set' {
    return rows.some((r) => r.name === name && r.host_alias === null) ? 'set' : 'not set';
  }
  function hostStatus(name: string, hostAlias: string): 'set' | 'not set' {
    return rows.some((r) => r.name === name && r.host_alias === hostAlias) ? 'set' : 'not set';
  }

  function addName() {
    const n = newName.trim().toUpperCase();
    if (n === '') return;
    if (!visibleNames.includes(n)) extraNames = [...extraNames, n];
    newName = '';
  }

  async function set(name: string) {
    const value = draftValue(name);
    if (value === '') return;
    const hostAlias = draftHost(name);
    busy = name;
    error = null;
    const r = await setSecret(name, value, hostAlias === '' ? undefined : hostAlias);
    busy = null;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    valueDrafts = { ...valueDrafts, [name]: '' };
    await reload();
  }

  // ── Add a secret (G2.6): one value, written to several hosts at once ──
  // The value lives only in this input until Add writes it; it is cleared
  // after, and never kept anywhere else in the app.
  let secretName = $state('');
  let addValue = $state('');
  let addGlobal = $state(true);
  let addHosts = $state<Record<string, 'write' | 'skip'>>({});
  let adding = $state(false);
  const NAME_OK = /^[A-Z0-9_]+$/;
  const addNameNorm = $derived(secretName.trim().toUpperCase());
  const writeHosts = $derived(visibleHosts.filter((h) => addHosts[h.alias] === 'write').map((h) => h.alias));
  const canAdd = $derived(NAME_OK.test(addNameNorm) && addValue !== '' && (addGlobal || writeHosts.length > 0) && !adding);

  async function addSecret() {
    if (!canAdd) return;
    adding = true;
    error = null;
    const name = addNameNorm;
    const value = addValue;
    const targets: (string | undefined)[] = [...(addGlobal ? [undefined] : []), ...writeHosts];
    const failed: string[] = [];
    for (const t of targets) {
      const r = await setSecret(name, value, t);
      if (!r.ok) failed.push(`${t ?? 'every host'}: ${r.error.message}`);
    }
    adding = false;
    addValue = '';
    if (failed.length > 0) error = `${name} was not written to ${failed.join('; ')}`;
    else secretName = '';
    if (!visibleNames.includes(name)) extraNames = [...extraNames, name];
    await reload();
  }

  async function del(name: string) {
    const hostAlias = draftHost(name);
    busy = name;
    error = null;
    const r = await deleteSecret(name, hostAlias === '' ? undefined : hostAlias);
    busy = null;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    await reload();
  }
</script>

<Modal title="Secrets" onclose={onclose} width="580px" testid="secrets-panel">
  <p class="muted">Values are write-only: fleet never reads or displays a secret once it is set.</p>
  {#if error}<p class="error" data-testid="secrets-error">{error}</p>{/if}
  <form class="add-secret" data-testid="secrets-add" onsubmit={(e) => { e.preventDefault(); void addSecret(); }}>
    <p class="muted">Assets read it as ${'{'}NAME{'}'}. Its value goes to hosts and is never shown again.</p>
    <div class="add-row">
      <input placeholder="NAME" bind:value={secretName} aria-label="Name" autocomplete="off" data-testid="secrets-add-secret-name" />
      <input type="password" placeholder="value" bind:value={addValue} aria-label="Value" autocomplete="off" data-testid="secrets-add-secret-value" />
    </div>
    <p class="muted small">Write-only. To change it, paste a new value.</p>
    <div class="add-hosts" role="group" aria-label="Hosts">
      <label class="host"><input type="checkbox" bind:checked={addGlobal} data-testid="secrets-add-global" /> Every host</label>
      {#each visibleHosts as h (h.alias)}
        <label class="host">
          <span>{h.alias}</span>
          <select
            value={addHosts[h.alias] ?? 'skip'}
            onchange={(e) => (addHosts = { ...addHosts, [h.alias]: (e.currentTarget as HTMLSelectElement).value as 'write' | 'skip' })}
            data-testid={`secrets-add-host-${h.alias}`}
          >
            <option value="write">Write</option>
            <option value="skip">Skip</option>
          </select>
        </label>
      {/each}
    </div>
    <button type="submit" disabled={!canAdd} data-testid="secrets-add-secret">{adding ? 'Adding…' : 'Add secret'}</button>
  </form>
  <div class="add-name">
    <input placeholder="NAME" bind:value={newName} data-testid="secrets-add-name" />
    <button onclick={addName} data-testid="secrets-add-name-submit">Add</button>
  </div>
  <div class="rows">
    {#each visibleNames as name (name)}
      <div class="secret-row" data-testid={`secret-row-${name}`}>
        <div class="head">
          <span class="name">{name}</span>
          <span class="status">global: {globalStatus(name)}</span>
        </div>
        <div class="controls">
          <input
            type="password"
            placeholder="value"
            aria-label={`Value of ${name}`}
            value={draftValue(name)}
            oninput={(e) => onValueInput(name, e)}
            data-testid={`secret-value-${name}`}
          />
          <select aria-label={`Where ${name} applies`} value={draftHost(name)} onchange={(e) => onHostChange(name, e)} data-testid={`secret-host-${name}`}>
            <option value="">global</option>
            {#each visibleHosts as h (h.alias)}
              <option value={h.alias}>{h.alias} ({hostStatus(name, h.alias)})</option>
            {/each}
          </select>
          <button onclick={() => set(name)} disabled={busy === name || draftValue(name) === ''} data-testid={`secret-set-${name}`}>Set</button>
          <button onclick={() => del(name)} disabled={busy === name} data-testid={`secret-delete-${name}`}>Delete</button>
        </div>
      </div>
    {/each}
    {#if visibleNames.length === 0}<p class="muted">No secrets known yet.</p>{/if}
  </div>
  <div class="actions">
    <button onclick={onclose}>Close</button>
  </div>
</Modal>

<style>
  .muted { color: var(--fg-muted); font-size: var(--text-xs); margin: 0 0 6px; }
  .error { color: var(--usage-crit); }
  .add-name { display: flex; gap: 6px; margin-bottom: 8px; }
  .add-secret { display: flex; flex-direction: column; gap: 6px; border: 1px solid var(--border); border-radius: var(--radius-md); padding: 8px; margin-bottom: 10px; }
  .add-row { display: flex; gap: 6px; }
  .add-row input { flex: 1; min-width: 0; }
  .small { font-size: var(--text-2xs); margin: 0; }
  .add-hosts { display: flex; flex-wrap: wrap; gap: 6px 14px; font-size: var(--text-xs); }
  .add-hosts .host { display: inline-flex; align-items: center; gap: 6px; }
  .add-secret button { align-self: flex-end; font-size: var(--text-xs); padding: 0.3rem 0.8rem; border: 1px solid var(--accent); background: transparent; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; }
  .add-secret button:disabled { opacity: 0.5; cursor: not-allowed; }
  .add-name input { flex: 1; }
  .rows { display: flex; flex-direction: column; gap: 8px; max-height: 50vh; overflow: auto; }
  .secret-row { border: 1px solid var(--border); border-radius: var(--radius-md); padding: 6px 8px; }
  .head { display: flex; justify-content: space-between; align-items: center; font-size: var(--text-xs); margin-bottom: 4px; }
  .name { font-family: var(--font-mono); }
  .status { color: var(--fg-muted); }
  .controls { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .controls input { flex: 1; min-width: 100px; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; }
  .actions button, .controls button { font-size: var(--text-xs); padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; }
  .actions button:disabled, .controls button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
