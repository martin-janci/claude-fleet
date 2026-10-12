<script lang="ts">
  import { untrack } from 'svelte';
  import type { LayerChange } from './assets_workspace';
  import { orgs } from './orgs';

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
  // Toolkit forms board, "Applies by": a layer is assigned host by host, or
  // applies by organisation — every host of that org takes it on top of its
  // own layers (`Layer::orgs`; a context layer, so the axis is fixed).
  let appliesBy = $state<'host' | 'org'>('host');
  const orgNames = $derived($orgs.map((o) => o.name));
  let org = $state('');
  $effect(() => {
    if (org === '' && orgNames.length > 0) org = orgNames[0];
  });
  const targets = $derived(layers.filter((l) => l !== layer));
  let to = $state(untrack(() => layers.filter((l) => l !== layer)[0] ?? ''));

  const nameOk = $derived(NAME.test(name));
  const nameError = $derived(mode !== 'move' && name !== '' && !nameOk ? 'Use lowercase letters, digits and dashes.' : '');
  const valid = $derived(
    mode === 'move'
      ? targets.includes(to)
      : nameOk && (mode !== 'rename' || name !== layer) && (mode !== 'create' || appliesBy === 'host' || orgNames.includes(org)),
  );

  function submit(e: Event) {
    e.preventDefault();
    if (!valid || busy) return;
    if (mode === 'create' && appliesBy === 'org') onsubmit({ op: 'create', catalog: cat, layer: name, axis: 'context', orgs: [org] });
    else if (mode === 'create') onsubmit({ op: 'create', catalog: cat, layer: name, axis });
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
      <fieldset class="f by" data-testid="layer-form-applies-by">
        <legend>Applies by</legend>
        <label><input type="radio" bind:group={appliesBy} value="host" data-testid="layer-form-by-host" /> Host</label>
        <label title={orgNames.length === 0 ? 'No organisations yet.' : undefined}
          ><input type="radio" bind:group={appliesBy} value="org" disabled={orgNames.length === 0} data-testid="layer-form-by-org" /> Organisation</label
        >
      </fieldset>
      {#if appliesBy === 'org'}
        <label class="f">
          <span>To</span>
          <select bind:value={org} data-testid="layer-form-org">
            {#each orgNames as o (o)}<option value={o}>{o}</option>{/each}
          </select>
        </label>
        <p class="note" data-testid="layer-form-org-note">
          A context layer every host of {org} takes on top of its own layers. A host with no layers in this catalog keeps the whole catalog.
        </p>
      {:else}
        <label class="f">
          <span>Axis</span>
          <select bind:value={axis} data-testid="layer-form-axis">
            <option value="context">context</option>
            <option value="role">role</option>
          </select>
        </label>
      {/if}
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
  .by { display: flex; gap: 12px; align-items: center; margin: 0; padding: 0; border: 0; }
  .by legend { float: left; margin-right: 4px; padding: 0; color: var(--fg-muted); }
  .by label { display: inline-flex; gap: 4px; align-items: center; }
  .note { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .err { margin: 0; font-size: var(--text-xs); color: var(--usage-crit); }
  .act { display: flex; gap: 8px; }
  input[aria-invalid='true'] { outline: 2px solid var(--usage-crit); outline-offset: -1px; }
</style>
