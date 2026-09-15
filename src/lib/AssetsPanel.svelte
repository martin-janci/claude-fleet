<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import {
    catalog, catalogConfig, loadCatalogConfig, configureCatalog, loadCatalog, loadAssets, loadInventory, scanHosts,
    planSync, lastSync, lastSyncRun,
    commitPending, pushCatalog, repoStatus, repoStatusStore,
    type HostScanResult, type AssetInventoryRow, type SyncPlan, type SyncRunSummary, type AssetKind,
  } from './assets';
  import { hosts } from './hosts';
  import AssetList from './AssetList.svelte';
  import AssetDetail from './AssetDetail.svelte';
  import ImportDialog from './ImportDialog.svelte';
  import SyncPlanDialog from './SyncPlanDialog.svelte';
  import SecretsPanel from './SecretsPanel.svelte';
  import NewAssetDialog from './NewAssetDialog.svelte';
  import LintAllDialog from './LintAllDialog.svelte';
  import PromptDialog from './PromptDialog.svelte';
  import { authorSessionOpened, clearAuthorSessionOpened } from './AuthorSessionDialog.svelte';

  let { visible }: { visible: boolean } = $props();

  let setupPath = $state('~/agent-assets');
  let setupRemote = $state('');
  let busy = $state<'' | 'setup' | 'pull' | 'scan' | 'plan' | 'apply' | 'commit' | 'push'>('');
  let error = $state<string | null>(null);
  let scanResults = $state<HostScanResult[] | null>(null);
  let showProblems = $state(false);
  let showImport = $state(false);
  let filter = $state('');
  let selected = $state<{ kind: string; name: string } | null>(null);
  let syncPlan = $state<SyncPlan | null>(null);
  // The most recent plan computed (kept after the dialog closes) so the
  // SecretsPanel can offer the names its blocked/missing-secret actions
  // named, without recomputing a plan just to open it.
  let lastPlan = $state<SyncPlan | null>(null);
  let showSecrets = $state(false);
  let showNewAsset = $state(false);
  let showLintAll = $state(false);
  let showCommitPrompt = $state(false);
  // Set (once) to `<kind>::<name>` right before selecting a just-created
  // asset, so the detail pane opens straight into edit mode for it. Never
  // cleared: a later re-selection of the same asset re-opening the editor
  // is a harmless edge case, not a bug worth the extra bookkeeping.
  let pendingAutoEdit = $state('');

  async function refresh() {
    const [a, i] = await Promise.all([loadAssets(), loadInventory()]);
    if (!a.ok) error = a.error.message;
    if (!i.ok) error = i.error.message;
  }

  // Shared by every path that needs to re-read the catalog: `pull: false`
  // just reloads the working tree as-is (used after import, and on initial
  // mount/setup); `pull: true` is only the explicit "Pull" button.
  async function reload(pull: boolean) {
    error = null;
    const l = await loadCatalog(pull);
    if (!l.ok) { error = l.error.message; return; }
    await refresh();
  }

  // Every authoring write (create/update/delete/resource/commit/push) goes
  // through this: reload the asset list and refresh the dirty/ahead/behind
  // status strip together, so the toolbar never shows a status stale
  // relative to what's in the list.
  async function afterWrite() {
    await Promise.all([refresh(), repoStatus()]);
  }

  onMount(async () => {
    const c = await loadCatalogConfig();
    if (c.ok && c.value) {
      await reload(false);
      void repoStatus();
    }
    void lastSync();
  });

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

  async function pull() {
    busy = 'pull';
    await reload(true);
    busy = '';
  }

  async function scan() {
    busy = 'scan'; error = null; scanResults = null;
    const r = await scanHosts();
    if (!r.ok) error = r.error.message; else { scanResults = r.value; await refresh(); }
    busy = '';
  }

  function onImportUnmanaged(_row: AssetInventoryRow) {
    showImport = true;
  }

  function onAssetCreated(kind: AssetKind, name: string) {
    showNewAsset = false;
    pendingAutoEdit = `${kind}::${name}`;
    selected = { kind, name };
    void afterWrite();
  }

  function onAssetDeleted() {
    selected = null;
    void afterWrite();
  }

  async function submitCommit(message: string) {
    showCommitPrompt = false;
    busy = 'commit'; error = null;
    const r = await commitPending(message);
    busy = '';
    if (!r.ok) { error = r.error.message; return; }
    await afterWrite();
  }

  async function doPush() {
    busy = 'push'; error = null;
    const r = await pushCatalog();
    busy = '';
    if (!r.ok) { error = r.error.message; return; }
    await refresh();
  }

  function onLintAllSelect(kind: string, name: string) {
    showLintAll = false;
    selected = { kind, name };
  }

  async function requestSync(filter: { hostAlias?: string; kind?: string; name?: string }) {
    busy = 'plan'; error = null;
    const r = await planSync(filter);
    busy = '';
    if (!r.ok) { error = r.error.message; return; }
    syncPlan = r.value;
    lastPlan = r.value;
  }

  function onSyncApplied(summary: SyncRunSummary) {
    // Keep the dialog mounted: it renders the per-action outcome badges and
    // the "restart Claude on <host>" strip from this same `summary`, and it
    // now disables its own Apply button and relabels Close to "Done" once
    // `summary` is set. The user dismisses it explicitly.
    lastSyncRun.set(summary);
    void refresh();
  }

  function summarizeRun(run: SyncRunSummary): string {
    const counts: Record<string, number> = {};
    for (const h of run.hosts) counts[h.status] = (counts[h.status] ?? 0) + 1;
    const parts = Object.entries(counts).map(([k, n]) => `${n} ${k}`);
    return `${new Date(run.finished_at * 1000).toLocaleString()} — ${parts.join(', ') || 'no hosts'}`;
  }

  const shortHead = $derived(($catalogConfig?.head_commit ?? '').slice(0, 7));
  const secretNames = $derived(
    lastPlan
      ? Array.from(
          new Set(lastPlan.hosts.flatMap((h) => h.actions.flatMap((a) => [...a.secrets, ...a.missing_secrets]))),
        ).sort()
      : [],
  );
</script>

<div class="assets-panel">
  {#if !$catalogConfig}
    <div class="setup" data-testid="assets-setup">
      <h3>Asset catalog</h3>
      <p class="muted">Point fleet at a git repo of skills, agents, hooks, MCP servers and plugin refs. A remote URL is cloned into the path when the path is empty.</p>
      <label>Local path <input bind:value={setupPath} data-testid="assets-setup-path" /></label>
      <label>Remote URL (optional) <input bind:value={setupRemote} placeholder="git@github.com:you/agent-assets.git" /></label>
      {#if error}<p class="error">{error}</p>{/if}
      <button class="primary" onclick={setup} disabled={busy !== ''} data-testid="assets-setup-submit">{busy === 'setup' ? 'Setting up…' : 'Use this catalog'}</button>
    </div>
  {:else}
    <div class="toolbar">
      <span class="path" title={$catalogConfig.repo_path}>{$catalogConfig.repo_path}</span>
      <span class="head" data-testid="assets-head">@ {shortHead || '—'}</span>
      <button onclick={pull} disabled={busy !== ''}>{busy === 'pull' ? 'Pulling…' : 'Pull'}</button>
      <button onclick={scan} disabled={busy !== ''} data-testid="assets-scan">{busy === 'scan' ? 'Scanning…' : 'Scan hosts'}</button>
      <button onclick={() => (showImport = true)} disabled={busy !== ''}>Import from host</button>
      <button onclick={() => requestSync({})} disabled={busy !== ''} data-testid="assets-sync">{busy === 'plan' ? 'Planning…' : 'Sync'}</button>
      <button onclick={() => (showSecrets = true)} disabled={busy !== ''} data-testid="assets-secrets">Secrets</button>
      <button onclick={() => (showNewAsset = true)} disabled={busy !== ''} data-testid="assets-new">New asset</button>
      <button onclick={() => (showLintAll = true)} disabled={busy !== ''} data-testid="assets-lint-all">Lint all</button>
      {#if $repoStatusStore && $repoStatusStore.dirty > 0}
        <button onclick={() => (showCommitPrompt = true)} disabled={busy !== ''} data-testid="assets-commit-pending">{busy === 'commit' ? 'Committing…' : 'Commit pending'}</button>
      {/if}
      <button
        onclick={doPush}
        disabled={busy !== '' || !$repoStatusStore?.has_upstream}
        data-testid="assets-push"
        title={$repoStatusStore?.has_upstream ? '' : 'no upstream configured'}
      >{busy === 'push' ? 'Pushing…' : `Push${$repoStatusStore?.ahead ? ` ↑${$repoStatusStore.ahead}` : ''}`}</button>
      {#if $lastSyncRun}
        <span class="last-sync" data-testid="assets-last-sync">{summarizeRun($lastSyncRun)}</span>
      {/if}
      {#if $catalog && $catalog.problems.length > 0}
        <button class="badge" onclick={() => (showProblems = !showProblems)} data-testid="assets-problems">{$catalog.problems.length} problems</button>
      {/if}
      <input class="filter" placeholder="filter" bind:value={filter} />
    </div>
    {#if $repoStatusStore}
      <p class="repo-status" data-testid="assets-repo-status">
        {$repoStatusStore.head.slice(0, 7)} · {$repoStatusStore.dirty} dirty
        {#if $repoStatusStore.has_upstream}· ↑{$repoStatusStore.ahead ?? 0} ↓{$repoStatusStore.behind ?? 0}{:else}· no upstream{/if}
      </p>
    {/if}
    {#if error}<p class="error">{error}</p>{/if}
    {#if scanResults}
      <p class="scan-result" data-testid="assets-scan-result">{scanResults.map((r) => `${r.host}: ${r.status}${r.detail ? ` (${r.detail})` : ''}`).join(' · ')}</p>
    {/if}
    {#if showProblems && $catalog}
      <ul class="problems">{#each $catalog.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>
    {/if}
    <div class="body">
      <div class="left">
        {#if $catalog}
          <AssetList listing={$catalog} {selected} {filter} onselect={(kind, name) => (selected = { kind, name })} onimport={onImportUnmanaged} />
        {:else}
          <p class="muted">Loading…</p>
        {/if}
      </div>
      <div class="right">
        {#if selected}
          {#key `${selected.kind}::${selected.name}`}
            <AssetDetail
              kind={selected.kind}
              name={selected.name}
              hosts={$hosts}
              onsync={requestSync}
              ondeleted={onAssetDeleted}
              startInEdit={pendingAutoEdit === `${selected.kind}::${selected.name}`}
            />
          {/key}
        {:else}
          <p class="muted empty">Select an asset.</p>
        {/if}
      </div>
    </div>
  {/if}
  {#if showImport}
    <ImportDialog onclose={() => (showImport = false)} ondone={() => { showImport = false; reload(false); }} />
  {/if}
  {#if syncPlan}
    <SyncPlanDialog
      plan={syncPlan}
      onclose={() => (syncPlan = null)}
      onapplied={onSyncApplied}
      onopensecrets={() => (showSecrets = true)}
      onapplying={(a) => (busy = a ? 'apply' : '')}
    />
  {/if}
  {#if showSecrets}
    <SecretsPanel names={secretNames} onclose={() => (showSecrets = false)} />
  {/if}
  {#if showNewAsset}
    <NewAssetDialog onclose={() => (showNewAsset = false)} onsaved={onAssetCreated} />
  {/if}
  {#if showLintAll}
    <LintAllDialog onclose={() => (showLintAll = false)} onselect={onLintAllSelect} />
  {/if}
  {#if showCommitPrompt}
    <PromptDialog
      title="Commit pending changes"
      label="Commit message"
      initialValue="catalog: commit pending changes"
      confirmLabel="Commit"
      onsubmit={submitCommit}
      oncancel={() => (showCommitPrompt = false)}
    />
  {/if}
</div>

<style>
  .assets-panel { display: flex; flex-direction: column; height: 100%; }
  .setup { max-width: 480px; margin: 40px auto; display: flex; flex-direction: column; gap: 10px; }
  .setup label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; }
  .toolbar { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid var(--border); font-size: 12px; }
  .path { color: var(--fg-muted); max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .head { font-family: ui-monospace, monospace; color: var(--fg-muted); }
  .badge { color: #d97706; }
  .last-sync { color: var(--fg-muted); font-size: 11px; white-space: nowrap; }
  .repo-status { font-size: 11px; color: var(--fg-muted); margin: 0; padding: 2px 10px; font-family: ui-monospace, monospace; }
  .filter { margin-left: auto; width: 160px; }
  .body { display: grid; grid-template-columns: 300px 1fr; flex: 1; min-height: 0; }
  .left { border-right: 1px solid var(--border); min-height: 0; overflow: auto; }
  .right { min-height: 0; overflow: auto; }
  .muted { color: var(--fg-muted); } .empty { padding: 14px; } .error { color: #dc2626; padding: 4px 10px; margin: 0; }
  .scan-result { font-size: 12px; padding: 4px 10px; margin: 0; color: var(--fg-muted); }
  .problems { font-size: 12px; margin: 0; padding: 4px 10px 4px 28px; }
  .primary { background: var(--accent); color: white; border: 0; border-radius: 4px; padding: 6px 10px; }
</style>
