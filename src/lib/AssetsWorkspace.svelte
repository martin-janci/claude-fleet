<script lang="ts">
  import type { Snippet } from 'svelte';
  import AssetsRail from './AssetsRail.svelte';
  import AssetsInbox from './AssetsInbox.svelte';
  import AssetList from './AssetList.svelte';
  import AssetInspector from './AssetInspector.svelte';
  import AssetsFooter from './AssetsFooter.svelte';
  import QueryInput from './QueryInput.svelte';
  import Badge from './Badge.svelte';
  import {
    catalog, identitiesOf, inventory, lastSyncRun, type AssetIdentity, type AssetSummary, type HostScanResult,
  } from './assets';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import {
    blockedOnSecrets, canWrite, catalogStatuses, changesetSummaries, keyOf, layerListing, parseKey, PERSONAL,
    type WorkspaceView,
  } from './assets_workspace';
  import { buildInbox, hostOrderOf, lastScanOf, sentence } from './assets_inbox';
  import { keep, parseQuery, type QueryRow } from './assets_query';

  /** The Assets workspace (spec, Workspace shell): rail · list with a
   *  sentence header and the query · Inspector, over a footer. It owns the
   *  view, the query and the selection; `AssetsPanel` owns loading, probing
   *  and every dialog, and is told what the person asked for. */
  let {
    readOnly = false,
    readOnlyClient = null,
    busy = '',
    error = null,
    scanResults = null,
    loading = false,
    scanDisabled = false,
    importBlocked = null,
    failed,
    selectedKey = $bindable(null),
    autoEditKey = '',
    onscan,
    onsync,
    onimport,
    onsecrets = () => {},
    onnew = () => {},
    onlintall = () => {},
    onpull = () => {},
    oncommit = () => {},
    onpush = () => {},
    onrefresh = () => {},
    ondeleted = () => {},
  }: {
    readOnly?: boolean;
    readOnlyClient?: string | null;
    /** Whether the Assets tab is shown; read by the keyboard (Task 12). */
    visible?: boolean;
    busy?: string;
    error?: string | null;
    scanResults?: HostScanResult[] | null;
    loading?: boolean;
    scanDisabled?: boolean;
    importBlocked?: string | null;
    /** Shown in the list area instead of the list (a failed load). */
    failed?: Snippet;
    selectedKey?: string | null;
    /** A just-created asset's key: the Inspector opens it in Source, editing. */
    autoEditKey?: string;
    onscan: () => void;
    onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void;
    onimport: (identity: AssetIdentity | null) => void;
    onsecrets?: () => void;
    onnew?: () => void;
    onlintall?: () => void;
    onpull?: () => void;
    oncommit?: () => void;
    onpush?: () => void;
    onrefresh?: () => void;
    ondeleted?: () => void;
  } = $props();


  let view = $state<WorkspaceView>('inbox');
  let queryText = $state('');
  let showProblems = $state(false);
  let showGrant = $state(false);
  const uid = $props.id();
  const grantNoteId = `${uid}-grant`;
  // Bumped by the `e` key (Task 12): open the selected asset in Source.
  let editNonce = $state(0);
  let listEl: HTMLElement | undefined = $state();
  let now = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (now = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  const listing = $derived($catalog);
  const shown = $derived($hosts.filter((h) => !h.hidden));
  const order = $derived(
    hostOrderOf([
      ...shown.map((h) => h.alias),
      ...(listing?.assets.flatMap((a) => a.hosts.map((s) => s.host_alias)) ?? []),
      ...(listing?.unmanaged.map((r) => r.host_alias) ?? []),
    ]),
  );
  const stale = $derived(new Set(shown.filter((h) => h.alias !== 'local' && !h.reachable).map((h) => h.alias)));
  const layersOf = (a: { kind: string; name: string; catalog?: string | null }) =>
    (a.catalog ?? PERSONAL) !== PERSONAL
      ? []
      : ($layerListing?.layers ?? []).filter((l) => (l.members ?? []).includes(`${a.kind}/${a.name}`)).map((l) => l.name);
  // "Behind the catalog" is computed whatever `catalog.auto` says; a copy the
  // last sync could not write for want of a secret only "differs".
  const blocked = $derived(blockedOnSecrets($lastSyncRun));
  const inbox = $derived(listing ? buildInbox({ listing, cards: $changesetSummaries, order, stale, layersOf, blocked }) : null);
  const head = $derived(
    inbox
      ? sentence(inbox, {
          reachable: shown.filter((h) => h.alias === 'local' || h.reachable).length,
          total: shown.length,
          lastScan: lastScanOf(listing, $inventory),
          now,
        })
      : null,
  );
  // Parsed once per change of the text; every row of every view asks the
  // same parsed query (PF11 / R21).
  const query = $derived(parseQuery(queryText));
  const keepRow = $derived((row: QueryRow) => keep(query, row));
  const vocab = $derived({
    hosts: order,
    layers: ($layerListing?.layers ?? []).map((l) => l.name),
    catalogs: $catalogStatuses?.map((c) => c.name) ?? [PERSONAL],
  });
  const counts = $derived({
    inbox: inbox?.needCount ?? 0,
    library: listing
      ? listing.assets.length + identitiesOf(listing).filter((i) => i.class === 'normal' || i.class === 'needs_person').length
      : 0,
  });
  const ctx = $derived({
    readOnly,
    remote: $hubStatus.remote,
    clientName: $hubStatus.client_name,
    statuses: $catalogStatuses,
  });
  const writable = (a: AssetSummary) => canWrite(a.catalog, ctx);
  // R19: the full detail and editor for a personal asset this window may
  // write; the desktop's authoring commands are personal until M6. An org
  // catalog's asset opens as a summary from the listing.
  const canOpen = (a: AssetSummary) => writable(a) && (a.catalog ?? PERSONAL) === PERSONAL;
  const selection = $derived(selectedKey ? parseKey(selectedKey) : null);
  const selectedAsset = $derived(
    selection?.type === 'asset' ? { kind: selection.kind, name: selection.name, catalog: selection.catalog } : null,
  );

  function select(key: string) {
    selectedKey = key;
  }
</script>

<div class="ws" data-testid="assets-workspace">
  <AssetsRail {view} {counts} {readOnly} busy={busy !== ''} onview={(v) => (view = v)} {onsecrets} />

  <div class="main">
    <header class="head">
      <div class="line">
        <span class="sentence" data-testid="assets-sentence">{head?.text ?? (loading ? 'Loading…' : 'Asset catalog')}</span>
        {#if head}<span class="sub">{head.sub}</span>{/if}
        {#if readOnly}
          <span class="ro">
            <button
              type="button"
              class="btn btn--chip"
              aria-expanded={showGrant}
              aria-controls={grantNoteId}
              onclick={() => (showGrant = !showGrant)}
              data-testid="assets-readonly">read-only · ask the operator to grant assets on personal</button
            >
            {#if showGrant}
              <span class="grant" role="note" id={grantNoteId}
                >On the hub's machine: <code data-testid="assets-grant-cmd"
                  >fleet-hub client grant {readOnlyClient ?? "<this client's name>"} assets</code
                >{#if !readOnlyClient}
                  (<code>fleet-hub client list</code> shows the name){/if}</span
              >
            {/if}
          </span>
        {/if}
        <span class="grow"></span>
        {#if listing && listing.problems.length > 0}
          <button
            type="button"
            class="btn btn--quiet"
            aria-expanded={showProblems}
            onclick={() => (showProblems = !showProblems)}
            data-testid="assets-problems"
          >
            <Badge tone="warn" glyph="!" label={`${listing.problems.length} problems`} />
          </button>
        {/if}
        {#if readOnly}
          <button type="button" class="btn btn--quiet" onclick={onrefresh} disabled={busy !== '' || loading} data-testid="assets-hub-refresh"
            >{loading ? 'Loading…' : 'Refresh'}</button
          >
        {/if}
        <button type="button" class="btn btn--quiet" onclick={onscan} disabled={busy !== '' || scanDisabled} data-testid="assets-scan"
          >{busy === 'scan' ? 'Scanning…' : 'Rescan'}</button
        >
        {#if !readOnly}
          <button type="button" class="btn btn--primary" onclick={() => onsync({})} disabled={busy !== ''} data-testid="assets-sync"
            >{busy === 'plan' ? 'Planning…' : 'Sync fleet'} <kbd>⌘↵</kbd></button
          >
        {/if}
      </div>
      <div class="line">
        <QueryInput bind:value={queryText} {vocab} onescape={() => listEl?.focus()} />
        {#if view === 'library' && !readOnly}
          <button type="button" class="btn btn--quiet" onclick={onnew} disabled={busy !== ''} data-testid="assets-new">New asset</button>
          <button
            type="button"
            class="btn btn--quiet"
            onclick={() => onimport(null)}
            disabled={busy !== '' || importBlocked !== null}
            title={importBlocked ?? ''}
            data-testid="assets-import">Import from host</button
          >
          <button type="button" class="btn btn--quiet" onclick={onlintall} disabled={busy !== ''} data-testid="assets-lint-all">Lint all</button>
        {/if}
      </div>
    </header>
    {#if error}<p class="error">{error}</p>{/if}
    {#if scanResults}
      <p class="scan-result" data-testid="assets-scan-result">
        {scanResults.map((r) => `${r.host}: ${r.status}${r.detail ? ` (${r.detail})` : ''}`).join(' · ')}
      </p>
    {/if}
    {#if showProblems && listing}
      <ul class="problems">{#each listing.problems as p (p.path)}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>
    {/if}
    <div class="body" bind:this={listEl} tabindex="-1" data-testid="assets-list">
      {#if failed}
        {@render failed()}
      {:else if listing && view === 'inbox' && inbox}
        <AssetsInbox {inbox} {order} {selectedKey} {query} readonly={readOnly} onselect={select} onimport={(i) => onimport(i)} />
      {:else if listing}
        <AssetList
          {listing}
          selected={selectedAsset}
          {selectedKey}
          filter=""
          keep={keepRow}
          canWrite={writable}
          {layersOf}
          openStatic
          readonly={readOnly}
          onselect={(kind, name, cat) => select(keyOf({ type: 'asset', catalog: cat ?? PERSONAL, kind, name }))}
          onpick={select}
          onimport={(i) => onimport(i)}
        />
      {:else}
        <p class="muted pad">Loading…</p>
      {/if}
    </div>
  </div>

  <div class="insp">
    <AssetInspector
      {selectedKey}
      {listing}
      cards={$changesetSummaries}
      hosts={$hosts}
      {order}
      {readOnly}
      {canOpen}
      {autoEditKey}
      {editNonce}
      {onsync}
      ondeleted={() => {
        selectedKey = null;
        ondeleted();
      }}
      onimport={(i) => onimport(i)}
    />
  </div>

  <div class="foot">
    <!-- Re-reads an org chip's repo status with each listing (R24). -->
    <AssetsFooter {readOnly} {listing} {busy} {onpull} {oncommit} {onpush} />
  </div>
</div>

<style>
  .ws {
    display: grid; flex: 1; min-height: 0;
    grid-template-columns: 172px minmax(0, 1fr) minmax(300px, 392px);
    grid-template-rows: minmax(0, 1fr) auto;
    grid-template-areas: 'rail main insp' 'foot foot foot';
  }
  .ws > :global(.rail) { grid-area: rail; }
  .main { grid-area: main; display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  .insp { grid-area: insp; min-height: 0; border-left: 1px solid var(--border); }
  .foot { grid-area: foot; }
  .head { display: grid; gap: 8px; padding: 10px 14px; border-bottom: 1px solid var(--border); }
  .line { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .sentence { font-size: 14px; font-weight: 600; letter-spacing: -0.005em; }
  .sub { color: var(--fg-muted); font-size: 12px; white-space: nowrap; }
  .grow { flex: 1; }
  .ro { display: inline-flex; align-items: center; gap: 8px; }
  .grant { font-size: 12px; color: var(--fg-muted); }
  .grant code, .problems code { font-family: var(--mono); font-size: 11.5px; user-select: text; }
  kbd { font-family: var(--mono); font-size: 10.5px; padding: 0 4px; border-radius: 3px; border: 1px solid color-mix(in srgb, currentColor 35%, transparent); opacity: 0.85; }
  .body { flex: 1; min-height: 0; overflow: auto; outline: 0; }
  .error { margin: 0; padding: 4px 14px; color: var(--usage-crit); }
  .scan-result { margin: 0; padding: 4px 14px; font-size: 12px; color: var(--fg-muted); }
  .problems { margin: 0; padding: 4px 14px 4px 32px; font-size: 12px; }
  .muted { color: var(--fg-muted); }
  .pad { padding: 14px; }
</style>
