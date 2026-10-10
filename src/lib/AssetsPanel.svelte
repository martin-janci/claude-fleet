<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import {
    catalog, catalogConfig, loadCatalogConfig, configureCatalog, loadCatalog, loadAssets, loadInventory, scanHosts,
    planSync, lastSync, lastSyncRun,
    commitPending, pushCatalog, repoStatus, repoStatusStore,
    type HostScanResult, type SyncPlan, type SyncRunSummary, type AssetKind, type AssetIdentity,
  } from './assets';
  import AssetsWorkspace from './AssetsWorkspace.svelte';
  import {
    canWrite, catalogStatuses, keyOf, loadAllLayers, loadCatalogStatuses, loadChangesets, loadLayers, PERSONAL,
    type WorkspaceView,
  } from './assets_workspace';
  import { proposeAndReload } from './assets_cards';
  import { assetsViewRequest, type AssetsViewRequest } from './app_views';
  import { push } from './toasts';
  import { loadFleetSettings } from './fleet_settings';
  import type { IpcError } from './result';
  import ImportDialog from './ImportDialog.svelte';
  import SecretsPanel from './SecretsPanel.svelte';
  import NewAssetDialog from './NewAssetDialog.svelte';
  import CommitAssetsDialog from './CommitAssetsDialog.svelte';
  import AuthorSessionDialog from './AuthorSessionDialog.svelte';
  import LintAllDialog from './LintAllDialog.svelte';
  import { authorSessionOpened, clearAuthorSessionOpened } from './AuthorSessionDialog.svelte';
  import { hubStatus, hubBlock, hubActionBlocked, ownsTheFleet } from './hub';
  import { hubConnection } from './hub_connection';

  let { visible }: { visible: boolean } = $props();

  let setupPath = $state('~/agent-assets');
  let setupRemote = $state('');
  let busy = $state<'' | 'setup' | 'pull' | 'scan' | 'plan' | 'apply' | 'commit' | 'push' | 'propose'>('');
  // The workspace's own card verbs (apply, dismiss, undo, admit, propose
  // again), bound up so the switcher's commands wait for them too.
  let cardBusy = $state('');
  // Per-ACTION error only (setup / scan / plan / commit / push). Four unrelated
  // toolbar handlers clear this on entry, so the catalog's own load state must
  // never be inferred from it — doing that made a successful Sync turn the
  // failed-load Retry block back into a permanent "Loading…".
  let error = $state<string | null>(null);
  // The catalog load's own state, owned by `reload()` alone.
  let catalogLoad = $state<'idle' | 'loading' | 'loaded' | 'failed'>('idle');
  let catalogLoadError = $state<string | null>(null);
  let scanResults = $state<HostScanResult[] | null>(null);
  let showImport = $state(false);
  // Preset from clicking Import next to an unmanaged identity in AssetList:
  // which host to read and which single asset to limit the import to. Reset
  // to null (the dialog's own "local" / "everything" defaults) by the
  // Library's own Import button and whenever the dialog closes.
  let importPreset = $state<{ host: string; only: string[] } | null>(null);
  // The workspace's selection: one row key (`keyOf`), shared by every view
  // and the Inspector.
  let selectedKey = $state<string | null>(null);
  // The workspace's view, bound so a switcher request can show the row it
  // selects (`library`) or the cards it proposes (`inbox`).
  let view = $state<WorkspaceView>('inbox');
  let syncPlan = $state<SyncPlan | null>(null);
  // The filter `syncPlan` was computed from, so the plan view's "Plan
  // anyway" (on a skipped-unlayered host) can re-plan with the same scope
  // plus allowUnlayered.
  let syncFilter = $state<{ hostAlias?: string; kind?: string; name?: string }>({});
  // The most recent plan computed (kept after the plan view closes) so the
  // SecretsPanel can offer the names its blocked/missing-secret actions
  // named, without recomputing a plan just to open it.
  let lastPlan = $state<SyncPlan | null>(null);
  let showSecrets = $state(false);
  let showNewAsset = $state(false);
  let showLintAll = $state(false);
  let showCommitPrompt = $state(false);
  /** "Write it with Claude…" from New asset (G2.6): its seeded instructions. */
  let authorNew = $state<string | null>(null);
  // A just-created asset's key, set together with the selection so the
  // Inspector opens it in Source, editing, and cleared once that happened:
  // selecting the asset again later opens its Overview.
  let pendingAutoEdit = $state('');

  async function openEditing(key: string) {
    pendingAutoEdit = key;
    selectedKey = key;
    await tick();
    if (pendingAutoEdit === key) pendingAutoEdit = '';
  }

  async function refresh() {
    // The workspace's own reads (R13, R14): best effort — a refusal shows
    // less, never an error. The cards refresh with every panel refresh.
    // Every loaded catalog's layers (R17) follow the statuses that name them.
    void loadCatalogStatuses().then(() => loadAllLayers(get(catalogStatuses)));
    void loadChangesets();
    void loadLayers();
    const [a, i] = await Promise.all([loadAssets(), loadInventory()]);
    if (!a.ok) error = a.error.message;
    if (!i.ok) error = i.error.message;
  }

  // Shared by every path that needs to re-read the catalog: `pull: false`
  // just reloads the working tree as-is (used after import, and on initial
  // mount/setup); `pull: true` is only the explicit "Pull" button. Answers
  // the load's error, if any.
  async function reload(pull: boolean): Promise<IpcError | null> {
    error = null;
    catalogLoad = 'loading';
    catalogLoadError = null;
    const l = await loadCatalog(pull);
    if (!l.ok) { catalogLoad = 'failed'; catalogLoadError = l.error.message; return l.error; }
    catalogLoad = 'loaded';
    await refresh();
    return null;
  }

  // Every authoring write (create/update/delete/resource/commit/push) goes
  // through this: reload the asset list and refresh the dirty/ahead/behind
  // status strip together, so the toolbar never shows a status stale
  // relative to what's in the list.
  async function afterWrite() {
    await Promise.all([refresh(), repoStatus()]);
  }

  // On a hub-backed desktop the catalog is the HUB's: a git checkout on the
  // hub's machine. Every command this panel uses routes to the hub's
  // `catalog_admin`, which answers a client the operator granted
  // (`fleet-hub client grant <name> assets`) and refuses any other with
  // E_FORBIDDEN. So the panel asks once (`catalog_config`): granted, it is
  // the full panel onto the hub's catalog; refused, it is the read-only
  // overview below, built on the two commands open to every paired client.
  const hubOverview = $derived($hubStatus.remote && !$hubStatus.unavailable);
  let hubAdmin = $state<'idle' | 'probing' | 'granted' | 'denied'>('idle');
  // Why the probe did not grant: E_FORBIDDEN is the ordinary answer (not
  // granted); anything else (an old hub, the link down) is shown as is.
  let hubAdminError = $state<string | null>(null);
  async function probeHubAdmin() {
    hubAdmin = 'probing';
    hubAdminError = null;
    const c = await loadCatalogConfig();
    if (!c.ok) {
      hubAdmin = 'denied';
      if (c.error.code !== 'E_FORBIDDEN') hubAdminError = c.error.message;
      return;
    }
    hubAdmin = 'granted';
    if (c.value) {
      await reload(false);
      void repoStatus();
    }
    void lastSync();
  }
  $effect(() => {
    if (!hubOverview || !visible) return;
    if (untrack(() => hubAdmin) !== 'idle') return;
    void probeHubAdmin();
  });
  // A configured hub this launch cannot use: nothing here can work. Any
  // still-refusing catalog key works here — `hubBlock` returns the
  // "unavailable" sentence regardless of which action it's asked about.
  const hubUnavailable = $derived($hubStatus.unavailable ? hubBlock('catalog_spawn_author_session', $hubStatus) : null);
  // Not the full panel: the hub's read-only overview until (and unless) the
  // probe says this client may manage the catalog.
  const catalogBlocked = $derived(hubUnavailable !== null || (hubOverview && hubAdmin !== 'granted'));
  // Import now routes to the hub (Task 6: any host, over SSH), so it is
  // blocked only while the live connection to the hub is down — same check
  // every other routed mutation gates on. "Open in session" stays this
  // machine's (see its REASONS entry).
  const importBlocked = $derived(hubActionBlocked('catalog_import_host', $hubStatus, $hubConnection));
  // A session is this machine's (local-only): no "Write it with Claude" on a hub client.
  const authorNewBlocked = $derived(hubBlock('catalog_spawn_author_session', $hubStatus));

  // ...but the list itself, and the scan that refreshes it, route for every
  // paired client: the read-only overview of the hub's catalog — which asset
  // is installed where, what drifted, what is on a host but not in the
  // catalog. Loaded once the window is known to be a hub client (the status
  // can resolve after mount).
  let overviewLoad = $state<'idle' | 'loading' | 'loaded' | 'failed'>('idle');
  let overviewError = $state<string | null>(null);
  let overviewNotConfigured = $state(false);
  async function loadOverview() {
    overviewLoad = 'loading';
    overviewError = null;
    overviewNotConfigured = false;
    const r = await loadAssets();
    if (r.ok) {
      overviewLoad = 'loaded';
      // An ungranted client may list catalogs and cards when unbound (M3
      // PF15) and is refused otherwise; never `catalog_last_sync` or
      // `catalog_load`.
      void loadCatalogStatuses();
      void loadChangesets();
      return;
    }
    overviewLoad = 'failed';
    overviewNotConfigured = r.error.code === 'E_CATALOG_NOT_CONFIGURED';
    overviewError = r.error.message;
  }
  $effect(() => {
    if (!hubOverview || !visible || hubAdmin !== 'denied') return;
    if (untrack(() => overviewLoad) !== 'idle') return;
    void loadOverview();
  });
  async function scanOnHub() {
    busy = 'scan'; error = null; scanResults = null;
    const r = await scanHosts();
    busy = '';
    if (!r.ok) { error = r.error.message; return; }
    scanResults = r.value;
    await loadOverview();
  }

  // Whether this machine's own catalog config has been read (the fleet owner's
  // path); a hub client settles through the grant probe instead.
  let configRead = $state(false);
  onMount(async () => {
    // The footer's `auto: on|off`.
    void loadFleetSettings();
    if (!ownsTheFleet($hubStatus)) { configRead = true; return; }
    const c = await loadCatalogConfig();
    configRead = true;
    if (c.ok && c.value) {
      await reload(false);
      void repoStatus();
    }
    void lastSync();
  });

  // ── A request from the quick switcher (Assets M6, R19) ──────────────────
  // The panel mounts only while the overlay is open, so a request made first
  // is read here on mount; it waits until the panel knows what this client
  // may do (the grant probe, the config read), then is taken once. A request
  // left over when the panel goes away is dropped, not run at the next open.
  const settled = $derived(hubUnavailable !== null || (hubOverview ? hubAdmin === 'granted' || hubAdmin === 'denied' : configRead));
  // The workspace is on screen: the full one, or the hub's read-only overview.
  const workspaceShown = $derived(catalogBlocked ? hubOverview : !!$catalogConfig);
  $effect(() => {
    const r = $assetsViewRequest;
    if (!r || !settled) return;
    untrack(() => {
      assetsViewRequest.set(null);
      void runAssetsRequest(r);
    });
  });
  onDestroy(() => assetsViewRequest.set(null));

  async function runAssetsRequest(r: AssetsViewRequest) {
    // An open sync plan covers the list; it yields to what was asked for,
    // except mid-apply.
    const clearPlan = () => { if (busy !== 'apply') syncPlan = null; };
    if (r.select && workspaceShown) {
      clearPlan();
      view = 'library';
      selectedKey = r.select;
    }
    // Rescan, Sync and Propose change things: a client without the grant (the
    // read-only overview) or without a catalog, or a panel or a card verb
    // that is busy, does nothing.
    if (!r.command || catalogBlocked || !$catalogConfig || busy !== '' || cardBusy !== '') return;
    if (r.command === 'rescan') {
      void scan();
    } else if (r.command === 'sync') {
      void requestSync({});
    } else {
      // The hub's propose needs the personal grant (final review minor 6).
      const ctx = { readOnly: catalogBlocked, remote: $hubStatus.remote, clientName: $hubStatus.client_name, statuses: $catalogStatuses };
      if (!canWrite(PERSONAL, ctx)) return;
      clearPlan();
      await proposeCards();
    }
  }

  /** "Propose cards": the hub derives its cards from the hosts as they are. */
  async function proposeCards() {
    busy = 'propose';
    let n;
    try {
      n = await proposeAndReload();
    } finally {
      busy = '';
    }
    if (n === null) return;
    view = 'inbox';
    push({ kind: 'info', message: `Proposed: ${n} open cards` });
  }

  // Tab-focus reload: a session delegated via "Open in session" edits the
  // catalog repo directly, outside any authoring command, so nothing else
  // tells this panel to refresh. When the Assets tab regains focus after
  // one was opened, reload once and clear the flag.
  let wasVisible = untrack(() => visible);
  $effect(() => {
    if (visible && !wasVisible && authorSessionOpened) {
      clearAuthorSessionOpened();
      void reload(false);
    }
    wasVisible = visible;
  });

  async function setup() {
    busy = 'setup'; error = null;
    const c = await configureCatalog(setupPath, setupRemote);
    if (!c.ok) { error = c.error.message; busy = ''; return; }
    await reload(false);
    busy = '';
  }

  // The personal chip's Pull. A failed pull leaves the listing as it was,
  // so its reason (git's own stderr) goes on the error line — the
  // failed-load block shows only while there is no listing at all.
  async function pull() {
    busy = 'pull';
    const e = await reload(true);
    busy = '';
    if (e) error = withGitStderr(e);
  }

  async function scan() {
    busy = 'scan'; error = null; scanResults = null;
    const r = await scanHosts();
    if (!r.ok) error = r.error.message; else { scanResults = r.value; await refresh(); }
    busy = '';
  }

  // The identity clicked drives which host/asset the dialog opens preset to:
  // `local` if the identity is on `local` (even alongside other hosts —
  // reading this machine needs no SSH hop), else its first host; `only`
  // limits the import to just this one asset rather than everything on
  // that host.
  function onImportUnmanaged(identity: AssetIdentity) {
    if (importBlocked) { error = importBlocked; return; }
    const host = identity.hosts.some((h) => h.host_alias === 'local') ? 'local' : (identity.hosts[0]?.host_alias ?? 'local');
    importPreset = { host, only: [`${identity.kind}:${identity.name}`] };
    showImport = true;
  }

  // The workspace's Import: the Library's button (no identity — the dialog's
  // own defaults) or an identity row, its Inspector, or the `a` key.
  function importFrom(identity: AssetIdentity | null) {
    if (identity) return onImportUnmanaged(identity);
    if (importBlocked) { error = importBlocked; return; }
    importPreset = null;
    showImport = true;
  }

  function onAssetCreated(kind: AssetKind, name: string) {
    showNewAsset = false;
    void openEditing(keyOf({ type: 'asset', catalog: PERSONAL, kind, name }));
    void afterWrite();
  }

  function onAssetDeleted() {
    selectedKey = null;
    void afterWrite();
  }

  // `E_CATALOG_GIT` from the shared `git()` helper (repo.rs) carries git's
  // own stderr in `error.details.stderr`, but the fixed message
  // (`git <args>: failed`) says nothing about *why* — no upstream, an auth
  // failure, non-fast-forward, all look identical without it. Append it,
  // truncated to a sane length in case git dumped something huge.
  const MAX_GIT_STDERR = 500;
  function withGitStderr(err: { message: string; details?: unknown }): string {
    const details = err.details as { stderr?: unknown } | undefined;
    const stderr = typeof details?.stderr === 'string' ? details.stderr.trim() : '';
    if (!stderr) return err.message;
    const truncated = stderr.length > MAX_GIT_STDERR ? `${stderr.slice(0, MAX_GIT_STDERR)}…` : stderr;
    return `${err.message}: ${truncated}`;
  }

  async function submitCommit(message: string, push = false) {
    showCommitPrompt = false;
    busy = 'commit'; error = null;
    const r = await commitPending(message);
    busy = '';
    if (!r.ok) { error = withGitStderr(r.error); return; }
    await afterWrite();
    if (push) await doPush();
  }

  async function doPush() {
    busy = 'push'; error = null;
    const r = await pushCatalog();
    busy = '';
    if (!r.ok) { error = withGitStderr(r.error); return; }
    await refresh();
  }

  function onLintAllSelect(kind: string, name: string) {
    showLintAll = false;
    selectedKey = keyOf({ type: 'asset', catalog: PERSONAL, kind, name });
  }

  async function requestSync(filter: { hostAlias?: string; kind?: string; name?: string }) {
    busy = 'plan'; error = null;
    syncFilter = filter;
    const r = await planSync(filter);
    busy = '';
    if (!r.ok) { error = r.error.message; return; }
    syncPlan = r.value;
    lastPlan = r.value;
  }

  function onSyncReplanned(p: SyncPlan) {
    syncPlan = p;
    lastPlan = p;
  }

  function onSyncApplied(summary: SyncRunSummary) {
    // Keep the plan view open: it renders the per-action outcome badges and
    // the "restart Claude on <host>" strip from this same `summary`, and it
    // now disables its own Apply button once `summary` is set. The user
    // dismisses it with Back.
    lastSyncRun.set(summary);
    void refresh();
  }

  const secretNames = $derived(
    lastPlan
      ? Array.from(
          new Set(lastPlan.hosts.flatMap((h) => h.actions.flatMap((a) => [...a.secrets, ...a.missing_secrets]))),
        ).sort()
      : [],
  );
</script>

<div class="assets-panel">
  {#if catalogBlocked}
    {#if hubOverview && hubAdmin !== 'denied'}
      <div class="setup" data-testid="assets-remote-probing">
        <h3>Asset catalog</h3>
        <p class="muted">Asking the hub…</p>
      </div>
    {:else if hubOverview}
      {#snippet hubFailed()}
        <div class="load-failed pad" data-testid="assets-hub-failed">
          {#if overviewNotConfigured}
            <p class="muted">The hub has no asset catalog yet. It is a git checkout on the hub's machine, set there — then Refresh here:</p>
            <pre class="cmd" data-testid="assets-hub-setup-cmd">fleet-hub catalog set ~/agent-assets --remote git@github.com:you/agent-assets.git</pre>
            <p class="muted">The remote is cloned when the path is empty. In the Docker setup, prefix it with <code>docker compose exec fleet-hub</code> and keep the checkout on the data volume, e.g. <code>/var/lib/fleet-hub/agent-assets</code>.</p>
          {:else}
            <p class="muted">The hub's asset catalog could not be loaded.</p>
            {#if overviewError}<p class="error">{overviewError}</p>{/if}
          {/if}
        </div>
      {/snippet}
      {#if hubAdminError}<p class="error" data-testid="assets-grant-error">{hubAdminError}</p>{/if}
      <AssetsWorkspace
        readOnly
        readOnlyClient={$hubStatus.client_name}
        {visible}
        {busy}
        {error}
        {scanResults}
        loading={overviewLoad === 'loading'}
        scanDisabled={overviewNotConfigured}
        failed={overviewLoad === 'failed' ? hubFailed : undefined}
        bind:selectedKey
        bind:view
        bind:cardBusy
        onscan={scanOnHub}
        onsync={() => {}}
        onimport={() => {}}
        onrefresh={() => void loadOverview()}
      />
    {:else}
      <div class="setup" data-testid="assets-remote">
        <h3>Asset catalog</h3>
        <p class="muted">{hubUnavailable}</p>
      </div>
    {/if}
  {:else if !$catalogConfig}
    <div class="setup" data-testid="assets-setup">
      <h3>Asset catalog</h3>
      <p class="muted">Point fleet at a git repo of skills, agents, hooks, MCP servers and plugin refs. A remote URL is cloned into the path when the path is empty.{#if hubOverview} The path is on the hub's machine{$hubStatus.url ? ` (${$hubStatus.url})` : ''}, and the clone uses its git credentials.{/if}</p>
      <label>{hubOverview ? "Path on the hub's machine" : 'Local path'} <input bind:value={setupPath} data-testid="assets-setup-path" /></label>
      <label>Remote URL (optional) <input bind:value={setupRemote} placeholder="git@github.com:you/agent-assets.git" /></label>
      {#if error}<p class="error">{error}</p>{/if}
      <button class="primary" onclick={setup} disabled={busy !== ''} data-testid="assets-setup-submit">{busy === 'setup' ? 'Setting up…' : 'Use this catalog'}</button>
    </div>
  {:else}
    {#snippet loadFailed()}
      <!-- `catalog` is only ever set on success; this keys off the load's
           OWN state, not the shared per-action `error`. -->
      <div class="load-failed pad" data-testid="assets-load-failed">
        <p class="muted">The asset catalog could not be loaded.</p>
        {#if catalogLoadError}<p class="error" data-testid="assets-load-error">{catalogLoadError}</p>{/if}
        <button class="btn" onclick={() => void reload(false)} data-testid="assets-retry">Retry</button>
      </div>
    {/snippet}
    <AssetsWorkspace
      {visible}
      {busy}
      {error}
      {scanResults}
      loading={catalogLoad === 'loading'}
      {importBlocked}
      failed={!$catalog && catalogLoad === 'failed' ? loadFailed : undefined}
      bind:selectedKey
      bind:view
      bind:cardBusy
      autoEditKey={pendingAutoEdit}
      plan={syncPlan}
      planFilter={syncFilter}
      onplanclose={() => (syncPlan = null)}
      onapplied={onSyncApplied}
      onapplying={(a) => (busy = a ? 'apply' : '')}
      onreplanned={onSyncReplanned}
      onscan={scan}
      onsync={requestSync}
      onimport={importFrom}
      onsecrets={() => (showSecrets = true)}
      onnew={() => (showNewAsset = true)}
      onlintall={() => (showLintAll = true)}
      onpull={pull}
      oncommit={() => (showCommitPrompt = true)}
      onpush={doPush}
      ondeleted={onAssetDeleted}
    />
  {/if}
  {#if showImport}
    <ImportDialog
      host={importPreset?.host ?? 'local'}
      only={importPreset?.only ?? []}
      onclose={() => { showImport = false; importPreset = null; }}
      ondone={() => { showImport = false; importPreset = null; reload(false); void repoStatus(); }}
    />
  {/if}
  {#if showSecrets}
    <SecretsPanel names={secretNames} onclose={() => (showSecrets = false)} />
  {/if}
  {#if showNewAsset}
    <NewAssetDialog
      onclose={() => (showNewAsset = false)}
      onsaved={onAssetCreated}
      onwrite={authorNewBlocked === null
        ? (instructions) => {
            showNewAsset = false;
            authorNew = instructions;
          }
        : undefined}
    />
  {/if}
  {#if authorNew !== null}
    <AuthorSessionDialog instructions={authorNew} onclose={() => (authorNew = null)} />
  {/if}
  {#if showLintAll}
    <LintAllDialog onclose={() => (showLintAll = false)} onselect={onLintAllSelect} />
  {/if}
  {#if showCommitPrompt}
    <CommitAssetsDialog
      canPush={!!$catalogConfig?.remote_url || !!$repoStatusStore?.has_upstream}
      oncommit={(message, push) => void submitCommit(message, push)}
      oncancel={() => (showCommitPrompt = false)}
    />
  {/if}
</div>

<style>
  .load-failed {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.5rem;
    padding: 0.5rem 0;
  }
  .assets-panel { display: flex; flex-direction: column; height: 100%; }
  .setup { max-width: 480px; margin: 40px auto; display: flex; flex-direction: column; gap: 10px; }
  .setup label { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-xs); }
  .cmd { margin: 0; padding: 6px 8px; font-family: var(--font-mono); font-size: var(--text-xs); background: var(--bg-pane); border-radius: var(--radius-sm); white-space: pre-wrap; word-break: break-all; user-select: text; }
  .muted { color: var(--fg-muted); } .error { color: var(--usage-crit); padding: 4px 10px; margin: 0; }
  .pad { padding: 14px; }
  .primary { background: var(--accent); color: var(--accent-fg); border: 0; border-radius: var(--radius-sm); padding: 6px 10px; }
</style>
