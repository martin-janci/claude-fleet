<script lang="ts">
  import { groupByKind, stateCounts, identitiesOf, hostOrder, oddHosts, catalogOf, type AssetListing, type AssetIdentity, type AssetSummary } from './assets';
  import HostStrip from './HostStrip.svelte';
  import Badge from './Badge.svelte';

  let {
    listing,
    selected,
    filter,
    onselect,
    onimport,
    readonly = false,
  }: {
    listing: AssetListing;
    selected: { kind: string; name: string } | null;
    filter: string;
    onselect: (kind: string, name: string) => void;
    onimport: (identity: AssetIdentity) => void;
    /** An overview (a hub-client desktop): no detail to open, nothing to
     *  import — each row says where the asset is and in what state. */
    readonly?: boolean;
  } = $props();

  const hostsTitle = (hosts: { host_alias: string; state: string }[]) =>
    hosts.length ? hosts.map((h) => `${h.host_alias}: ${h.state.replace('_', ' ')}`).join('\n') : 'not installed on any host';

  const groups = $derived(
    groupByKind(listing).map((g) => ({
      ...g,
      assets: g.assets.filter((a) => filter === '' || a.name.includes(filter) || a.description.toLowerCase().includes(filter.toLowerCase())),
    })).filter((g) => g.assets.length > 0),
  );
  const ids = $derived(identitiesOf(listing).filter((i) => filter === '' || i.name.toLowerCase().includes(filter.toLowerCase())));
  const order = $derived(hostOrder(identitiesOf(listing)));
  let showInternals = $state(false);
  const internal = (i: AssetIdentity) => i.class === 'fleet_internal' || i.class === 'harness_internal';
  const visible = $derived(ids.filter((i) => showInternals || !internal(i)));
  const hiddenCount = $derived(ids.filter(internal).length);
  const orphans = $derived(listing.unmanaged.filter((r) => r.state === 'orphan' && (filter === '' || r.name.toLowerCase().includes(filter.toLowerCase()))));
  const isSel = (kind: string, name: string) => selected?.kind === kind && selected?.name === name;
  /** Assets M5 (PF9): the listing spans every catalog, so a row is keyed by
   *  (catalog, name). The detail and the editor are the personal catalog's
   *  (`catalog_get_asset`), so only a personal row opens one; an org
   *  catalog's row is a static row naming its catalog until the Inspector
   *  reads org assets from the listing (R19). */
  const isPersonal = (a: AssetSummary) => catalogOf(a) === 'personal';
  const rowTestid = (a: AssetSummary) =>
    isPersonal(a) ? `asset-row-${a.kind}-${a.name}` : `asset-row-${catalogOf(a)}-${a.kind}-${a.name}`;
</script>

<div class="asset-list">
  {#each groups as g (g.kind)}
    <div class="group-header">{g.label} <span class="count">{g.assets.length}</span></div>
    {#each g.assets as a (`${catalogOf(a)}:${a.name}`)}
      {@const c = stateCounts(a.hosts)}
      {#snippet chips()}
        <span class="chips">
          {#if c.in_sync}<Badge tone="ok" glyph="●" label={`${c.in_sync} in sync`} />{/if}
          {#if c.drifted}<Badge tone="warn" glyph="◐" label={`${c.drifted} drifted`} />{/if}
          {#if c.missing}<Badge tone="muted" glyph="○" label={`${c.missing} missing`} />{/if}
          {#if c.unsupported}<Badge tone="muted" glyph="–" label={`${c.unsupported} unsupported`} />{/if}
        </span>
      {/snippet}
      {#if readonly || !isPersonal(a)}
        <div class="row static" title={hostsTitle(a.hosts)} data-testid={rowTestid(a)}>
          <span class="name">{a.name}</span>
          {#if !isPersonal(a)}<span class="meta">{catalogOf(a)}</span>{/if}
          {#if a.version}<span class="meta">{a.version}</span>{/if}
          {@render chips()}
        </div>
      {:else}
        <button class="row" class:selected={isSel(a.kind, a.name)} onclick={() => onselect(a.kind, a.name)} data-testid={rowTestid(a)}>
          <span class="name">{a.name}</span>
          {@render chips()}
        </button>
      {/if}
    {/each}
  {/each}
  {#if visible.length > 0 || hiddenCount > 0}
    <div class="group-header">On hosts, not in catalog <span class="count">{visible.length}</span></div>
    {#each visible as i (`${i.kind}:${i.name}`)}
      <div class="row unmanaged" data-testid={`identity-row-${i.kind}-${i.name}`}>
        <span class="name">{i.name}</span>
        <span class="meta">{i.kind}</span>
        {#if i.class === 'needs_person'}<Badge tone="warn" label={i.reason ?? 'needs a person'} title={i.reason ?? ''} />{/if}
        <HostStrip {order} present={[...new Set(i.hosts.map((h) => h.host_alias))]} odd={oddHosts(i)} />
        {#if !readonly && !internal(i)}
          <button class="link" onclick={() => onimport(i)} title="Import this asset">Import</button>
        {/if}
      </div>
    {/each}
    {#if hiddenCount > 0}
      <button class="link toggle" onclick={() => (showInternals = !showInternals)}>
        {showInternals ? 'Hide' : 'Show'} {hiddenCount} fleet internal{hiddenCount === 1 ? '' : 's'}
      </button>
    {/if}
  {/if}
  {#each orphans as r (`${r.host_alias}:${r.harness}:${r.kind}:${r.name}`)}
    <div class="row unmanaged" data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>
      <span class="name">{r.name}</span>
      <span class="meta">{r.kind} · {r.host_alias}</span>
      <Badge tone="warn" label="orphan" testid={`orphan-badge-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`} />
    </div>
  {/each}
</div>

<style>
  .asset-list { overflow: auto; height: 100%; font-size: 13px; }
  .group-header { padding: 8px 10px 4px; color: var(--fg-muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; }
  .count { opacity: 0.7; margin-left: 4px; }
  .row { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; padding: 5px 10px; background: none; border: 0; color: var(--fg); cursor: pointer; }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--bg-pane); box-shadow: inset 2px 0 0 var(--accent); }
  .row.unmanaged, .row.static { cursor: default; }
  .row.static:hover { background: none; }
  .name { flex: 1; font-family: ui-monospace, monospace; }
  .meta { color: var(--fg-muted); font-size: 11px; }
  .chips { display: flex; gap: 4px; }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; font-size: 12px; }
  .link.toggle { display: block; padding: 4px 10px; font-size: 11px; }
</style>
