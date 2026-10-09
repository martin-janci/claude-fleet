<script lang="ts">
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import { PERSONAL, keyOf, type LayerListing } from './assets_workspace';
  import { layerFootprint } from './assets_layers';

  /** Layers (spec, Workspace shell; mockups screen 3; R17): every loaded
   *  catalog's layers, grouped by catalog, each with its member count and
   *  footprint (the hosts it reaches). Create / rename / move produce cards
   *  (R5), never direct commits. */
  let { layers, order, selectedKey, readOnly, canPropose = true, busy, onselect, onnew, onpropose }: {
    layers: Record<string, LayerListing> | null;
    order: string[];
    selectedKey: string | null;
    readOnly: boolean;
    /** The personal grant, which the hub's propose needs (final review minor 6). */
    canPropose?: boolean;
    busy: boolean;
    onselect: (key: string) => void;
    onnew: () => void;
    onpropose: () => void;
  } = $props();

  const catalogs = $derived(Object.keys(layers ?? {}).sort((a, b) => (a === PERSONAL ? -1 : b === PERSONAL ? 1 : a.localeCompare(b))));
  const total = $derived(catalogs.reduce((n, c) => n + (layers?.[c]?.layers.length ?? 0), 0));
</script>

<div class="layers" data-testid="layers-view">
  <div class="head">
    <span class="sentence">{total} layer{total === 1 ? '' : 's'} across {catalogs.length} catalog{catalogs.length === 1 ? '' : 's'}</span>
    {#if !readOnly}
      <button type="button" class="btn" data-testid="layers-new" disabled={busy} onclick={onnew}>New layer</button>
      {#if canPropose}
        <button type="button" class="btn btn--quiet" data-testid="layers-propose" disabled={busy} onclick={onpropose}>Propose again</button>
      {/if}
    {/if}
  </div>
  {#if total === 0}
    <p class="quiet">No layers yet. Adopt assets from the Inbox, or make one with New layer.</p>
  {/if}
  {#each catalogs as cat (cat)}
    {@const l = layers![cat]}
    {@const foot = layerFootprint(l)}
    <section data-testid={`layers-catalog-${cat}`}>
      <h3 class="grp">{cat} <span class="n">{l.layers.length}</span></h3>
      {#each l.layers as layer (layer.name)}
        {@const key = keyOf({ type: 'layer', catalog: cat, name: layer.name })}
        <button
          type="button"
          class="row"
          class:selected={selectedKey === key}
          aria-current={selectedKey === key ? 'true' : undefined}
          data-row-key={key}
          data-testid={`layer-row-${cat}-${layer.name}`}
          onclick={() => onselect(key)}
        >
          <b>{layer.name}</b>
          <Badge tone="muted" label={layer.axis} />
          <span class="num">{layer.members?.length ?? 0}</span>
          <HostStrip {order} present={[...(foot.get(layer.name) ?? [])]} />
        </button>
      {/each}
    </section>
  {/each}
</div>

<style>
  .layers { display: grid; align-content: start; }
  .head { display: flex; align-items: center; gap: 8px; padding: 8px 12px; }
  .sentence { flex: 1; font-weight: 600; }
  .grp { margin: 0; padding: 8px 12px 4px; font-size: var(--text-2xs); text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  .n { font-variant-numeric: tabular-nums; }
  .row { display: grid; grid-template-columns: minmax(0, 1fr) auto 44px auto; gap: 8px; align-items: center; width: 100%; padding: 4px 12px; border: 0; background: none; text-align: left; font: inherit; }
  .row.selected { background: var(--accent-soft); }
  .row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: -2px; }
  .num { text-align: right; font-variant-numeric: tabular-nums; color: var(--fg-muted); }
  .quiet { padding: 12px; color: var(--fg-muted); }
</style>
