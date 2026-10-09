<script lang="ts">
  import { groupByKind, stateCounts, identitiesOf, hostOrderOf, catalogOf, type AssetListing, type AssetIdentity, type AssetSummary } from './assets';
  import { assetDots } from './assets_inbox';
  import { keyOf, scopeBadge } from './assets_workspace';
  import { rowOfAsset, rowOfIdentity, rowOfOrphan, type QueryRow } from './assets_query';
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import IdentityRow from './IdentityRow.svelte';
  import RowName from './RowName.svelte';

  /** The Library (spec, Workspace shell: "every asset once, grouped by
   *  kind; managed-elsewhere assets read-only"; Rulings R20, R21). One row
   *  per (catalog, name) with its scope or catalog badge and one dot per
   *  host; what this window cannot write sits in its own "Managed,
   *  read-only" group as static rows; then S1a's identities and orphans. */
  let {
    listing,
    selected,
    selectedKey = null,
    filter,
    keep,
    canWrite,
    layersOf,
    openStatic = false,
    onselect,
    onpick,
    onimport,
    readonly = false,
  }: {
    listing: AssetListing;
    selected: { kind: string; name: string; catalog?: string } | null;
    /** The selected row's key, for the identity and orphan rows (an asset's
     *  is `selected`). */
    selectedKey?: string | null;
    /** The old free-text filter; the workspace passes '' and `keep`. */
    filter: string;
    /** The query as a row predicate (R21, PF11): the caller parses the query
     *  once and closes over it, so no row parses anything. */
    keep?: (row: QueryRow) => boolean;
    /** R20: false = this window may only look at the asset. */
    canWrite?: (a: AssetSummary) => boolean;
    /** A catalog asset's personal layers, for `layer:`. */
    layersOf?: (a: AssetSummary) => string[];
    /** Whether a static row (an org catalog's, a managed one) opens in the
     *  Inspector. Off, such a row is inert: the detail pane behind
     *  `onselect` reads the personal catalog only. */
    openStatic?: boolean;
    onselect: (kind: string, name: string, catalog?: string) => void;
    /** An identity or orphan row was picked (the Inspector shows it). */
    onpick?: (key: string) => void;
    onimport: (identity: AssetIdentity) => void;
    /** An overview (a hub-client desktop): no detail to open, nothing to
     *  import — each row says where the asset is and in what state. */
    readonly?: boolean;
  } = $props();

  const hostsTitle = (hosts: { host_alias: string; state: string }[]) =>
    hosts.length ? hosts.map((h) => `${h.host_alias}: ${h.state.replace('_', ' ')}`).join('\n') : 'not installed on any host';

  const lower = $derived(filter.toLowerCase());
  /** A read-only window is one chip, not a group: its rows are all
   *  managed to the query (`scope:managed`) but stay in their kinds. */
  const managed = (a: AssetSummary) => !readonly && canWrite?.(a) === false;
  const keeps = (row: QueryRow) => keep?.(row) ?? true;

  const kept = $derived(
    listing.assets.filter(
      (a) =>
        (filter === '' || a.name.includes(filter) || a.description.toLowerCase().includes(lower)) &&
        keeps({ ...rowOfAsset(a, layersOf?.(a) ?? []), managedElsewhere: readonly || canWrite?.(a) === false }),
    ),
  );
  const groups = $derived(groupByKind({ ...listing, assets: kept.filter((a) => !managed(a)) }));
  const managedRows = $derived(groupByKind({ ...listing, assets: kept.filter(managed) }).flatMap((g) => g.assets));

  const ids = $derived(
    identitiesOf(listing).filter((i) => (filter === '' || i.name.toLowerCase().includes(lower)) && keeps(rowOfIdentity(i))),
  );
  const orphans = $derived.by(() => {
    const by = new Map<string, typeof listing.unmanaged>();
    for (const r of listing.unmanaged) {
      if (r.state !== 'orphan') continue;
      by.set(`${r.kind}/${r.name}`, [...(by.get(`${r.kind}/${r.name}`) ?? []), r]);
    }
    return [...by.values()].filter((rows) => (filter === '' || rows[0].name.toLowerCase().includes(lower)) && keeps(rowOfOrphan(rows)));
  });

  /** One dot per host, in one fixed order across every row. */
  const order = $derived(
    hostOrderOf([
      ...listing.assets.flatMap((a) => a.hosts.map((h) => h.host_alias)),
      ...identitiesOf(listing).flatMap((i) => i.hosts.map((h) => h.host_alias)),
      ...listing.unmanaged.filter((r) => r.state === 'orphan').map((r) => r.host_alias),
    ]),
  );
  const noStale: ReadonlySet<string> = new Set();

  let showInternals = $state(false);
  const internal = (i: AssetIdentity) => i.class === 'fleet_internal' || i.class === 'harness_internal';
  const visible = $derived(ids.filter((i) => showInternals || !internal(i)));
  const hiddenCount = $derived(ids.filter(internal).length);

  const isSel = (a: AssetSummary) =>
    selected?.kind === a.kind && selected?.name === a.name && (selected?.catalog ?? 'personal') === catalogOf(a);
  const assetKey = (a: AssetSummary) => keyOf({ type: 'asset', catalog: catalogOf(a), kind: a.kind, name: a.name });
  const idKey = (i: { kind: string; name: string }) => keyOf({ type: 'identity', kind: i.kind, name: i.name });
  const orphanKey = (r: { kind: string; name: string }) => keyOf({ type: 'orphan', kind: r.kind, name: r.name });

  /** Assets M5 (PF9): the listing spans every catalog, so a row is keyed by
   *  (catalog, name). The old detail and the editor are the personal
   *  catalog's (`catalog_get_asset`): a personal row this window may write
   *  opens one; every other row is static, and opens only when the Inspector
   *  can show it from the listing (`openStatic`). */
  const isPersonal = (a: AssetSummary) => catalogOf(a) === 'personal';
  const rowTestid = (a: AssetSummary) =>
    isPersonal(a) ? `asset-row-${a.kind}-${a.name}` : `asset-row-${catalogOf(a)}-${a.kind}-${a.name}`;
  const openStaticRow = (a: AssetSummary) => {
    if (openStatic) onselect(a.kind, a.name, catalogOf(a));
  };
  function onStaticKey(e: KeyboardEvent, a: AssetSummary) {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    openStaticRow(a);
  }
  function onPickKey(e: KeyboardEvent, key: string) {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    onpick?.(key);
  }
</script>

{#snippet assetRow(a: AssetSummary, withKind: boolean)}
  {@const c = stateCounts(a.hosts)}
  {@const b = scopeBadge(a)}
  {#snippet body()}
    <RowName kind={withKind ? a.kind : undefined} name={a.name}>
      <Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} />
      {#if managed(a)}<Badge tone="muted" label="managed" title="This window can only look at it" />{/if}
      {#if a.version && (readonly || !isPersonal(a) || managed(a))}<span class="meta">{a.version}</span>{/if}
    </RowName>
    <HostStrip {order} states={assetDots(a, order, noStale)} />
    <span class="chips">
      {#if c.in_sync}<Badge tone="ok" glyph="●" label={`${c.in_sync} in sync`} />{/if}
      {#if c.drifted}<Badge tone="warn" glyph="◐" label={`${c.drifted} drifted`} />{/if}
      {#if c.missing}<Badge tone="muted" glyph="○" label={`${c.missing} missing`} />{/if}
      {#if c.unsupported}<Badge tone="muted" glyph="–" label={`${c.unsupported} unsupported`} />{/if}
    </span>
  {/snippet}
  {#if readonly || !isPersonal(a) || managed(a)}
    <div
      class="row static"
      class:selected={isSel(a)}
      class:inert={!openStatic}
      role="button"
      tabindex="-1"
      aria-current={isSel(a) ? 'true' : undefined}
      aria-disabled={openStatic ? undefined : 'true'}
      title={hostsTitle(a.hosts)}
      data-row-key={assetKey(a)}
      data-testid={rowTestid(a)}
      onclick={() => openStaticRow(a)}
      onkeydown={(e) => onStaticKey(e, a)}
    >
      {@render body()}
    </div>
  {:else}
    <button
      type="button"
      class="row"
      class:selected={isSel(a)}
      aria-current={isSel(a) ? 'true' : undefined}
      data-row-key={assetKey(a)}
      data-testid={rowTestid(a)}
      onclick={() => onselect(a.kind, a.name, catalogOf(a))}
    >
      {@render body()}
    </button>
  {/if}
{/snippet}

<div class="asset-list">
  {#each groups as g (g.kind)}
    <div class="group-header">{g.label} <span class="count">{g.assets.length}</span></div>
    {#each g.assets as a (`${catalogOf(a)}:${a.name}`)}{@render assetRow(a, false)}{/each}
  {/each}
  {#if managedRows.length}
    <div class="group-header" data-testid="library-managed-header">Managed, read-only <span class="count">{managedRows.length}</span></div>
    <div data-testid="library-managed">
      {#each managedRows as a (`${catalogOf(a)}:${a.kind}:${a.name}`)}{@render assetRow(a, true)}{/each}
    </div>
  {/if}
  {#if visible.length > 0 || hiddenCount > 0}
    <div class="group-header">On hosts, not in catalog <span class="count">{visible.length}</span></div>
    {#each visible as i (`${i.kind}:${i.name}`)}
      {#if onpick}
        <IdentityRow
          identity={i}
          {order}
          {readonly}
          {onimport}
          testid={`identity-row-${i.kind}-${i.name}`}
          select={{ key: idKey(i), testid: `identity-row-${i.kind}-${i.name}`, selected: selectedKey === idKey(i), onselect: () => onpick(idKey(i)) }}
        />
      {:else}
        <IdentityRow identity={i} {order} {readonly} {onimport} rowKey={idKey(i)} testid={`identity-row-${i.kind}-${i.name}`} />
      {/if}
    {/each}
    {#if hiddenCount > 0}
      <button class="link toggle" onclick={() => (showInternals = !showInternals)}>
        {showInternals ? 'Hide' : 'Show'} {hiddenCount} fleet internal{hiddenCount === 1 ? '' : 's'}
      </button>
    {/if}
  {/if}
  {#each orphans as rows (orphanKey(rows[0]))}
    {@const r = rows[0]}
    {@const hosts = [...new Set(rows.map((x) => x.host_alias))]}
    {#snippet body()}
      <RowName name={r.name}>
        <span class="meta">{r.kind} · {hosts.join(', ')}</span>
        <Badge tone="warn" label="orphan" testid={`orphan-badge-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`} />
      </RowName>
      <HostStrip {order} states={Object.fromEntries(hosts.map((h) => [h, 'differs' as const]))} />
    {/snippet}
    {#if onpick}
      <div
        class="row unmanaged"
        class:selected={selectedKey === orphanKey(r)}
        role="button"
        tabindex="-1"
        aria-current={selectedKey === orphanKey(r) ? 'true' : undefined}
        data-row-key={orphanKey(r)}
        data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}
        onclick={() => onpick(orphanKey(r))}
        onkeydown={(e) => onPickKey(e, orphanKey(r))}
      >
        {@render body()}
      </div>
    {:else}
      <div class="row unmanaged plain" data-row-key={orphanKey(r)} data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>
        {@render body()}
      </div>
    {/if}
  {/each}
</div>

<style>
  /* Scrolls inside its one home, the workspace's list body. */
  .asset-list { font-size: var(--text-sm); }
  .group-header { padding: 8px 10px 4px; color: var(--fg-muted); font-size: var(--text-2xs); text-transform: uppercase; letter-spacing: 0.04em; }
  .count { opacity: 0.7; margin-left: 4px; }
  .row {
    display: flex; align-items: center; gap: 10px; width: 100%; box-sizing: border-box; text-align: left;
    min-height: 34px; padding: 0 14px; background: none; border: 0; border-bottom: 1px solid var(--border);
    color: var(--fg); font: inherit; cursor: pointer;
  }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .row:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .row.plain { cursor: default; }
  .row.static { cursor: pointer; }
  /* A static row nothing can open yet: no hover that promises a click. */
  .row.static.inert { cursor: default; }
  .row.static.inert:hover { background: none; }
  .meta { color: var(--fg-muted); font-size: var(--text-2xs); }
  .chips { display: flex; gap: 4px; margin-left: auto; }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; font-size: var(--text-xs); }
  .link.toggle { display: block; padding: 4px 10px; font-size: var(--text-2xs); }
</style>
