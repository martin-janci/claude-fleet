<script lang="ts">
  import { untrack } from 'svelte';
  import { getAsset, deleteAsset, lintAsset, type AssetDetail, type LintReport, type WriteResult } from './assets';
  import type { HostRow } from './hosts';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import AssetEditor from './AssetEditor.svelte';
  import AuthorSessionDialog from './AuthorSessionDialog.svelte';

  let {
    kind,
    name,
    hosts,
    onsync,
    ondeleted,
    startInEdit = false,
  }: {
    kind: string;
    name: string;
    hosts: HostRow[];
    /** Requests a sync plan scoped to this asset (optionally to one host).
     *  The caller (AssetsPanel) computes the plan and owns the dialog, the
     *  same way `AssetList`'s `onimport` bubbles up to `ImportDialog`. */
    onsync?: (filter: { hostAlias?: string; kind?: string; name?: string }) => void;
    /** Fires once the asset is deleted, so the caller can clear its
     *  selection (this component has nothing left to show). */
    ondeleted?: () => void;
    /** Open straight into edit mode (a just-created asset). The caller keys
     *  this component by kind+name so a fresh instance — and a fresh read
     *  of this prop — is created per selection. */
    startInEdit?: boolean;
  } = $props();

  let detail = $state<AssetDetail | null>(null);
  let error = $state<string | null>(null);
  let harnessTab = $state('claude');
  // Bumped after any write this component makes (a save, a cancel that may
  // have already committed resource changes, a delete's sibling ops) to
  // force the fetch effect below to re-run even though kind/name did not
  // change.
  let reloadKey = $state(0);

  $effect(() => {
    const k = kind, n = name;
    void reloadKey;
    detail = null; error = null;
    getAsset(k, n).then((r) => {
      if (k !== kind || n !== name) return;
      if (r.ok) detail = r.value; else error = r.error.message;
    });
  });

  // ── Edit ─────────────────────────────────────────────────────────────
  let editing = $state(untrack(() => startInEdit));
  let lastCommit = $state<string | null>(null);

  function onSaved(result: WriteResult) {
    lastCommit = result.commit;
    editing = false;
    reloadKey += 1;
  }
  function onEditCancel() {
    editing = false;
    // A resource add/remove inside the editor commits on its own even
    // though the header/body edit was never saved — refetch either way so
    // a cancelled edit never leaves a stale resources list on screen.
    reloadKey += 1;
  }

  // ── Delete ───────────────────────────────────────────────────────────
  let showDeleteConfirm = $state(false);
  let deleting = $state(false);
  let deleteError = $state<string | null>(null);

  async function confirmDelete() {
    if (!detail) return;
    deleting = true;
    deleteError = null;
    const r = await deleteAsset(detail.asset.kind, detail.asset.name);
    deleting = false;
    if (!r.ok) {
      deleteError = r.error.message;
      return;
    }
    showDeleteConfirm = false;
    ondeleted?.();
  }

  // ── Lint ─────────────────────────────────────────────────────────────
  let showLint = $state(false);
  let lintBusy = $state(false);
  let lintError = $state<string | null>(null);
  let lintReport = $state<LintReport | null>(null);

  async function toggleLint() {
    if (showLint) {
      showLint = false;
      return;
    }
    showLint = true;
    if (!detail) return;
    lintBusy = true;
    lintError = null;
    const r = await lintAsset(detail.asset.kind, detail.asset.name);
    lintBusy = false;
    if (!r.ok) {
      lintError = r.error.message;
      return;
    }
    lintReport = r.value;
  }

  // ── Open in session ──────────────────────────────────────────────────
  let showAuthorDialog = $state(false);

  const harnesses = $derived(detail ? detail.previews.map((p) => p.harness) : []);
  const preview = $derived(detail?.previews.find((p) => p.harness === harnessTab) ?? null);
  const visibleHosts = $derived(hosts.filter((h) => !h.hidden));

  // Raw state (`in_sync`, `drifted`, `missing`, `orphan`, `unsupported`, or
  // `null` for "skipped"/"not scanned") — used to decide when a cell gets a
  // Sync link, before `cell()` below turns it into display text.
  function rawState(hostAlias: string, harness: string): string | null {
    const host = visibleHosts.find((h) => h.alias === hostAlias);
    if (host && host.alias !== 'local' && !host.reachable) return null;
    return detail?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness)?.state ?? null;
  }

  function cell(hostAlias: string, harness: string): string {
    const host = visibleHosts.find((h) => h.alias === hostAlias);
    if (host && host.alias !== 'local' && !host.reachable) return 'skipped';
    const s = rawState(hostAlias, harness);
    return s ? s.replace('_', ' ') : 'not scanned';
  }

  // `orphan` deliberately excluded: it only ever appears on an unmanaged
  // inventory row (AssetsPanel's "On hosts, not in catalog" list), never in
  // a catalog asset's own `hosts` — the array this component reads — so it
  // can never reach `rawState()` here.
  const SYNCABLE_STATES = new Set(['missing', 'drifted']);

  function requestSync(hostAlias: string) {
    if (!detail) return;
    onsync?.({ hostAlias, kind: detail.asset.kind, name: detail.asset.name });
  }
</script>

<div class="detail">
  {#if error}
    <p class="error">{error}</p>
  {:else if !detail}
    <p class="muted">Loading…</p>
  {:else}
    <div class="title-row">
      <h3 data-testid="asset-detail-title"><span class="kind">{detail.asset.kind}</span> {detail.asset.name} <span class="ver">v{detail.asset.version}</span></h3>
      <div class="title-actions">
        {#if lastCommit}<span class="commit" data-testid="asset-last-commit">saved {lastCommit.slice(0, 7)}</span>{/if}
        <button
          class="sync-btn"
          onclick={() => onsync?.({ kind: detail?.asset.kind, name: detail?.asset.name })}
          data-testid="asset-sync"
        >Sync this asset</button>
        <button class="sync-btn" onclick={() => (showAuthorDialog = true)} data-testid="asset-open-session">Open in session</button>
        <button class="sync-btn" onclick={toggleLint} data-testid="asset-lint">{lintBusy ? 'Linting…' : 'Lint'}</button>
        {#if !editing}
          <button class="sync-btn" onclick={() => (editing = true)} data-testid="asset-edit">Edit</button>
        {/if}
        <button class="sync-btn danger" onclick={() => (showDeleteConfirm = true)} data-testid="asset-delete">Delete</button>
      </div>
    </div>
    {#if showLint}
      <div class="lint-report" data-testid="asset-lint-report">
        {#if lintBusy}
          <p class="muted">Linting…</p>
        {:else if lintError}
          <p class="error">{lintError}</p>
        {:else if lintReport}
          <p class="lint-summary">{lintReport.errors.length} errors, {lintReport.warnings.length} warnings</p>
          {#each lintReport.errors as f, i (i)}<p class="lint-error" data-testid={`asset-lint-error-${i}`}>{f.field}: {f.message}</p>{/each}
          {#each lintReport.warnings as f, i (i)}<p class="lint-warn" data-testid={`asset-lint-warning-${i}`}>{f.field}: {f.message}</p>{/each}
        {/if}
      </div>
    {/if}
    <p class="desc">{detail.asset.description}</p>
    {#if detail.asset.tags?.length}<p class="tags">{#each detail.asset.tags ?? [] as t}<span class="tag">{t}</span>{/each}</p>{/if}

    <h4>Hosts</h4>
    <table class="matrix">
      <thead><tr><th>host</th>{#each harnesses as h}<th>{h}</th>{/each}</tr></thead>
      <tbody>
        {#each visibleHosts as host (host.alias)}
          <tr>
            <td>{host.alias}</td>
            {#each harnesses as h}
              {@const s = cell(host.alias, h)}
              {@const raw = rawState(host.alias, h)}
              <td class={`state-${s.replace(' ', '-')}`} data-testid={`matrix-cell-${host.alias}-${h}`} title={s === 'skipped' ? 'host unreachable' : ''}>
                {s}
                {#if raw && SYNCABLE_STATES.has(raw)}
                  <button class="cell-sync" onclick={() => requestSync(host.alias)} data-testid={`cell-sync-${host.alias}-${h}`}>Sync</button>
                {/if}
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>

    {#if editing}
      <h4>Edit</h4>
      <AssetEditor asset={detail.asset} onsaved={onSaved} oncancel={onEditCancel} />
    {:else}
      <h4>Preview</h4>
      <div class="tabs" role="tablist">
        {#each harnesses as h}
          <button role="tab" class:active={harnessTab === h} aria-selected={harnessTab === h} onclick={() => (harnessTab = h)} data-testid={`preview-tab-${h}`}>{h}</button>
        {/each}
      </div>
      {#if preview?.unsupported}
        <p class="muted">{preview.unsupported}</p>
      {:else if preview?.plan}
        {#each preview.plan.warnings as w}<p class="warn">{w}</p>{/each}
        {#if preview.plan.placeholders.length}<p class="warn">Unresolved placeholders: {preview.plan.placeholders.join(', ')}</p>{/if}
        {#each preview.plan.files as f (f.path)}
          <div class="file">
            <div class="path" data-testid="preview-file-path">{f.path}</div>
            <pre>{f.bytes}</pre>
          </div>
        {/each}
        {#each preview.plan.merges as m (m.file + m.json_path.join('/'))}
          <div class="file">
            <div class="path">{m.file} → {m.json_path.join('.')} <span class="mode">({m.mode})</span></div>
            <pre>{JSON.stringify(m.value, null, 2)}</pre>
          </div>
        {/each}
        {#if preview.plan.files.length === 0 && preview.plan.merges.length === 0}<p class="muted">Nothing to install.</p>{/if}
      {/if}
    {/if}
  {/if}
</div>

{#if showDeleteConfirm && detail}
  <ConfirmDialog
    title="Delete asset?"
    confirmLabel="Delete"
    danger
    busy={deleting}
    onconfirm={confirmDelete}
    oncancel={() => (showDeleteConfirm = false)}
    confirmTestId="asset-delete-confirm"
  >
    This deletes <code>{detail.asset.kind}/{detail.asset.name}</code> from the catalog repo and commits the removal.
    Hosts that had it become orphaned until the next sync.
    {#if deleteError}<br /><span class="error">{deleteError}</span>{/if}
  </ConfirmDialog>
{/if}

{#if showAuthorDialog && detail}
  <AuthorSessionDialog kind={detail.asset.kind} name={detail.asset.name} onclose={() => (showAuthorDialog = false)} />
{/if}

<style>
  .detail { padding: 10px 14px; overflow: auto; height: 100%; font-size: 13px; }
  .title-row { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; flex-wrap: wrap; }
  .title-actions { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
  .commit { font-size: 11px; color: #16a34a; }
  .sync-btn { font-size: 11px; padding: 2px 8px; border: 1px solid var(--border); border-radius: 4px; background: transparent; color: var(--fg); cursor: pointer; }
  .sync-btn.danger { color: #e64a4a; border-color: #e64a4a; }
  .lint-report { margin: 6px 0; padding: 6px 8px; border: 1px solid var(--border); border-radius: 4px; }
  .lint-summary { margin: 0 0 4px; font-weight: 600; }
  .lint-error { color: #dc2626; margin: 2px 0; font-size: 12px; }
  .lint-warn { color: #d97706; margin: 2px 0; font-size: 12px; }
  .cell-sync { margin-left: 6px; font-size: 10px; padding: 0 4px; border: 1px solid var(--border); border-radius: 8px; background: transparent; color: var(--accent); cursor: pointer; }
  h3 { margin: 0 0 4px; font-size: 15px; font-family: ui-monospace, monospace; }
  .kind, .ver { color: var(--fg-muted); font-size: 11px; font-family: system-ui; }
  h4 { margin: 14px 0 6px; font-size: 11px; text-transform: uppercase; color: var(--fg-muted); }
  .desc { margin: 0; } .tags { margin: 4px 0 0; } .tag { border: 1px solid var(--border); border-radius: 8px; padding: 0 6px; font-size: 11px; margin-right: 4px; }
  .matrix { border-collapse: collapse; } .matrix th, .matrix td { text-align: left; padding: 3px 10px 3px 0; border-bottom: 1px solid var(--border); }
  .state-in-sync { color: #16a34a; } .state-drifted { color: #d97706; } .state-skipped, .state-not-scanned, .state-missing, .state-unsupported, .state-orphan { color: var(--fg-muted); }
  .tabs { display: flex; gap: 2px; margin-bottom: 6px; } .tabs button { background: none; border: 1px solid var(--border); border-radius: 4px; padding: 2px 8px; color: var(--fg-muted); cursor: pointer; } .tabs button.active { color: var(--fg); border-color: var(--accent); }
  .file { margin: 6px 0; } .path { font-family: ui-monospace, monospace; font-size: 12px; color: var(--fg-muted); } .mode { opacity: 0.7; }
  pre { margin: 2px 0 0; padding: 8px; background: var(--bg-pane); border: 1px solid var(--border); border-radius: 4px; overflow: auto; max-height: 320px; font-size: 12px; }
  .muted { color: var(--fg-muted); } .warn { color: #d97706; margin: 2px 0; } .error { color: #dc2626; }
</style>
