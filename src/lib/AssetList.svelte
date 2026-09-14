<script lang="ts">
  import { groupByKind, stateCounts, type AssetListing, type AssetInventoryRow } from './assets';

  let {
    listing,
    selected,
    filter,
    onselect,
    onimport,
  }: {
    listing: AssetListing;
    selected: { kind: string; name: string } | null;
    filter: string;
    onselect: (kind: string, name: string) => void;
    onimport: (row: AssetInventoryRow) => void;
  } = $props();

  const groups = $derived(
    groupByKind(listing).map((g) => ({
      ...g,
      assets: g.assets.filter((a) => filter === '' || a.name.includes(filter) || a.description.toLowerCase().includes(filter.toLowerCase())),
    })).filter((g) => g.assets.length > 0),
  );
  const unmanaged = $derived(listing.unmanaged.filter((r) => filter === '' || r.name.includes(filter)));
  const isSel = (kind: string, name: string) => selected?.kind === kind && selected?.name === name;
</script>

<div class="asset-list">
  {#each groups as g (g.kind)}
    <div class="group-header">{g.label} <span class="count">{g.assets.length}</span></div>
    {#each g.assets as a (a.name)}
      {@const c = stateCounts(a.hosts)}
      <button class="row" class:selected={isSel(a.kind, a.name)} onclick={() => onselect(a.kind, a.name)} data-testid={`asset-row-${a.kind}-${a.name}`}>
        <span class="name">{a.name}</span>
        <span class="chips">
          {#if c.in_sync}<span class="chip ok">{c.in_sync} in sync</span>{/if}
          {#if c.drifted}<span class="chip warn">{c.drifted} drifted</span>{/if}
          {#if c.missing}<span class="chip muted">{c.missing} missing</span>{/if}
          {#if c.unsupported}<span class="chip muted">{c.unsupported} unsupported</span>{/if}
        </span>
      </button>
    {/each}
  {/each}
  {#if unmanaged.length > 0}
    <div class="group-header">On hosts, not in catalog <span class="count">{unmanaged.length}</span></div>
    {#each unmanaged as r (`${r.host_alias}:${r.harness}:${r.kind}:${r.name}`)}
      <div class="row unmanaged" data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>
        <span class="name">{r.name}</span>
        <span class="meta">{r.kind} · {r.host_alias}</span>
        <button class="link" onclick={() => onimport(r)} title="Import from this host">Import</button>
      </div>
    {/each}
  {/if}
</div>

<style>
  .asset-list { overflow: auto; height: 100%; font-size: 13px; }
  .group-header { padding: 8px 10px 4px; color: var(--fg-muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; }
  .count { opacity: 0.7; margin-left: 4px; }
  .row { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; padding: 5px 10px; background: none; border: 0; color: var(--fg); cursor: pointer; }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--bg-pane); box-shadow: inset 2px 0 0 var(--accent); }
  .row.unmanaged { cursor: default; }
  .name { flex: 1; font-family: ui-monospace, monospace; }
  .meta { color: var(--fg-muted); font-size: 11px; }
  .chips { display: flex; gap: 4px; }
  .chip { font-size: 10px; padding: 1px 6px; border-radius: 8px; border: 1px solid var(--border); }
  .chip.ok { color: #16a34a; } .chip.warn { color: #d97706; } .chip.muted { color: var(--fg-muted); }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; font-size: 12px; }
</style>
