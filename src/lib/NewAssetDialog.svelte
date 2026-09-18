<script lang="ts">
  import Modal from './Modal.svelte';
  import { catalog, createAsset, KIND_ORDER, KIND_LABEL, type AssetKind } from './assets';

  let {
    onclose,
    onsaved,
  }: {
    onclose: () => void;
    /** Fires after a successful create; the caller selects the asset and
     *  opens the editor. */
    onsaved: (kind: AssetKind, name: string) => void;
  } = $props();

  let kind = $state<AssetKind>('skill');
  let name = $state('');
  let duplicateFrom = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;
  const nameError = $derived(name !== '' && !NAME_RE.test(name) ? 'must match [a-z0-9][a-z0-9-]*' : null);
  const canCreate = $derived(name.trim() !== '' && NAME_RE.test(name) && !busy);

  const candidates = $derived(($catalog?.assets ?? []).filter((a) => a.kind === kind));

  // A kind switch invalidates any previously chosen duplicate-from (it named
  // an asset of the old kind).
  $effect(() => {
    void kind;
    duplicateFrom = '';
  });

  async function create() {
    if (!canCreate) return;
    busy = true;
    error = null;
    const r = await createAsset(kind, name, duplicateFrom === '' ? undefined : duplicateFrom);
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onsaved(kind, name);
  }
</script>

<Modal title="New asset" onclose={busy ? undefined : onclose} width="420px" testid="new-asset-dialog">
  <label>Kind
    <select bind:value={kind} disabled={busy} data-testid="new-asset-kind">
      {#each KIND_ORDER as k (k)}<option value={k}>{KIND_LABEL[k]}</option>{/each}
    </select>
  </label>
  <label>Name
    <input bind:value={name} disabled={busy} placeholder="my-skill" data-testid="new-asset-name" />
  </label>
  {#if nameError}<p class="err" data-testid="new-asset-name-error">{nameError}</p>{/if}
  <label>Duplicate from (optional)
    <select bind:value={duplicateFrom} disabled={busy} data-testid="new-asset-duplicate-from">
      <option value="">(none — start from the template)</option>
      {#each candidates as a (a.name)}<option value={a.name}>{a.name}</option>{/each}
    </select>
  </label>
  {#if error}<p class="err" data-testid="new-asset-error">{error}</p>{/if}
  <div class="actions">
    <button onclick={onclose} disabled={busy}>Cancel</button>
    <button class="primary" onclick={create} disabled={!canCreate} data-testid="new-asset-create">{busy ? 'Creating…' : 'Create'}</button>
  </div>
</Modal>

<style>
  label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--fg-muted); margin-bottom: 8px; }
  input, select { font: inherit; padding: 4px 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: 4px; }
  .err { color: #dc2626; font-size: 12px; margin: -4px 0 8px; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 8px; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
