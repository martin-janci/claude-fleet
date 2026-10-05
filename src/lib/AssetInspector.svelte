<script lang="ts">
  import { untrack } from 'svelte';
  import Inspector from './Inspector.svelte';
  import AssetDetail from './AssetDetail.svelte';
  import Badge from './Badge.svelte';
  import HostStrip from './HostStrip.svelte';
  import { identitiesOf, catalogOf, type AssetIdentity, type AssetListing, type AssetSummary } from './assets';
  import { assetDots } from './assets_inbox';
  import type { HostRow } from './hosts';
  import { ago, assetHistory, parseKey, scopeBadge, type ChangesetSummary, type CommitEntry } from './assets_workspace';
  import type { IpcError } from './result';

  /** What the Inspector shows for the selected row (Rulings R19): a pane of
   *  tabs. A personal asset this window may write gets the one
   *  `AssetDetail` (Overview, Source and Hosts are its sections) plus
   *  History; every other row gets a summary read from the listing — never a
   *  `catalog_get_asset`, which opens personal assets only. */
  let {
    selectedKey,
    listing,
    cards,
    hosts,
    order,
    readOnly,
    canOpen,
    autoEditKey,
    editNonce,
    onsync,
    ondeleted,
    onimport,
  }: {
    selectedKey: string | null;
    listing: AssetListing | null;
    cards: ChangesetSummary[] | null;
    hosts: HostRow[];
    order: string[];
    readOnly: boolean;
    /** A personal asset this window may write: the full detail and editor. */
    canOpen: (a: AssetSummary) => boolean;
    /** A just-created asset's key: open it in Source, editing. */
    autoEditKey: string;
    /** Bumped by the `e` key: open the selected asset in Source, editing. */
    editNonce: number;
    onsync: (f: { hostAlias?: string; kind?: string; name?: string }) => void;
    ondeleted: () => void;
    onimport: (id: AssetIdentity) => void;
  } = $props();

  type Tab = 'overview' | 'source' | 'hosts' | 'history';
  const LABEL: Record<Tab, string> = { overview: 'Overview', source: 'Source', hosts: 'Hosts', history: 'History' };

  const sel = $derived(selectedKey ? parseKey(selectedKey) : null);
  const asset = $derived.by((): AssetSummary | null => {
    if (sel?.type !== 'asset') return null;
    const found = listing?.assets.find((a) => catalogOf(a) === sel.catalog && a.kind === sel.kind && a.name === sel.name);
    if (found) return found;
    // Just created (the listing has not caught up) or chosen from Lint all:
    // a personal asset this window may write opens by name.
    return !readOnly && sel.catalog === 'personal'
      ? { kind: sel.kind, name: sel.name, version: '', description: '', tags: [], hosts: [], catalog: 'personal' }
      : null;
  });
  const identity = $derived(
    sel?.type === 'identity' && listing ? (identitiesOf(listing).find((i) => i.kind === sel.kind && i.name === sel.name) ?? null) : null,
  );
  const orphanRows = $derived(
    sel?.type === 'orphan' && listing ? listing.unmanaged.filter((r) => r.state === 'orphan' && r.kind === sel.kind && r.name === sel.name) : [],
  );
  const card = $derived(sel?.type === 'card' ? ((cards ?? []).find((c) => c.id === sel.id) ?? null) : null);
  // Defence in depth: only a personal asset is ever read through
  // `catalog_get_asset`, whatever `canOpen` says.
  const full = $derived(!!asset && !readOnly && catalogOf(asset) === 'personal' && canOpen(asset));
  const tabs = $derived.by((): Tab[] => {
    if (asset) return full ? ['overview', 'source', 'hosts', 'history'] : readOnly ? ['overview', 'hosts'] : ['overview', 'hosts', 'history'];
    if (identity) return ['overview', 'hosts'];
    return ['overview'];
  });

  let pick = $state<Tab>('overview');
  // Open in Source, editing: a just-created asset, or `e` (a new nonce).
  // Decided BEFORE `AssetDetail` mounts (`$effect.pre`; PF1): the detail
  // reads `startInEdit` once, when it is created.
  let editing = $state(false);
  let seenNonce = untrack(() => editNonce);
  $effect.pre(() => {
    const key = selectedKey;
    const nonce = editNonce;
    const canEdit = full;
    untrack(() => {
      const edit = (key !== null && key === autoEditKey) || nonce !== seenNonce;
      seenNonce = nonce;
      editing = edit && canEdit;
      pick = editing ? 'source' : 'overview';
    });
  });
  // The tab the person picked, unless this row has no such tab (a selection
  // change or a read-only flip took it away).
  const tab = $derived<Tab>(tabs.includes(pick) ? pick : 'overview');

  // ── History ──────────────────────────────────────────────────────────
  type HistoryState = { key: string; rows: CommitEntry[] | null; error: string | null };
  let history = $state<HistoryState | null>(null);
  let historySeq = 0;

  /** The History tab's words for a refusal (R13): a client without a grant,
   *  and a hub older than M5 that does not know the action. */
  function historyError(e: IpcError, catalog: string): string {
    if (e.code === 'E_FORBIDDEN') return `This client has no grant on catalog ${catalog}; ask the operator to grant assets on it.`;
    if (e.code === 'E_INVALID' && /\bunknown\b[^]*\basset_history\b/i.test(e.message)) return 'This hub does not keep asset history yet; update the hub.';
    return e.message;
  }

  // Every entry to the History tab reads it afresh (a Save since the last
  // visit added a commit; an error earlier deserves a retry): leaving the
  // tab drops what was shown.
  $effect(() => {
    if (tab !== 'history' || !asset || !selectedKey) {
      untrack(() => (history = null));
      return;
    }
    const key = selectedKey;
    const a = asset;
    if (untrack(() => history?.key) === key) return;
    const seq = ++historySeq;
    history = { key, rows: null, error: null };
    void assetHistory(a.kind, a.name, a.catalog).then((r) => {
      if (seq !== historySeq || history?.key !== key) return;
      history = r.ok ? { key, rows: r.value, error: null } : { key, rows: null, error: historyError(r.error, catalogOf(a)) };
    });
  });

  const now = Math.floor(Date.now() / 1000);
  const sideWords = (side?: string | null) => (side === 'host' ? ' — edited on the host' : side === 'catalog' ? ' — differs from the catalog' : '');
  const kindWord = (k: string) => k.replace('_', ' ');
  const title = $derived(asset?.name ?? identity?.name ?? orphanRows[0]?.name ?? card?.summary ?? '');
  const eyebrow = $derived(
    asset
      ? `${kindWord(asset.kind)} · catalog ${catalogOf(asset)}`
      : identity
        ? `${kindWord(identity.kind)} · on hosts, not in a catalog`
        : orphanRows.length
          ? `${kindWord(orphanRows[0].kind)} · orphan`
          : card
            ? `${card.kind} card · ${card.state}`
            : '',
  );
  const noStale: ReadonlySet<string> = new Set();
</script>

{#if !sel}
  <p class="empty" data-testid="inspector-empty">Select an asset.</p>
{:else if !asset && !identity && !orphanRows.length && !card}
  <p class="empty" data-testid="inspector-empty">This row is no longer listed.</p>
{:else}
  <Inspector {eyebrow} {title} tabs={tabs.map((t) => ({ id: t, label: LABEL[t] }))} active={tab} onchange={(id) => (pick = id as Tab)}>
    {#if asset && full}
      <!-- One instance for Overview, Source and Hosts, kept mounted (hidden)
           under History too, so going back never refetches. An open editor
           is kept mounted by AssetDetail itself (hidden outside Source), so
           an unsaved draft survives every tab. Re-created only for another
           asset or an `e` press. -->
      {#key `${selectedKey}::${editNonce}`}
        <div class="detail" hidden={tab === 'history'}>
          <AssetDetail
            kind={asset.kind}
            name={asset.name}
            {hosts}
            section={tab === 'source' ? 'source' : tab === 'hosts' ? 'hosts' : 'overview'}
            onsection={() => (pick = 'source')}
            {onsync}
            {ondeleted}
            startInEdit={editing}
          />
        </div>
      {/key}
    {/if}
    {#if asset && tab === 'history'}
      <div class="pad" data-testid="inspector-history">
        {#if !history || (history.rows === null && history.error === null)}
          <p class="muted">Loading…</p>
        {:else if history.error}
          <p class="error">{history.error}</p>
        {:else if history.rows && history.rows.length === 0}
          <p class="muted">No commits touch this asset yet.</p>
        {:else if history.rows}
          <ol class="commits">
            {#each history.rows as c (c.sha)}
              <li><span class="sha">{c.sha.slice(0, 7)}</span> <span class="subj">{c.subject}</span> <span class="muted">{c.author} · {ago(c.at, now)}</span></li>
            {/each}
          </ol>
        {/if}
      </div>
    {:else if asset && !full && tab === 'hosts'}
      <ul class="pad hosts" data-testid="inspector-hosts">
        {#each asset.hosts as h (`${h.host_alias}:${h.harness}`)}
          <li>{h.host_alias}: {h.state.replace('_', ' ')}{sideWords(h.drift_side)}{h.harness === 'claude' ? '' : ` (${h.harness})`}</li>
        {/each}
        {#if asset.hosts.length === 0}<li class="muted">Not scanned on any host.</li>{/if}
      </ul>
    {:else if asset && !full}
      {@const b = scopeBadge(asset)}
      <div class="pad" data-testid="inspector-summary">
        <p>{asset.description}</p>
        <dl class="kv">
          <dt>Scope</dt><dd><Badge tone={b.tone} dashed={b.dashed} label={b.label} title={b.title} /></dd>
          <dt>Version</dt><dd class="mono">{asset.version}</dd>
          <dt>Hosts</dt><dd><HostStrip {order} states={assetDots(asset, order, noStale)} /></dd>
        </dl>
        <p class="muted" data-testid="inspector-note">
          {readOnly
            ? 'Read-only: this window has no grant on this catalog.'
            : 'Managed in catalog ' + catalogOf(asset) + ': this window shows where it is installed and its History; editing it comes later.'}
        </p>
      </div>
    {:else if identity && tab === 'hosts'}
      <ul class="pad hosts" data-testid="inspector-hosts">
        {#each identity.hosts as h (`${h.host_alias}:${h.harness}`)}
          <li>{h.host_alias} ({h.harness}) <span class="mono">{h.host_hash?.slice(0, 7) ?? '—'}</span></li>
        {/each}
      </ul>
    {:else if identity}
      <div class="pad" data-testid="inspector-summary">
        <dl class="kv">
          <dt>Found on</dt><dd>{[...new Set(identity.hosts.map((h) => h.host_alias))].join(', ')}</dd>
          <dt>Copies</dt><dd>{identity.variants > 1 ? `${identity.variants} different` : 'identical'}</dd>
          {#if identity.reason}<dt>Needs</dt><dd>{identity.reason}</dd>{/if}
        </dl>
        {#if !readOnly}
          <button type="button" class="btn btn--quiet is-bounded" onclick={() => onimport(identity)} data-testid="inspector-import">Import…</button>
        {/if}
      </div>
    {:else if orphanRows.length}
      <div class="pad" data-testid="inspector-summary">
        <p>Fleet put it on {[...new Set(orphanRows.map((r) => r.host_alias))].join(', ')}; the catalog no longer has it. The next sync of those hosts removes it, with a backup.</p>
      </div>
    {:else if card}
      <div class="pad" data-testid="inspector-summary">
        <p class="sentence">{card.summary}</p>
        <dl class="kv">
          <dt>State</dt><dd>{card.state}</dd>
          <dt>Proposed</dt><dd>{ago(card.created_at, now)}</dd>
          {#if card.error}<dt>Error</dt><dd class="error">{card.error}</dd>{/if}
        </dl>
        <ul class="groups">
          {#each Object.entries(card.groups ?? {}) as [g, n] (g)}<li><Badge label={`${g} ${n}`} /></li>{/each}
        </ul>
      </div>
    {/if}
  </Inspector>
{/if}

<style>
  .empty { padding: 14px; color: var(--fg-muted); }
  .pad { margin: 0; padding: 12px 14px; font-size: 13px; }
  .hosts { list-style: none; display: grid; gap: 4px; }
  .kv { display: grid; grid-template-columns: 92px 1fr; gap: 5px 10px; margin: 8px 0; font-size: 12px; }
  .kv dt { color: var(--fg-muted); }
  .kv dd { margin: 0; }
  .commits { margin: 0; padding: 0 0 0 18px; display: grid; gap: 4px; font-size: 12px; }
  .sha, .mono { font-family: ui-monospace, monospace; font-size: 11.5px; }
  .groups { list-style: none; display: flex; gap: 4px; flex-wrap: wrap; margin: 0; padding: 0; }
  .sentence { font-weight: 600; }
  .muted { color: var(--fg-muted); }
  .error { color: var(--usage-crit); }
</style>
