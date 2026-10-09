<script lang="ts">
  // Layout L2 (master_detail): a resource's records in a list beside one
  // record's editor. The list reads through the resource's list command
  // (which routes to the hub on a paired desktop, so the page shows the
  // hub's records there, read-only); every change runs one of the
  // resource's declared actions, then re-reads the list.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import RecordEditor from './RecordEditor.svelte';
  import ActionForm from './ActionForm.svelte';
  import FlowView from './FlowView.svelte';
  import OrgSuggestions from '../OrgSuggestions.svelte';
  import PairingResult from './PairingResult.svelte';
  import ActionResult, { type Shown } from './ActionResult.svelte';
  import { catalogStatuses } from '../assets_workspace';
  import { devices, type Pairing } from '../devices';
  import { hosts } from '../hosts';
  import { orgs } from '../orgs';
  import { trackers } from '../trackers';
  import { push, pushError } from '../toasts';
  import WizardDialog from '../forms/WizardDialog.svelte';
  import type { Values } from '../forms/forms';
  import { pairDevice, pairDeviceWizard } from '../forms/pair_device_wizard';
  import type { Page } from './pages';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { columnsOf, cellText, layoutOf, peerLine, peerUp, rowActions, subOf, topology, trustOf } from './layouts';
  import {
    afterChange,
    badgesOf,
    buildArgs,
    idOf,
    itemValue,
    itemsOf,
    listRecords,
    runAction,
    titleOf,
    type ActionSpec,
    type FieldSpec,
    type OptionSource,
    type ResourceRecord,
    type ResourceType,
  } from './resources';

  let {
    page,
    resource,
    readonly = false,
    reason = null,
    resources = [],
    now = () => Math.floor(Date.now() / 1000),
  }: {
    page: Page;
    resource: ResourceType;
    /** Every resource of the bundle: a row or a need may run another's
     *  action (an org's untrusted device is trusted by the device's own). */
    resources?: ResourceType[];
    /** Unix seconds; injectable for tests. */
    now?: () => number;
    /** Show without changing anything (a paired desktop). */
    readonly?: boolean;
    /** Why it is read-only, and where to change it instead. */
    reason?: string | null;
  } = $props();

  let records = $state<ResourceRecord[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<string | null>(null);
  /** Bumped on every re-read, so the editor's draft restarts from it. */
  let version = $state(0);
  let adding = $state(false);
  let busy = $state(false);
  /** A create whose answer is shown (`result: pairing`), until dismissed. */
  let pairing = $state<Pairing | null>(null);
  /** A record action's answer (`result: output | image`), until dismissed. */
  let shown = $state<Shown | null>(null);

  const current = $derived(records.find((r) => idOf(resource, r) === selected) ?? null);

  /** The list beside the editor, a table above it, or Federation's map. */
  const layout = $derived(layoutOf(resource));
  const columns = $derived(columnsOf(resource));
  /** The open tab of a tabbed record (an org): kept across re-reads. */
  let recordTab = $state('overview');
  /** A record action opened from a row: its form shows in the editor. */
  let rowAction = $state<string | null>(null);
  /** A row action that asks first. */
  let asking = $state<{ title: string; message: string; go: () => void } | null>(null);
  const bundle = $derived(resources.length ? resources : [resource]);
  const peers = $derived(
    topology(records.map((r) => ({ id: idOf(resource, r), label: titleOf(resource, r), up: peerUp(r) }))),
  );

  function pick(r: ResourceRecord) {
    const id = idOf(resource, r);
    if (id !== selected) rowAction = null;
    selected = id;
  }

  /** A row's action: one with a form opens it in the editor below; one
   *  without runs on that row, asking first when it says to. */
  function runRow(a: ActionSpec, r: ResourceRecord) {
    selected = idOf(resource, r);
    if (a.params.length) {
      rowAction = a.id;
      return;
    }
    rowAction = null;
    const go = () => void run(a, buildArgs(a, r, null, {}));
    if (a.confirm) asking = { title: a.label, message: a.confirm, go };
    else go();
  }

  /** A device row's "Trust device", through the device's own update. */
  function trustRow(r: ResourceRecord) {
    const t = trustOf(bundle, titleOf(resource, r));
    if (!t) return;
    selected = idOf(resource, r);
    const go = () => void run(t.action, t.args);
    if (t.confirm) asking = { title: `Trust ${titleOf(resource, r)}`, message: t.confirm, go };
    else go();
  }

  /** "Scan all hosts": the per-host rescan, once for each host a record is on. */
  const rescan = $derived((resource.actions ?? []).find((a) => a.id === 'debug_device.rescan'));
  async function scanAll() {
    const a = rescan;
    if (!a) return;
    const seen = new Set<string>();
    for (const r of records) {
      const host = String(r.host ?? '');
      if (!host || seen.has(host)) continue;
      seen.add(host);
      busy = true;
      const res = await runAction(a, buildArgs(a, r, null, {}));
      busy = false;
      if (!res.ok) pushError(res.error, `${a.label} ${host} failed`);
    }
    await reload();
    await afterChange(resource);
  }

  /** Redesign 10.12: "Pair a device" is the pair_device wizard
   *  (one fleet.form/1 spec, the same one the chat shows) rather than the
   *  inline create form; its answer is the same PairingResult. */
  const pairWizard = $derived(resource.id === 'device');
  let wizardOpen = $state(false);
  let wizardBusy = $state(false);
  let wizardError = $state<string | null>(null);

  async function pairFromWizard(v: Values) {
    wizardBusy = true;
    wizardError = null;
    const r = await pairDevice(v);
    wizardBusy = false;
    if (!r.ok) {
      wizardError = r.error.message;
      return;
    }
    wizardOpen = false;
    pairing = r.value;
    await reload();
    await afterChange(resource);
    const made = records.find((x) => titleOf(resource, x) === r.value.name);
    if (made) selected = idOf(resource, made);
  }

  async function reload() {
    const r = await listRecords(resource);
    loaded = true;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    error = null;
    records = Array.isArray(r.value) ? r.value : [];
    version += 1;
    if (!records.some((x) => idOf(resource, x) === selected)) {
      selected = records.length ? idOf(resource, records[0]) : null;
    }
  }

  onMount(() => {
    void reload();
    // The stores the option selects read (hosts, trackers, orgs) and other views
    // keep of this resource, fresh when the page opens.
    void afterChange(resource);
  });

  /** Run an action; a failure is a toast, and the list is re-read either way. */
  async function run(action: ActionSpec, args: Record<string, unknown>): Promise<boolean> {
    rowAction = null;
    busy = true;
    const r = await runAction(action, args);
    busy = false;
    if (!r.ok) pushError(r.error, `${action.label} failed`);
    else if (action.result === 'pairing') pairing = r.value as Pairing;
    else if (action.result === 'output' || action.result === 'image') shown = resultOf(action, r.value);
    else if (action.report) {
      // `{ ok, error? }`: a test that ran and failed is not a failed call.
      const rep = r.value as { ok?: boolean; error?: string | null } | null;
      if (rep?.ok) push({ kind: 'success', message: `${action.label}: ok` });
      else push({ kind: 'error', message: `${action.label}: ${rep?.error ?? 'failed'}` });
    }
    await reload();
    await afterChange(resource);
    return r.ok;
  }

  /** What `shown` holds for an action's answer, titled by the action and
   *  the record it ran on. */
  function resultOf(action: ActionSpec, value: unknown): Shown {
    const title = current ? `${action.label.replace(/…$/, '')} · ${titleOf(resource, current)}` : action.label;
    const v = (value ?? {}) as Record<string, unknown>;
    if (action.result === 'image')
      return { kind: 'image', title, caption: String(v.caption ?? ''), mime: String(v.mime ?? 'image/png'), data: String(v.data ?? '') };
    return {
      kind: 'output',
      title,
      output: String(v.output ?? ''),
      exit_code: Number(v.exit_code ?? 0),
      truncated: v.truncated === true,
    };
  }

  async function create(params: Record<string, string>) {
    const c = resource.create;
    if (!c) return;
    const ok = await run(c, buildArgs(c, null, null, params));
    if (ok) {
      adding = false;
      const name = (params[resource.title_field] ?? '').trim();
      const made = records.find((r) => titleOf(resource, r) === name);
      if (made) selected = idOf(resource, made);
    }
  }

  /** What an option source offers, from the store the renderer already holds. */
  function sourceOptions(source: OptionSource): { value: string; label: string }[] {
    if (source === 'hosts') return get(hosts).map((h) => ({ value: h.alias, label: h.alias }));
    if (source === 'orgs') return get(orgs).map((o) => ({ value: o.name, label: o.name }));
    if (source === 'devices') return get(devices).map((d) => ({ value: d.name, label: d.name }));
    if (source === 'catalogs') return (get(catalogStatuses) ?? []).map((c) => ({ value: c.name, label: c.name }));
    return get(trackers).map((t) => ({ value: String(t.id), label: t.name }));
  }

  /** A select's choices for a list field's add form: the source's values,
   *  minus the ones the record already has. */
  function options(field: FieldSpec, param: string): { value: string; label: string }[] {
    const action = field.type === 'items' ? field.add.find((a) => a.params.some((p) => p.name === param)) : undefined;
    const spec = action?.params.find((p) => p.name === param);
    if (!spec || spec.type !== 'options' || !current) return [];
    const have = new Set(itemsOf(field, current).map(itemValue));
    return sourceOptions(spec.source).filter((o) => !have.has(o.value));
  }

  /** A create or record action form's choices: the source's values,
   *  nothing to leave out. */
  function actionOptions(action: ActionSpec | undefined, param: string): { value: string; label: string }[] {
    const spec = action?.params.find((p) => p.name === param);
    return spec?.type === 'options' ? sourceOptions(spec.source) : [];
  }
  const createOptions = (param: string) => actionOptions(resource.create, param);
</script>

<div class="resource-page" data-testid={`resource-${resource.id}`}>
  {#if reason}<p class="reason" data-testid="resource-readonly">{reason}</p>{/if}

  {#if !readonly}
    {#each page.list_items ?? [] as item, i (i)}
      {#if item.type === 'notice'}
        <p class={`notice ${item.tone}`}>{item.text}</p>
      {:else if item.type === 'custom' && item.component === 'org_suggestions'}
        <OrgSuggestions onchanged={() => void reload()} />
      {/if}
    {/each}
  {/if}

  {#if layout === 'list'}
    <div class="md">
      <div class="list">
        <ul role="listbox" aria-label={resource.plural} data-testid="resource-list">
          {#each records as r (idOf(resource, r))}
            <li role="presentation">
              <button
                type="button"
                role="option"
                aria-selected={selected === idOf(resource, r)}
                data-testid="resource-row"
                onclick={() => (selected = idOf(resource, r))}>
                {#if resource.color_field}
                  <span class="swatch" style:background={String(r[resource.color_field] ?? '') || 'transparent'}></span>
                {/if}
                <span class="title">{titleOf(resource, r)}</span>
                {#each badgesOf(resource, r) as b (b)}<span class="badge" data-testid="resource-badge">{b}</span>{/each}
              </button>
            </li>
          {/each}
        </ul>
        {@render status()}
        {@render addControls()}
      </div>
      <div class="detail">{@render detail()}</div>
    </div>
  {:else if layout === 'federation'}
    <div class="fed" data-testid="federation">
      <figure class="topo" data-testid="federation-map">
        <svg viewBox="0 0 320 200" role="img" aria-label={`This hub and ${records.length} linked ${records.length === 1 ? 'hub' : 'hubs'}`}>
          {#each peers as n (n.id)}
            <line x1="160" y1="100" x2={n.x} y2={n.y} class="edge" class:down={!n.up} data-testid="federation-edge" />
          {/each}
          <circle cx="160" cy="100" r="26" class="node hub" />
          <text x="160" y="104" class="node-label">This hub</text>
          {#each peers as n (n.id)}
            <g class="peer-node" class:down={!n.up}>
              <circle cx={n.x} cy={n.y} r="20" class="node" />
              <text x={n.x} y={n.y + 4} class="node-label">{n.label.length > 12 ? `${n.label.slice(0, 11)}…` : n.label}</text>
            </g>
          {/each}
        </svg>
        <figcaption>Solid: link up. Dashed red: down, retrying.</figcaption>
      </figure>
      <div class="list">
        <ul role="listbox" aria-label={resource.plural} data-testid="resource-list" class="peers">
          {#each records as r (idOf(resource, r))}
            <li role="presentation">
              <button
                type="button"
                role="option"
                class="peer"
                class:down={!peerUp(r)}
                aria-selected={selected === idOf(resource, r)}
                data-testid="resource-row"
                onclick={() => pick(r)}>
                <span class="dot" aria-hidden="true"></span>
                <span class="title">{titleOf(resource, r)}</span>
                {#each badgesOf(resource, r) as b (b)}<span class="badge" data-testid="resource-badge">{b}</span>{/each}
                <span class="line" data-testid="peer-line">{peerLine(resource, r, now())}</span>
              </button>
            </li>
          {/each}
        </ul>
        {@render status()}
        {@render addControls()}
      </div>
    </div>
    <div class="detail below">{@render detail()}</div>
  {:else}
    {#if !readonly && rescan && records.length > 0}
      <div class="toolbar">
        <button type="button" class="btn" disabled={busy} data-testid="resource-scan-all" onclick={() => void scanAll()}>Scan all hosts</button>
      </div>
    {/if}
    <div class="table-wrap">
      <table class="rtable" role="grid" aria-label={resource.plural} data-testid="resource-table">
        <thead>
          <tr>
            <th scope="col">{resource.label}</th>
            {#each columns as c (c.id)}<th scope="col">{c.label}</th>{/each}
            {#if !readonly}<th scope="col"><span class="sr">Actions</span></th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each records as r (idOf(resource, r))}
            {@const sub = subOf(resource, r)}
            <!-- The title's button takes the keyboard; a click anywhere on
                 the row picks it too. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <tr data-testid="resource-row" aria-selected={selected === idOf(resource, r)} onclick={() => pick(r)}>
              <td class="lead">
                <button type="button" class="row-title" onclick={(e) => { e.stopPropagation(); pick(r); }}
                  >{titleOf(resource, r)}</button
                >
                {#each badgesOf(resource, r) as b (b)}<span class="badge" data-testid="resource-badge">{b}</span>{/each}
                {#if sub}<span class="sub" data-testid="resource-sub">{sub}</span>{/if}
                {#if typeof r.last_error === 'string' && r.last_error}<span class="sub err-line">{r.last_error}</span>{/if}
              </td>
              {#each columns as c (c.id)}<td class="cell" data-testid={`cell-${c.id}`}>{cellText(c, r, now())}</td>{/each}
              {#if !readonly}
                <td class="acts">
                  {#if resource.id === 'device' && r.trusted === false && r.this_device !== true && trustOf(bundle, titleOf(resource, r))}
                    <button
                      type="button"
                      class="btn"
                      disabled={busy}
                      data-testid="row-trust"
                      onclick={(e) => {
                        e.stopPropagation();
                        trustRow(r);
                      }}>Trust device…</button
                    >
                  {/if}
                  {#each rowActions(resource, r) as a (a.id)}
                    <button
                      type="button"
                      class="btn btn--quiet"
                      disabled={busy}
                      data-testid={`row-action-${a.id}`}
                      onclick={(e) => {
                        e.stopPropagation();
                        runRow(a, r);
                      }}>{a.label}</button
                    >
                  {/each}
                </td>
              {/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {@render status()}
    {@render addControls()}
    <div class="detail below">{@render detail()}</div>
  {/if}
</div>

{#snippet status()}
  {#if loaded && records.length === 0 && !error}
    <p class="empty" data-testid="resource-empty">{resource.empty}</p>
  {/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
{/snippet}

{#snippet addControls()}
  {#if !readonly && resource.create_flow}
    <button type="button" class="btn" data-testid="resource-add" disabled={adding} onclick={() => (adding = true)}
      >Add {resource.label.toLowerCase()}</button
    >
  {:else if !readonly && resource.create && pairWizard}
    <button
      type="button"
      class="btn"
      data-testid="resource-add"
      onclick={() => {
        wizardError = null;
        wizardOpen = true;
      }}>{resource.create.label}</button
    >
    {#if wizardOpen}
      <WizardDialog
        wizard={pairDeviceWizard($orgs)}
        busy={wizardBusy}
        error={wizardError}
        run={(v) => void pairFromWizard(v)}
        onclose={() => (wizardOpen = false)} />
    {/if}
  {:else if !readonly && resource.create}
    {#if adding}
      <ActionForm action={resource.create} {busy} options={createOptions} onrun={(p) => void create(p)} testid="resource-create" />
    {:else}
      <button type="button" class="btn" data-testid="resource-add" onclick={() => (adding = true)}>{resource.create.label}</button>
    {/if}
  {/if}
{/snippet}

{#snippet detail()}
  {#if pairing}
    <PairingResult {pairing} onclose={() => (pairing = null)} />
  {/if}
  {#if shown}
    <ActionResult {shown} onclose={() => (shown = null)} />
  {/if}
  {#if adding && resource.create_flow}
    <FlowView
      flow={resource.create_flow}
      oncancel={() => (adding = false)}
      ondone={(message, recordId) => {
        adding = false;
        push({ kind: 'success', message });
        void reload().then(() => {
          if (recordId !== null) selected = String(recordId);
          void afterChange(resource);
        });
      }} />
  {:else if current}
    {#key `${selected}:${version}`}
      <RecordEditor
        {resource}
        sections={page.sections ?? []}
        record={current}
        {readonly}
        {options}
        {actionOptions}
        {run}
        resources={bundle}
        bind:tab={recordTab}
        initialAction={rowAction}
        reload={() => void reload()} />
    {/key}
  {:else if loaded && records.length > 0}
    <p class="empty">Pick one.</p>
  {/if}
{/snippet}

{#if asking}
  <ConfirmDialog
    title={asking.title}
    message={asking.message}
    confirmLabel="Go ahead"
    danger
    confirmTestId="record-confirm"
    onconfirm={() => {
      const go = asking?.go;
      asking = null;
      go?.();
    }}
    oncancel={() => (asking = null)} />
{/if}

<style>
  .md {
    display: grid;
    grid-template-columns: 13rem minmax(0, 1fr);
    gap: 1rem;
    align-items: start;
  }
  @media (max-width: 720px) {
    .md {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li button {
    width: 100%;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem;
    text-align: left;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    padding: 0.35rem 0.45rem;
    font: inherit;
    font-size: var(--text-2xs);
    color: var(--fg);
    cursor: pointer;
  }
  li button:hover {
    background: var(--control-bg-hover);
  }
  li button[aria-selected='true'] {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 30%, transparent);
  }
  li button:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: var(--radius-xs);
    border: 1px solid var(--border);
  }
  .badge {
    font-size: var(--text-2xs);
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .empty,
  .reason {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin: 0;
  }
  .reason {
    margin-bottom: 0.6rem;
  }
  .err {
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
  .notice {
    font-size: var(--text-2xs);
    margin: 0 0 0.5rem;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
  }
  /* Table mode (boards OrgDevices, DebugDevices): the records as rows
     above the editor, each with its own actions. */
  .toolbar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 0.5rem;
  }
  .table-wrap {
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    margin-bottom: 0.5rem;
  }
  .rtable {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
  }
  .rtable th {
    text-align: left;
    font-weight: 500;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--border);
  }
  .rtable td {
    padding: 0.45rem 0.6rem;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .rtable tbody tr:last-child td {
    border-bottom: none;
  }
  .rtable tbody tr {
    cursor: pointer;
  }
  .rtable tbody tr:hover {
    background: var(--control-bg-hover);
  }
  .rtable tbody tr[aria-selected='true'] {
    background: var(--accent-soft);
  }
  .lead {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem;
  }
  .row-title {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--fg);
    cursor: pointer;
    text-align: left;
  }
  .row-title:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .sub {
    flex-basis: 100%;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err-line {
    color: var(--usage-crit);
  }
  .cell {
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .acts {
    text-align: right;
    white-space: nowrap;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .detail.below {
    margin-top: 0.75rem;
  }
  /* Federation: this hub and its peers, then the peers as rows. */
  .fed {
    display: grid;
    grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr);
    gap: 1rem;
    align-items: start;
  }
  @media (max-width: 720px) {
    .fed {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .topo {
    margin: 0;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .topo svg {
    width: 100%;
    height: auto;
    display: block;
  }
  .topo figcaption {
    text-align: center;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .edge {
    stroke: var(--usage-ok);
    stroke-width: 1.5;
  }
  .edge.down {
    stroke: var(--usage-crit);
    stroke-dasharray: 4 3;
  }
  .node {
    fill: var(--bg-pane);
    stroke: var(--usage-ok);
    stroke-width: 1.5;
  }
  .node.hub {
    stroke: var(--accent);
    fill: var(--accent-soft);
  }
  .peer-node.down .node {
    stroke: var(--usage-crit);
  }
  .node-label {
    fill: var(--fg);
    font-size: 9px;
    text-anchor: middle;
  }
  .peers {
    gap: 0.4rem;
  }
  .peer {
    border-color: var(--border);
    padding: 0.5rem 0.6rem;
  }
  .peer.down {
    border-color: color-mix(in srgb, var(--usage-crit) 50%, transparent);
  }
  .peer .dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--usage-ok);
  }
  .peer.down .dot {
    background: var(--usage-crit);
  }
  .peer .line {
    flex-basis: 100%;
    color: var(--fg-muted);
  }
  .peer.down .line {
    color: var(--usage-crit);
  }
</style>
