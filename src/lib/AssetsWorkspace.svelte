<script lang="ts">
  import { viewKey } from './shortcuts';
  import Loader from './Loader.svelte';
  import ListLoading from './ListLoading.svelte';
  import { tick, untrack, type Snippet } from 'svelte';
  import AssetsRail from './AssetsRail.svelte';
  import AssetsInbox from './AssetsInbox.svelte';
  import AssetsLayers from './AssetsLayers.svelte';
  import LayerInspector from './LayerInspector.svelte';
  import LayerChangeForm from './LayerChangeForm.svelte';
  import AssetsHosts from './AssetsHosts.svelte';
  import HostInspector from './HostInspector.svelte';
  import AssetList from './AssetList.svelte';
  import AssetInspector from './AssetInspector.svelte';
  import AssetsFooter from './AssetsFooter.svelte';
  import SyncPlanView from './SyncPlanView.svelte';
  import QueryInput from './QueryInput.svelte';
  import Badge from './Badge.svelte';
  import {
    catalog, identitiesOf, inventory, lastSyncRun, planSync,
    type AssetIdentity, type AssetSummary, type HostScanResult, type SyncPlan, type SyncRunSummary,
  } from './assets';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import { orgs } from './orgs';
  import {
    admitCatalog, blockedOnSecrets, canWrite, cardViews, catalogStatuses, changesetSummaries, keyOf, layerListing, layersByCatalog, loadAllLayers,
    loadCatalogStatuses, loadChangesets, parseKey, PERSONAL, proposeLayerChange, unadmitCatalog,
    type ChangesetSummary, type LayerChange, type WorkspaceView,
  } from './assets_workspace';
  import { coveringNewCard, mergePlans, NEEDS_A_LOOK, olderHubWords, primaryVerb, proposeAndReload } from './assets_cards';
  import { runCardVerb, type CardVerbs } from './card_actions';
  import { buildInbox, hostOrderOf, keepCard, lastScanOf, sentence } from './assets_inbox';
  import { keep, parseQuery, type QueryRow } from './assets_query';
  import { get } from 'svelte/store';
  import { isEditable } from './terminal_keys';
  import { push, pushError } from './toasts';

  /** The Assets workspace (spec, Workspace shell): rail · list with a
   *  sentence header and the query · Inspector, over a footer. It owns the
   *  view, the query and the selection; `AssetsPanel` owns loading, probing
   *  and every dialog, and is told what the person asked for. */
  let {
    readOnly = false,
    readOnlyClient = null,
    visible = true,
    busy = '',
    error = null,
    scanResults = null,
    loading = false,
    scanDisabled = false,
    importBlocked = null,
    failed,
    selectedKey = $bindable(null),
    view = $bindable('inbox'),
    cardBusy = $bindable(''),
    autoEditKey = '',
    plan = null,
    planFilter = {},
    onplanclose = () => {},
    onapplied = () => {},
    onapplying = () => {},
    onreplanned = () => {},
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
    /** Whether the Assets tab is shown: a hidden one takes no keys. */
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
    /** The rail's view; bound so `AssetsPanel` can show a requested row. */
    view?: WorkspaceView;
    /** A card verb is running (apply, dismiss, undo, admit, propose); bound so
     *  the panel holds the quick switcher's commands meanwhile. */
    cardBusy?: string;
    /** A just-created asset's key: the Inspector opens it in Source, editing. */
    autoEditKey?: string;
    /** An open sync plan: it takes the main column until it is closed (Back, Esc). */
    plan?: SyncPlan | null;
    /** The filter `plan` was computed from ("Plan anyway" re-plans with it). */
    planFilter?: { hostAlias?: string; kind?: string; name?: string };
    onplanclose?: () => void;
    onapplied?: (s: SyncRunSummary) => void;
    onapplying?: (b: boolean) => void;
    onreplanned?: (p: SyncPlan) => void;
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


  let queryText = $state('');
  let showProblems = $state(false);
  let showGrant = $state(false);
  const uid = $props.id();
  const grantNoteId = `${uid}-grant`;
  // Bumped by the `e` key: open the selected asset in Source. `editKey` is
  // set with the selection and cleared once the Inspector has read it.
  let editNonce = $state(0);
  let editKey = $state('');
  let listEl: HTMLElement | undefined = $state();
  let inspEl: HTMLElement | undefined = $state();
  let queryEl: ReturnType<typeof QueryInput> | undefined = $state();
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
    // Layers across every catalog once they are loaded, else the personal listing's.
    layers: $layersByCatalog
      ? Object.values($layersByCatalog).reduce((n, l) => n + l.layers.length, 0)
      : ($layerListing?.layers.length ?? 0),
    hosts: shown.length,
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
    review = null;
    selectedKey = key;
  }
  // An asset selected from outside (the quick switcher) leaves a Rollout
  // review, which would otherwise keep covering the list it is in — except
  // while a card verb runs (the review's own apply is in flight).
  $effect(() => {
    if (selection?.type === 'asset') untrack(() => { if (cardBusy === '') review = null; });
  });

  // ── The sync plan and the Rollout review take the main column ──────────
  /** A Rollout card's plan, read-only: each pending host planned host-scoped
   *  and joined (R15). Its Apply is the card's own, never this view's. */
  let review = $state<{ plan: SyncPlan; owned: { assets: Set<string>; catalogs: Set<string> } } | null>(null);
  const planOpen = $derived(!!review || !!plan);
  /** A sync plan (with its Apply) is the main column's one primary. */
  const syncPlanOpen = $derived(!!plan && !review);
  // A plan computed elsewhere (the header's Sync, `s`) replaces a review.
  $effect(() => {
    if (plan) untrack(() => (review = null));
  });
  async function reviewRollout(id: number) {
    const v = $cardViews[id];
    if (!v || anyBusy) return;
    const pending = v.items.filter((i) => i.state === 'pending');
    setCardBusy('review');
    let plans;
    try {
      plans = await Promise.all(pending.map((i) => planSync({ hostAlias: i.name })));
    } finally {
      setCardBusy('');
    }
    const ok = plans.flatMap((r) => (r.ok ? [r.value] : []));
    for (const r of plans) if (!r.ok) pushError(r.error, 'Plan');
    if (plan) onplanclose();
    review = {
      plan: mergePlans(ok),
      owned: { assets: new Set(pending.flatMap((i) => i.params.assets ?? [])), catalogs: new Set(v.catalogs ?? []) },
    };
  }
  /** "Plan anyway" inside a review: that host's plan is swapped in. */
  function reviewReplanned(p: SyncPlan) {
    if (!review) return;
    const aliases = new Set(p.hosts.map((h) => h.host_alias));
    review = { ...review, plan: mergePlans([{ ...review.plan, hosts: review.plan.hosts.filter((h) => !aliases.has(h.host_alias)) }, p]) };
  }
  /** Esc closes only the plan, never the whole Assets overlay (the App
   *  listens on `window`, in the bubble phase). It is taken here in the
   *  capture phase, and also from `body`, so it holds even when focus fell
   *  out of the view (a removed or disabled button). A field, a dialog and
   *  anything outside the workspace (the agent panel) keep their own Esc. */
  function onPlanEsc(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest?.('dialog,[role="dialog"]') || isEditable(t)) return;
    if (t && t !== document.body && t !== document.documentElement && !rootEl?.contains(t)) return;
    e.preventDefault();
    e.stopPropagation();
    void closePlan();
  }
  $effect(() => {
    if (!visible || !planOpen) return;
    window.addEventListener('keydown', onPlanEsc, true);
    return () => window.removeEventListener('keydown', onPlanEsc, true);
  });
  /** Back and Esc: close the plan or the review — not while an apply runs. */
  async function closePlan() {
    if (!planOpen || busy === 'apply') return;
    review = null;
    if (plan) onplanclose();
    await tick();
    listEl?.focus({ preventScroll: true });
  }

  // ── Card verbs (R12, R13): run, reload, toast (`card_actions`) ─────────
  const anyBusy = $derived(busy !== '' || cardBusy !== '');
  async function changed() {
    await loadChangesets();
    void loadAllLayers($catalogStatuses);
    onrefresh();
  }
  const setCardBusy = (b: string) => (cardBusy = b);
  const cardVerbs: CardVerbs & { reject: (id: number, positions: number[]) => void } = {
    apply: (id, positions) =>
      void runCardVerb('apply', id, { positions, setBusy: setCardBusy, onchanged: changed, select: (c) => select(keyOf({ type: 'card', id: c })) }),
    dismiss: (id) => void runCardVerb('dismiss', id, { setBusy: setCardBusy, onchanged: changed }),
    undo: (id) => void runCardVerb('undo', id, { setBusy: setCardBusy, onchanged: changed }),
    reject: (id, positions) => void runCardVerb('reject', id, { positions, setBusy: setCardBusy, onchanged: changed }),
    synchost: (host) => onsync({ hostAlias: host }),
  };
  const selectedCard = $derived(selection?.type === 'card' ? (($changesetSummaries ?? []).find((c) => c.id === selection.id) ?? null) : null);
  /** R20 per catalog: a card is actionable when this window may write every
   *  catalog its apply commits to (a card naming none needs personal). */
  const cardWritable = (c: ChangesetSummary) => (c.catalogs?.length ? c.catalogs : [PERSONAL]).every((cat) => canWrite(cat, ctx));
  /** The selected card's verb is on screen and runnable: the main column's one
   *  primary then, and the target of ⌘↵. Otherwise Sync fleet is. */
  const cardPrimary = $derived(
    !planOpen &&
      !!selectedCard &&
      !readOnly &&
      view === 'inbox' &&
      !!inbox &&
      !failed &&
      cardWritable(selectedCard) &&
      primaryVerb(selectedCard, $cardViews[selectedCard.id] ?? null) !== null &&
      keepCard(query, selectedCard),
  );
  const pendingOf = (id: number): number[] => ($cardViews[id]?.items ?? []).filter((i) => i.state === 'pending').map((i) => i.position);

  // ── Layers (R17, R5, R6): create / rename / move propose a card ────────
  /** Every loaded catalog's layers; until they load, the personal listing. */
  const layersAll = $derived($layersByCatalog ?? ($layerListing ? { [PERSONAL]: $layerListing } : null));
  /** The loaded catalogs this window may author layers in. */
  const layerCatalogs = $derived(
    ($catalogStatuses ? $catalogStatuses.filter((s) => s.state === 'loaded').map((s) => s.name) : [PERSONAL]).filter((c) => canWrite(c, ctx)),
  );
  let creating = $state(false);
  /** The New layer form is on screen: its Propose is the main column's one
   *  primary, so the header's Sync goes plain (R-C). */
  const creatingShown = $derived(creating && view === 'layers' && !planOpen && !failed && !readOnly && layerCatalogs.length > 0);
  const layerSelected = $derived.by(() => {
    if (selection?.type !== 'layer') return null;
    const l = layersAll?.[selection.catalog];
    const def = l?.layers.find((x) => x.name === selection.name);
    return l && def ? { catalog: selection.catalog, def, listing: l } : null;
  });
  /** A layer change becomes a card the person applies (R5): jump to it in
   *  the Inbox, with Apply now in the toast. */
  async function proposeChange(c: LayerChange) {
    if (anyBusy) return;
    cardBusy = 'card';
    let r;
    try {
      r = await proposeLayerChange(c);
    } finally {
      cardBusy = '';
    }
    if (!r.ok) {
      const older = olderHubWords(r.error, 'propose layer changes');
      if (older) push({ kind: 'error', message: older });
      else pushError(r.error, 'Propose');
      return;
    }
    const v = r.value;
    await loadChangesets();
    creating = false;
    view = 'inbox';
    void closePlan();
    select(`card:${v.id}`);
    push({ kind: 'info', message: `Card ready: ${v.summary}`, action: { label: 'Apply now', run: () => cardVerbs.apply(v.id, null) } });
  }
  /** "Propose again": the hub re-derives its cards from the hosts as they are. */
  async function proposeAgain() {
    // The hub's propose needs the personal grant.
    if (anyBusy || !canWrite(PERSONAL, ctx)) return;
    cardBusy = 'card';
    let n;
    try {
      n = await proposeAndReload();
    } finally {
      cardBusy = '';
    }
    if (n !== null) push({ kind: 'info', message: `Proposed again: ${n} open cards` });
  }

  // ── Hosts (R18): the admission toggles ─────────────────────────────────
  const orgName = (id: number | null) => (id === null ? null : ($orgs.find((o) => o.id === id)?.name ?? `org ${id}`));
  const selectedHost = $derived(selection?.type === 'host' ? selection.alias : null);
  /** Bumped after an admission changes: the Host Inspector re-reads what the host receives. */
  let hostRevision = $state(0);
  /** Admit or unadmit a catalog for a host; then re-read the catalogs (their
   *  `admitted` lists) and the layers. Unadmitting leaves what is installed. */
  async function toggleAdmission(host: string, catalogName: string, on: boolean) {
    // The view disables the toggle; this holds for anything else that calls it.
    if (anyBusy || readOnly || !canWrite(catalogName, ctx)) return;
    cardBusy = 'admit';
    let r;
    let reload;
    try {
      r = await (on ? admitCatalog(host, catalogName) : unadmitCatalog(host, catalogName));
      if (r.ok) {
        reload = await loadCatalogStatuses();
        if (reload.ok) void loadAllLayers(get(catalogStatuses));
      }
    } finally {
      cardBusy = '';
    }
    if (!r.ok) {
      pushError(r.error, 'Admission');
      return;
    }
    hostRevision += 1;
    push({
      kind: 'info',
      message: on ? `${host} now receives ${catalogName}` : `${host} no longer receives ${catalogName}; what is installed stays until you remove it`,
    });
    // The change is made; a failed re-read leaves the toggles stale (the
    // catalogs read as none), which the person must be told.
    if (reload && !reload.ok) pushError(reload.error, 'Reload catalogs');
  }

  // ── The keyboard (Rulings R22) ────────────────────────────────────────
  // Focus moves between rows: the DOM order is the display order, a folded
  // section renders no rows. Space/Enter select natively (a static row
  // handles both itself). Esc is the App's (it closes the Assets overlay),
  // except in the query, which keeps it (QueryInput).
  function rows(): HTMLElement[] {
    return listEl ? Array.from(listEl.querySelectorAll<HTMLElement>('[data-row-key]')).filter((r) => r.matches('button, [tabindex]')) : [];
  }
  function focusedKey(): string | null {
    const el = document.activeElement as HTMLElement | null;
    return el && el !== listEl && listEl?.contains(el) ? (el.dataset.rowKey ?? null) : null;
  }
  function move(delta: 1 | -1) {
    const all = rows();
    if (!all.length) return;
    const at = all.findIndex((r) => r.dataset.rowKey === (focusedKey() ?? selectedKey));
    const next = at < 0 ? (delta > 0 ? 0 : all.length - 1) : Math.min(all.length - 1, Math.max(0, at + delta));
    all[next].focus();
    // The Inbox and the Library scroll the same element (`.body`).
    all[next].scrollIntoView?.({ block: 'nearest' });
  }
  /** The listed asset behind a key, when this window may open and write it
   *  (`s` and `e` act where the Inspector offers Sync and Edit). */
  function ownAsset(key: string | null): AssetSummary | null {
    const sel = key ? parseKey(key) : null;
    if (readOnly || !listing || sel?.type !== 'asset') return null;
    const a = listing.assets.find((x) => x.kind === sel.kind && x.name === sel.name && (x.catalog ?? PERSONAL) === sel.catalog);
    return a && canOpen(a) ? a : null;
  }
  async function editAsset(key: string) {
    editKey = key;
    selectedKey = key;
    editNonce += 1;
    await tick();
    if (editKey === key) editKey = '';
  }
  function onKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    // PF10: a dialog (a modal or the catalog chip's popover) and a field
    // keep their keys.
    if (!visible || e.defaultPrevented || target?.closest?.('dialog,[role="dialog"]') || isEditable(target)) return;
    // A plan or a review covers the list: its rows take no keys (Esc is
    // taken by `onPlanEsc`, below).
    if (planOpen) return;
    // The keys are the registry's `assets` rows (step 0.1).
    const act = viewKey('assets', e);
    if (!act) return;
    if (act === 'assets.primary') {
      // The region's one primary (R18, R-C): the selected card's verb when
      // it applies (a review verb only selects, so there is nothing to run),
      // else Sync fleet.
      if (readOnly) return;
      // The New layer form is the main column's primary: Sync is not.
      if (creatingShown) return;
      if (selectedCard && cardPrimary) {
        const v = primaryVerb(selectedCard, $cardViews[selectedCard.id] ?? null);
        if (v?.apply && !anyBusy) {
          e.preventDefault();
          cardVerbs.apply(selectedCard.id, null);
        }
        return;
      }
      if (busy === '') {
        e.preventDefault();
        onsync({});
      }
      return;
    }
    // `a`, `s`, `e` and `i` act on the selection from the list, the Inspector
    // or the workspace itself: not from the footer's chips or the rail, where
    // a letter typed is not a request to sync or edit something.
    const inScope = !!target && (!!listEl?.contains(target) || !!inspEl?.contains(target) || target === rootEl);
    const onSelection = act === 'assets.adopt' || act === 'assets.sync' || act === 'assets.edit' || act === 'assets.ignore';
    if (!inScope && onSelection) return;
    const key = focusedKey() ?? selectedKey;
    const sel = key ? parseKey(key) : null;
    // The arrows move rows in the list only; elsewhere (the Inspector's
    // scrolling tab panel) they keep scrolling natively.
    const inList = !!target && !!listEl?.contains(target);
    switch (act) {
      case 'assets.down':
        // (An arrow, unlike `j`, scrolls natively outside the list.)
        if (e.key === 'ArrowDown' && !inList) break;
        e.preventDefault();
        move(1);
        break;
      case 'assets.up':
        if (e.key === 'ArrowUp' && !inList) break;
        e.preventDefault();
        move(-1);
        break;
      case 'assets.search':
        e.preventDefault();
        queryEl?.focus();
        break;
      case 'assets.adopt': {
        if (readOnly || sel?.type !== 'identity' || !listing) break;
        // A New card that covers the identity is the one adopt: apply it
        // rather than opening Import for the same copy. One that needs a
        // look, or that hides the copy, is not an adopt: `a` selects it.
        const covering = coveringNewCard($changesetSummaries, $cardViews, sel.kind, sel.name);
        if (covering) {
          const first = $cardViews[covering.id]?.items[0];
          if (!first || first.grp === NEEDS_A_LOOK || first.action === 'hide') {
            e.preventDefault();
            select(keyOf({ type: 'card', id: covering.id }));
          } else if (!anyBusy && cardWritable(covering)) {
            e.preventDefault();
            cardVerbs.apply(covering.id, null);
          }
          break;
        }
        const id = identitiesOf(listing).find((i) => i.kind === sel.kind && i.name === sel.name);
        // As the row's own Import: never a fleet or harness internal.
        if (id && id.class !== 'fleet_internal' && id.class !== 'harness_internal') {
          e.preventDefault();
          onimport(id);
        }
        break;
      }
      case 'assets.sync': {
        const a = busy === '' ? ownAsset(key) : null;
        if (!a) break;
        e.preventDefault();
        onsync({ kind: a.kind, name: a.name });
        break;
      }
      case 'assets.edit':
        if (!key || !ownAsset(key)) break;
        e.preventDefault();
        void editAsset(key);
        break;
      case 'assets.ignore': {
        // Ignore: reject a card's pending items — the selected card's, or the
        // New card that covers the selected identity. A person's verdict:
        // it sticks until the content changes.
        if (readOnly || anyBusy) break;
        const target =
          sel?.type === 'card' ? (($changesetSummaries ?? []).find((c) => c.id === sel.id) ?? null)
          : sel?.type === 'identity' ? coveringNewCard($changesetSummaries, $cardViews, sel.kind, sel.name)
          : null;
        const id = target && cardWritable(target) ? target.id : null;
        const positions = id === null ? [] : pendingOf(id);
        if (id === null || !positions.length) break;
        e.preventDefault();
        cardVerbs.reject(id, positions);
        break;
      }
    }
  }

  // A shown panel gives the list the keyboard, unless focus is already
  // inside (`j` works the moment Assets opens).
  let rootEl: HTMLElement | undefined = $state();
  $effect(() => {
    if (!visible || !listEl) return;
    untrack(() => {
      // An open plan holds the keyboard, not the list behind it.
      if (!rootEl?.contains(document.activeElement)) (rootEl?.querySelector<HTMLElement>('.plan-view') ?? listEl)?.focus({ preventScroll: true });
    });
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="ws" data-testid="assets-workspace" role="group" aria-label="Assets workspace" bind:this={rootEl} onkeydown={onKeydown}>
  <AssetsRail {view} {counts} {readOnly} busy={busy !== ''} onview={(v) => {
      view = v;
      creating = false;
      void closePlan();
    }} {onsecrets} />

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
            >{#if loading}<Loader name="comet" size={12} class="btn-loader" />{/if}{loading ? 'Loading…' : 'Refresh'}</button
          >
        {/if}
        <button type="button" class="btn btn--quiet" onclick={onscan} disabled={busy !== '' || scanDisabled} data-testid="assets-scan"
          >{busy === 'scan' ? 'Scanning…' : 'Rescan'}</button
        >
        {#if !readOnly}
          <button type="button" class="btn" class:btn--primary={!cardPrimary && !syncPlanOpen && !creatingShown} onclick={() => onsync({})} disabled={busy !== ''} data-testid="assets-sync"
            >{busy === 'plan' ? 'Planning…' : 'Sync fleet'}{#if !cardPrimary && !planOpen && !creatingShown} <kbd>⌘↵</kbd>{/if}</button
          >
        {/if}
      </div>
      <div class="line">
        <QueryInput bind:this={queryEl} bind:value={queryText} {vocab} onescape={() => listEl?.focus()} />
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
      {#if review}
        <SyncPlanView
          mode="review"
          plan={review.plan}
          owned={review.owned}
          onclose={closePlan}
          onopensecrets={onsecrets}
          onreplanned={reviewReplanned}
        />
      {:else if plan}
        <SyncPlanView {plan} filter={planFilter} onclose={closePlan} {onapplied} onopensecrets={onsecrets} {onapplying} {onreplanned} />
      {:else if failed}
        {@render failed()}
      {:else if listing && view === 'inbox' && inbox}
        <AssetsInbox
          {inbox}
          {order}
          {selectedKey}
          {query}
          readonly={readOnly}
          views={$cardViews}
          busy={anyBusy}
          primaryId={cardPrimary ? (selectedCard?.id ?? null) : null}
          canActOn={cardWritable}
          oncard={cardVerbs}
          onselect={select}
          onimport={(i) => onimport(i)}
        />
      {:else if view === 'layers'}
        {#if creatingShown}
          <div class="newlayer">
            <LayerChangeForm
              mode="create"
              catalogs={layerCatalogs}
              catalog={layerCatalogs.includes(PERSONAL) ? PERSONAL : layerCatalogs[0]}
              busy={anyBusy}
              onsubmit={proposeChange}
              oncancel={() => (creating = false)}
            />
          </div>
        {/if}
        <AssetsLayers
          layers={layersAll}
          {order}
          {selectedKey}
          readOnly={readOnly || layerCatalogs.length === 0}
          canPropose={canWrite(PERSONAL, ctx)}
          busy={anyBusy}
          onselect={select}
          onnew={() => (creating = !creating)}
          onpropose={proposeAgain}
        />
      {:else if view === 'hosts'}
        <AssetsHosts
          hosts={shown}
          statuses={$catalogStatuses}
          layers={layersAll}
          {orgName}
          {selectedKey}
          {readOnly}
          busy={anyBusy}
          canAdmit={(c) => canWrite(c, ctx)}
          onselect={select}
          ontoggle={toggleAdmission}
        />
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
        <ListLoading />
      {/if}
    </div>
  </div>

  <div class="insp" bind:this={inspEl}>
    {#if layerSelected}
      {#key selectedKey}
        <LayerInspector
          catalog={layerSelected.catalog}
          layer={layerSelected.def}
          listing={layerSelected.listing}
          {order}
          writable={canWrite(layerSelected.catalog, ctx)}
          busy={anyBusy}
          onchange={proposeChange}
        />
      {/key}
    {:else if selectedHost && view === 'hosts'}
      {#key selectedHost}<HostInspector alias={selectedHost} revision={hostRevision} />{/key}
    {:else}
    <AssetInspector
      {selectedKey}
      {listing}
      cards={$changesetSummaries}
      hosts={$hosts}
      {order}
      {readOnly}
      {canOpen}
      autoEditKey={editKey || autoEditKey}
      {editNonce}
      {onsync}
      oncard={cardVerbs}
      onreview={reviewRollout}
      onreject={cardVerbs.reject}
      onselect={select}
      cardBusy={anyBusy}
      {cardWritable}
      ondeleted={() => {
        selectedKey = null;
        ondeleted();
      }}
      onimport={(i) => onimport(i)}
    />
    {/if}
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
  /* Narrow: the rail shrinks to icons (its buttons keep their names), then the
     Inspector stacks under the list. */
  @media (max-width: 1100px) {
    .ws { grid-template-columns: 56px minmax(0, 1fr) minmax(var(--inspector-min), var(--list-w)); }
  }
  @media (max-width: 860px) {
    .ws {
      grid-template-columns: 56px minmax(0, 1fr);
      grid-template-rows: minmax(0, 1fr) minmax(0, 45%) auto;
      grid-template-areas: 'rail main' 'rail insp' 'foot foot';
    }
    .insp { border-left: 0; border-top: 1px solid var(--border); }
  }
  .head { display: grid; gap: 8px; padding: 10px 14px; border-bottom: 1px solid var(--border); }
  .line { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .sentence { font-size: var(--text-md); font-weight: 600; letter-spacing: -0.005em; }
  .sub { color: var(--fg-muted); font-size: var(--text-xs); white-space: nowrap; }
  .grow { flex: 1; }
  .ro { display: inline-flex; align-items: center; gap: 8px; }
  .grant { font-size: var(--text-xs); color: var(--fg-muted); }
  .grant code, .problems code { font-family: var(--mono); font-size: var(--text-2xs); user-select: text; }
  kbd { font-family: var(--mono); font-size: var(--text-2xs); padding: 0 4px; border-radius: var(--radius-xs); border: 1px solid color-mix(in srgb, currentColor 35%, transparent); opacity: 0.85; }
  .newlayer { padding: 10px 14px 0; }
  .body { flex: 1; min-height: 0; overflow: auto; outline: 0; }
  .body:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .error { margin: 0; padding: 4px 14px; color: var(--usage-crit); }
  .scan-result { margin: 0; padding: 4px 14px; font-size: var(--text-xs); color: var(--fg-muted); }
  .problems { margin: 0; padding: 4px 14px 4px 32px; font-size: var(--text-xs); }
</style>
