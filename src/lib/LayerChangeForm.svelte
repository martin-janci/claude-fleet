<script lang="ts">
  import { untrack } from 'svelte';
  import type { LayerChange } from './assets_workspace';

  /** One layer change, inline (not a modal): create a layer, rename one, or
   *  move a member to another layer. It only builds the change; the workspace
   *  proposes it, and it becomes a card to apply (R5), never a direct commit.
   *  The name rule is the backend's `check_layer_name` (validate.rs), which
   *  is `is_valid_name`: `[a-z0-9][a-z0-9-]*`. */
  let {
    mode,
    catalogs,
    catalog,
    layer,
    member,
    layers = [],
    busy = false,
    onsubmit,
    oncancel,
  }: {
    mode: 'create' | 'rename' | 'move';
    catalogs: string[];
    catalog: string;
    layer?: string;
    member?: string;
    layers?: string[];
    busy?: boolean;
    onsubmit: (c: LayerChange) => void;
    oncancel: () => void;
  } = $props();

  const NAME = /^[a-z0-9][a-z0-9-]*$/;
  const uid = $props.id();

  let cat = $state(untrack(() => catalog));
  let name = $state('');
  let axis = $state<'context' | 'role'>('context');
  const targets = $derived(layers.filter((l) => l !== layer));
  let to = $state(untrack(() => layers.filter((l) => l !== layer)[0] ?? ''));

  const nameOk = $derived(NAME.test(name));
  const nameError = $derived(mode !== 'move' && name !== '' && !nameOk ? 'Use lowercase letters, digits and dashes.' : '');
  const valid = $derived(
    mode === 'move' ? targets.includes(to) : nameOk && (mode !== 'rename' || name !== layer),
  );

  function submit(e: Event) {
    e.preventDefault();
    if (!valid || busy) return;
    if (mode === 'create') onsubmit({ op: 'create', catalog: cat, layer: name, axis });
    else if (mode === 'rename') onsubmit({ op: 'rename', catalog: cat, layer: layer ?? '', to: name });
    else onsubmit({ op: 'move', catalog: cat, member: member ?? '', layer: layer ?? '', to });
  }
</script>

<form class="lf" data-testid="layer-form" onsubmit={submit} aria-label={mode === 'create' ? 'New layer' : mode === 'rename' ? `Rename ${layer}` : `Move ${member}`}>
  {#if mode === 'move'}
    <p class="what">Move <code>{member}</code> out of <b>{layer}</b></p>
    <label class="f">
      <span>To layer</span>
      <select bind:value={to} data-testid="layer-form-to">
        {#each targets as t (t)}<option value={t}>{t}</option>{/each}
      </select>
    </label>
    {#if targets.length === 0}<p class="err" role="status">There is no other layer to move it to.</p>{/if}
  {:else}
    {#if mode === 'create'}
      <label class="f">
        <span>Catalog</span>
        <select bind:value={cat} data-testid="layer-form-catalog">
          {#each catalogs as c (c)}<option value={c}>{c}</option>{/each}
        </select>
      </label>
    {/if}
    <label class="f">
      <span>{mode === 'create' ? 'Name' : `New name for ${layer}`}</span>
      <input
        type="text"
        bind:value={name}
        autocomplete="off"
        spellcheck="false"
        aria-invalid={nameError ? 'true' : undefined}
        aria-describedby={nameError ? `${uid}-err` : undefined}
        data-testid="layer-form-name"
      />
    </label>
    {#if nameError}<p class="err" id="{uid}-err" role="status">{nameError}</p>{/if}
    {#if mode === 'create'}
      <label class="f">
        <span>Axis</span>
        <select bind:value={axis} data-testid="layer-form-axis">
          <option value="context">context</option>
          <option value="role">role</option>
        </select>
      </label>
    {/if}
  {/if}
  <div class="act">
    <button type="submit" class="btn btn--primary" disabled={!valid || busy} data-testid="layer-form-submit">Propose</button>
    <button type="button" class="btn btn--quiet" onclick={oncancel}>Cancel</button>
  </div>
</form>

<style>
  .lf { display: grid; gap: 8px; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); }
  .f { display: grid; gap: 3px; font-size: var(--text-xs); }
  .f > span { color: var(--fg-muted); }
  .what { margin: 0; font-size: var(--text-xs); overflow-wrap: anywhere; }
  .what code { font-family: var(--mono); font-size: var(--text-2xs); }
  .err { margin: 0; font-size: var(--text-xs); color: var(--usage-crit); }
  .act { display: flex; gap: 8px; }
  input[aria-invalid='true'] { outline: 2px solid var(--usage-crit); outline-offset: -1px; }
</style>
