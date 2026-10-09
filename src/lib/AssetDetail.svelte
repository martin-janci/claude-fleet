<script lang="ts">
  import { tablistKeys } from './tablist_keys';
  import Loader from './Loader.svelte';
  import { untrack } from 'svelte';
  import { getAsset, deleteAsset, lintAsset, type AssetDetail, type LintReport, type WriteResult } from './assets';
  import type { HostRow } from './hosts';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import AssetEditor from './AssetEditor.svelte';
  import AuthorSessionDialog from './AuthorSessionDialog.svelte';
  import { hubStatus, hubBlock } from './hub';
  import { behindWords, driftSideWords } from './assets_inbox';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';

  let {
    kind,
    name,
    hosts,
    onsync,
    ondeleted,
    startInEdit = false,
    /** Assets M5 (R19): which part the Inspector shows — `overview` (title,
     *  actions, lint, description, tags), `hosts` (the host × harness
     *  matrix), `source` (the editor or the rendered preview). `all` (the
     *  default) is the whole detail, as before. Switching it never refetches. */
    section = 'all',
    /** Edit lives in Source: ask the Inspector to show it. */
    onsection,
    /** Whether the editor is open, on mount and on every change (final
     *  review I2: the Inspector's `e` must not re-create an open editor). */
    onediting,
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
    section?: 'all' | 'overview' | 'hosts' | 'source';
    onsection?: (s: 'source') => void;
    onediting?: (editing: boolean) => void;
  } = $props();

  const show = (s: 'overview' | 'hosts' | 'source') => section === 'all' || section === s;

  let detail = $state<AssetDetail | null>(null);
  let error = $state<string | null>(null);
  let harnessTab = $state('claude');
  // Bumped after any write this component makes (a save, a cancel that may
  // have already committed resource changes, a delete's sibling ops) to
  // force the fetch effect below to re-run even though kind/name did not
  // change.
  let reloadKey = $state(0);

  // Only a real change of asset (or a write) refetches — not a re-render of
  // the same props (a `$derived` is equality-checked; the Inspector flips
  // `section` per tab without ever reading the asset again).
  const target = $derived(JSON.stringify([kind, name]));
  $effect(() => {
    const [k, n] = JSON.parse(target) as [string, string];
    void reloadKey;
    detail = null; error = null;
    getAsset(k, n).then((r) => {
      if (k !== kind || n !== name) return;
      if (r.ok) detail = r.value; else error = r.error.message;
    });
  });

  // ── Edit ─────────────────────────────────────────────────────────────
  let editing = $state(untrack(() => startInEdit));
  $effect.pre(() => {
    const on = editing;
    untrack(() => onediting?.(on));
  });
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
  // On a hub the checkout is on the hub's machine, where this window cannot
  // start a session (`catalog_spawn_author_session` is local-only).
  const authorBlocked = $derived(hubBlock('catalog_spawn_author_session', $hubStatus));

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

  /** Which side moved, for a drifted cell (final review, minor 2). */
  function side(hostAlias: string, harness: string): string | null {
    if (rawState(hostAlias, harness) !== 'drifted') return null;
    return driftSideWords(detail?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness)?.drift_side);
  }

  /** The cell's hover words: why it is skipped, or which side moved — and
   *  for a copy only behind the catalog, what will happen to it. */
  function cellTitle(hostAlias: string, harness: string, moved: string | null): string {
    if (cell(hostAlias, harness) === 'skipped') return 'host unreachable';
    if (rawState(hostAlias, harness) === 'drifted' && detail?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness)?.drift_side === 'catalog') {
      return behindWords(settingBool($fleetSettings, SETTING_KEYS.catalogAuto));
    }
    return moved ?? '';
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
    {#if show('overview')}
    <div class="title-row">
      <h3 data-testid="asset-detail-title"><span class="kind">{detail.asset.kind}</span> {detail.asset.name} <span class="ver">v{detail.asset.version}</span></h3>
      <div class="title-actions">
        {#if lastCommit}<span class="commit" data-testid="asset-last-commit">saved {lastCommit.slice(0, 7)}</span>{/if}
        <button
          class="sync-btn"
          onclick={() => onsync?.({ kind: detail?.asset.kind, name: detail?.asset.name })}
          data-testid="asset-sync"
        >Sync this asset</button>
        <button class="sync-btn" onclick={() => (showAuthorDialog = true)} disabled={authorBlocked !== null} title={authorBlocked ?? ''} data-testid="asset-open-session">Open in session…</button>
        <button class="sync-btn" onclick={toggleLint} data-testid="asset-lint">{#if lintBusy}<Loader name="comet" size={12} class="btn-loader" />{/if}{lintBusy ? 'Linting…' : 'Lint'}</button>
        {#if !editing}
          <button class="sync-btn" onclick={() => { editing = true; onsection?.('source'); }} data-testid="asset-edit">Edit</button>
        {/if}
        <button class="sync-btn danger" onclick={() => (showDeleteConfirm = true)} data-testid="asset-delete">Delete…</button>
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
    {#if detail.asset.install_as}<p class="install-as" data-testid="asset-install-as">installs as <code>{detail.asset.install_as}</code></p>{/if}
    {#if detail.asset.tags?.length}<p class="tags">{#each detail.asset.tags ?? [] as t}<span class="tag">{t}</span>{/each}</p>{/if}

    {/if}

    {#if show('hosts')}
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
              {@const moved = side(host.alias, h)}
              <td class={`state-${s.replace(' ', '-')}`} data-testid={`matrix-cell-${host.alias}-${h}`} title={cellTitle(host.alias, h, moved)}>
                {s}{#if moved}<span class="side">{` — ${moved}`}</span>{/if}
                {#if raw && SYNCABLE_STATES.has(raw)}
                  <button class="cell-sync" onclick={() => requestSync(host.alias)} data-testid={`cell-sync-${host.alias}-${h}`}>Sync</button>
                {/if}
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
    {/if}

    <!-- The editor stays mounted for as long as `editing` (hidden, not
         unmounted, under another section) so an unsaved draft survives a
         visit to Overview or Hosts. -->
    {#if editing}
      <div class="edit-pane" hidden={!show('source')}>
        <h4>Edit</h4>
        <AssetEditor asset={detail.asset} onsaved={onSaved} oncancel={onEditCancel} />
      </div>
    {:else if show('source')}
      <h4>Preview</h4>
      <div class="tabs" role="tablist" aria-label="Harness preview" use:tablistKeys>
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
  /* No scroller of its own: it lives in the Inspector's tab panel, which scrolls. */
  .detail { padding: 10px 14px; font-size: var(--text-sm); }
  .title-row { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-2); flex-wrap: wrap; }
  .title-actions { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
  .commit { font-size: var(--text-2xs); color: var(--usage-ok); }
  .sync-btn { font-size: var(--text-2xs); padding: 2px var(--space-2); border: 1px solid var(--border); border-radius: var(--radius-sm); background: transparent; color: var(--fg); cursor: pointer; }
  .sync-btn.danger { color: var(--usage-crit); border-color: var(--usage-crit); }
  .lint-report { margin: 6px 0; padding: 6px var(--space-2); border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .lint-summary { margin: 0 0 var(--space-1); font-weight: 600; }
  .lint-error { color: var(--usage-crit); margin: 2px 0; font-size: var(--text-xs); }
  .lint-warn { color: var(--usage-warn); margin: 2px 0; font-size: var(--text-xs); }
  .cell-sync { margin-left: 6px; font-size: var(--text-2xs); padding: 0 var(--space-1); border: 1px solid var(--border); border-radius: var(--radius-lg); background: transparent; color: var(--accent); cursor: pointer; }
  h3 { margin: 0 0 var(--space-1); font-size: var(--text-md); font-family: var(--font-mono); }
  .kind, .ver { color: var(--fg-muted); font-size: var(--text-2xs); font-family: var(--font-sans); }
  h4 { margin: 14px 0 6px; font-size: var(--text-2xs); text-transform: uppercase; color: var(--fg-muted); }
  .desc { margin: 0; } .install-as { margin: var(--space-1) 0 0; font-size: var(--text-2xs); color: var(--fg-muted); } .tags { margin: var(--space-1) 0 0; } .tag { border: 1px solid var(--border); border-radius: var(--radius-lg); padding: 0 6px; font-size: var(--text-2xs); margin-right: var(--space-1); }
  .matrix { border-collapse: collapse; } .matrix th, .matrix td { text-align: left; padding: 3px 10px 3px 0; border-bottom: 1px solid var(--border); }
  .state-in-sync { color: var(--usage-ok); } .state-drifted { color: var(--usage-warn); } .state-skipped, .state-not-scanned, .state-missing, .state-unsupported, .state-orphan { color: var(--fg-muted); }
  .tabs { display: flex; gap: 2px; margin-bottom: 6px; } .tabs button { background: none; border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 2px var(--space-2); color: var(--fg-muted); cursor: pointer; } .tabs button.active { color: var(--fg); border-color: var(--accent); }
  .file { margin: 6px 0; } .path { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--fg-muted); } .mode { opacity: 0.7; }
  pre { margin: 2px 0 0; padding: var(--space-2); background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: auto; max-height: 320px; font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); } .warn { color: var(--usage-warn); margin: 2px 0; } .error { color: var(--usage-crit); }
</style>
