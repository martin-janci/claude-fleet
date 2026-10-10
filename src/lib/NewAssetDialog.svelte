<script lang="ts">
  import Loader from './Loader.svelte';
  import Modal from './Modal.svelte';
  import { catalog, catalogOf, createAsset, KIND_ORDER, KIND_ONE, type AssetKind } from './assets';

  let {
    onclose,
    onsaved,
    onwrite,
  }: {
    onclose: () => void;
    /** Fires after a successful create; the caller selects the asset and
     *  opens the editor. */
    onsaved: (kind: AssetKind, name: string) => void;
    /** "Write it with Claude…" (G2.6): hand the kind and name to a session
     *  instead, seeded with these instructions. Absent where no session can
     *  start (a hub client). */
    onwrite?: (instructions: string) => void;
  } = $props();

  let kind = $state<AssetKind>('skill');
  let name = $state('');
  let duplicateFrom = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;
  const nameError = $derived(name !== '' && !NAME_RE.test(name) ? 'Use lower case letters, digits and dashes.' : null);
  const canCreate = $derived(name.trim() !== '' && NAME_RE.test(name) && !busy);

  // Assets M5 (PF9): the listing spans every catalog; a new asset is
  // created in the personal catalog, so it copies from personal's only (one
  // name per kind there — the option keys stay unique).
  const candidates = $derived(($catalog?.assets ?? []).filter((a) => a.kind === kind && catalogOf(a) === 'personal'));

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
  <p class="intro">Skills, agents, commands, hooks, MCP servers and plugins live in one catalog.</p>
  <label>Kind
    <select bind:value={kind} disabled={busy} data-testid="new-asset-kind">
      {#each KIND_ORDER as k (k)}<option value={k}>{KIND_ONE[k]}</option>{/each}
    </select>
  </label>
  <label>Name
    <input bind:value={name} disabled={busy} placeholder={kind === 'command' ? 'ship-it' : 'my-skill'} data-testid="new-asset-name" />
    <span class="help">Lower case and dashes; it becomes the folder name.</span>
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
    {#if onwrite}
      <button
        class="write"
        onclick={() => onwrite?.(`Create a new ${KIND_ONE[kind].toLowerCase()}${canCreate ? ` named "${name}"` : ''} that …`)}
        disabled={busy}
        data-testid="new-asset-write">Write it with Claude…</button
      >
    {/if}
    <button onclick={onclose} disabled={busy}>Cancel</button>
    <button class="primary" onclick={create} disabled={!canCreate} data-testid="new-asset-create">{#if busy}<Loader name="comet" size={12} class="btn-loader" />{/if}{busy ? 'Creating…' : 'Create'}</button>
  </div>
</Modal>

<style>
  label { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-xs); color: var(--fg-muted); margin-bottom: 8px; }
  input, select { font: inherit; padding: 4px 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: var(--radius-sm); }
  .intro { margin: 0 0 8px; font-size: var(--text-xs); color: var(--fg-muted); }
  .help { font-size: var(--text-2xs); color: var(--fg-muted); }
  .actions .write { margin-right: auto; }
  .err { color: var(--usage-crit); font-size: var(--text-xs); margin: -4px 0 8px; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 8px; }
  .actions button { font-size: var(--text-xs); padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
