<script lang="ts">
  import Inspector from './Inspector.svelte';
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import LayerChangeForm from './LayerChangeForm.svelte';
  import { layerFootprint, whyChain } from './assets_layers';
  import { layerCards, layerSource } from './assets_layers';
  import type { ChangesetSummary, LayerChange, LayerDef, LayerListing } from './assets_workspace';

  /** A layer in the Inspector (spec, Workspace shell; mockups screen 3; tabs
   *  M15 G7.13, Assets board): Assets (its members, with Move…), Changeset
   *  (the proposed cards that change it, each opened in the list), Hosts,
   *  where each host answers "why is it on {host}?" with the role/context →
   *  extends chain, and Source (the layer as the catalog defines it). Rename
   *  and Move open an inline form that proposes a card (R5); nothing is
   *  committed from here. */
  let {
    catalog,
    layer,
    listing,
    order,
    writable,
    busy = false,
    cards = null,
    onopencard,
    onchange,
  }: {
    /** The changeset cards (`changesetSummaries`); null before they load. */
    cards?: readonly ChangesetSummary[] | null;
    /** Select a card in the workspace. */
    onopencard?: (id: number) => void;
    catalog: string;
    layer: LayerDef;
    listing: LayerListing;
    order: string[];
    writable: boolean;
    busy?: boolean;
    onchange: (c: LayerChange) => void;
  } = $props();

  let tab = $state<string>('members');
  /** The inline form: a rename, or a move of one member. */
  let form = $state<{ mode: 'rename' } | { mode: 'move'; member: string } | null>(null);

  const members = $derived(layer.members ?? []);
  const reached = $derived([...(layerFootprint(listing).get(layer.name) ?? [])].sort((a, b) => a.localeCompare(b)));
  const open = $derived(layerCards(cards ?? [], catalog, layer.name));
  const TABS = $derived([
    { id: 'members', label: `Assets ${members.length}` },
    { id: 'changeset', label: open.length > 0 ? `Changeset ${open.length}` : 'Changeset' },
    { id: 'hosts', label: 'Hosts' },
    { id: 'source', label: 'Source' },
  ]);
  const names = $derived(listing.layers.map((l) => l.name));
</script>

<Inspector eyebrow={`Layer · catalog ${catalog}`} title={layer.name} tabs={TABS} active={tab} onchange={(id) => (tab = id)} testid="layer-inspector">
  <div class="pad">
    <div class="meta">
      <Badge tone="muted" label={layer.axis} />
      {#if layer.extends}<span class="muted">extends <code>{layer.extends}</code></span>{/if}
      {#if layer.orgs?.length}<span class="muted" data-testid="layer-applies-by-org">applies by organisation: {layer.orgs.join(', ')}</span>{/if}
      <span class="grow"></span>
      {#if writable && form?.mode !== 'rename'}
        <button type="button" class="btn btn--quiet" data-testid="layer-rename" disabled={busy} onclick={() => (form = { mode: 'rename' })}>Rename</button>
      {/if}
    </div>
    {#if layer.description}<p class="muted">{layer.description}</p>{/if}
    {#if form}
      <LayerChangeForm
        mode={form.mode}
        catalogs={[catalog]}
        {catalog}
        layer={layer.name}
        member={form.mode === 'move' ? form.member : undefined}
        layers={names}
        {busy}
        onsubmit={onchange}
        oncancel={() => (form = null)}
      />
    {/if}
    {#if tab === 'members'}
      {#if members.length === 0}
        <p class="muted">No members yet. Assets join a layer by being adopted into it.</p>
      {:else}
        <ul class="list">
          {#each members as m (m)}
            <li data-testid={`layer-member-${m}`}>
              <code>{m}</code>
              {#if writable}
                <button type="button" class="btn btn--quiet" data-testid={`layer-move-${m}`} disabled={busy} onclick={() => (form = { mode: 'move', member: m })}>Move…</button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    {:else if tab === 'changeset'}
      {#if open.length === 0}
        <p class="muted" data-testid="layer-changeset-empty">No proposed card changes this layer.</p>
      {:else}
        <ul class="list">
          {#each open as c (c.id)}
            <li data-testid={`layer-card-${c.id}`}>
              <span>{c.summary}</span>
              {#if onopencard}
                <button type="button" class="btn btn--quiet" data-testid={`layer-card-open-${c.id}`} onclick={() => onopencard(c.id)}>Open</button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    {:else if tab === 'source'}
      <pre class="source" data-testid="layer-source">{layerSource(layer)}</pre>
    {:else}
      <div class="foot"><HostStrip {order} present={reached} /></div>
      {#if reached.length === 0}
        <p class="muted">No host receives this layer.</p>
      {:else}
        <ul class="list">
          {#each reached as h (h)}
            <li class="why">
              <span>Why is it on {h}?</span>
              <code data-testid={`layer-why-${h}`}>{whyChain(listing, h, layer.name).join(' → ')}</code>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</Inspector>

<style>
  .pad { display: grid; gap: 10px; padding: 12px 14px; align-content: start; }
  .meta { display: flex; align-items: center; gap: 8px; }
  .grow { flex: 1; }
  .muted { margin: 0; color: var(--fg-muted); font-size: var(--text-xs); }
  code { font-family: var(--mono); font-size: var(--text-2xs); overflow-wrap: anywhere; }
  .list { display: grid; gap: 4px; margin: 0; padding: 0; list-style: none; }
  .list li { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .list li.why { display: grid; gap: 2px; justify-content: stretch; padding: 6px 0; border-bottom: 1px solid var(--border); font-size: var(--text-xs); }
  .foot { display: flex; }
  .source { margin: 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-sunk); font-family: var(--mono); font-size: var(--text-2xs); white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
